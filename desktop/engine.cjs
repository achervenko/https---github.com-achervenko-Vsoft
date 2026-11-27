const { spawn } = require('node:child_process');
const { constants } = require('node:fs');
const fs = require('node:fs/promises');
const path = require('node:path');

function execute(file, args, cwd) {
  return new Promise((resolve, reject) => {
    const child = spawn(file, args, { cwd, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    child.stdout.setEncoding('utf8').on('data', (value) => { stdout += value; });
    child.stderr.setEncoding('utf8').on('data', (value) => {
      stderr = (stderr + value).slice(-12000);
    });
    child.on('error', reject);
    child.on('close', (code) => resolve({ code, stdout, stderr }));
  });
}

async function collectPdfs(paths) {
  const files = new Set();
  const skipped = [];
  async function visit(candidate) {
    try {
      const entry = await fs.lstat(candidate);
      if (entry.isSymbolicLink()) return;
      if (entry.isDirectory()) {
        const names = (await fs.readdir(candidate)).sort();
        for (const name of names) await visit(path.join(candidate, name));
      } else if (entry.isFile() && path.extname(candidate).toLowerCase() === '.pdf') {
        files.add(await fs.realpath(candidate));
      }
    } catch {
      skipped.push(candidate);
    }
  }
  for (const candidate of paths) {
    if (typeof candidate === 'string' && path.isAbsolute(candidate)) await visit(candidate);
  }
  return { files: [...files], skipped };
}

function parseOutcome({ code, stdout }, expectedDwg) {
  let outcome;
  try {
    outcome = JSON.parse(stdout);
  } catch {
    outcome = null;
  }
  switch (outcome?.status) {
    case 'ready':
      if (code === 0 && outcome.dwg === expectedDwg) return { status: 'ready', dwg: outcome.dwg };
      break;
    case 'needs_review':
      if (code === 0 && outcome.dwg === expectedDwg &&
          Array.isArray(outcome.sheets) && outcome.sheets.length > 0 &&
          outcome.sheets.every((entry) => Number.isSafeInteger(entry?.sheet) && entry.sheet > 0 &&
            Array.isArray(entry.review) && entry.review.length > 0 &&
            entry.review.every((flag) => typeof flag === 'string'))) {
        return { status: 'needs_review', dwg: outcome.dwg, sheets: outcome.sheets };
      }
      break;
    case 'error':
      if (Number.isInteger(outcome.exit_code) && outcome.exit_code !== 0 && outcome.exit_code === code) {
        return { status: 'error', exit_code: outcome.exit_code };
      }
      break;
    case 'result_error':
      if (code === 1 && typeof outcome.message === 'string' && outcome.message.trim()) {
        return { status: 'result_error', message: outcome.message };
      }
      break;
  }
  return { status: 'result_error', message: 'Ошибка протокола: движок вернул некорректный результат.' };
}

async function convertPdf({ root, source, destination, workspace, selection, beforeWrite }) {
  const executable = path.resolve(root, process.env.VSOFT_BIN || path.join('target', 'release', 'vsoft.exe'));
  const profile = process.env.VSOFT_PROFILE || 'universal';
  const reference = profile === 'eplan' ? path.resolve(root, process.env.VSOFT_REFERENCE || 'reference.dxf') : 'none';
  await fs.access(executable).catch(() => { throw new Error('Соберите движок командой cargo build --release.'); });
  if (profile === 'eplan') {
    await fs.access(reference).catch(() => { throw new Error('Не найден reference.dxf. Положите референс в корень проекта.'); });
  }
  const published = path.join(destination, path.parse(source).name + '.dwg');
  const collision = { status: 'result_error', message: 'DWG с таким именем уже существует. Выберите другую папку результатов.' };
  try {
    await fs.lstat(published);
    return [collision];
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  beforeWrite?.([published]);
  await fs.mkdir(workspace, { recursive: true });
  const output = await fs.mkdtemp(path.join(workspace, 'job-'));
  const pages = selection;
  const result = await execute(executable, [
    'convert-result', source, reference, output, '--profile', profile,
    ...(profile === 'universal' && process.env.VSOFT_BUILDER ? ['--builder', process.env.VSOFT_BUILDER] : []),
    ...(pages ? ['--pages', pages] : []),
  ], root);
  const outcome = parseOutcome(result, path.join(output, 'drawing.dwg'));
  const diagnostic = result.stderr.trim();
  const size = diagnostic.match(/^Sheet (\d+): unsupported page size \(([\d.]+) x ([\d.]+) pt; expected ([\d.]+) x ([\d.]+) pt\)$/m);
  const rotation = diagnostic.match(/^Sheet (\d+): unsupported rotation \((\d+) degrees\)$/m);
  const missing = diagnostic.match(/^Sheet (\d+): (missing PDF text|missing vector geometry)$/m);
  if (outcome.status === 'error') {
    if (size) outcome.message = `Лист ${size[1]}: размер ${size[2]} × ${size[3]} пт. Текущий профиль поддерживает ${size[4]} × ${size[5]} пт.`;
    if (rotation) outcome.message = `Лист ${rotation[1]}: текущий профиль не поддерживает поворот на ${rotation[2]}°.`;
    if (missing) outcome.message = `Лист ${missing[1]}: ${missing[2] === 'missing PDF text' ? 'в PDF нет текстового слоя' : 'в PDF нет векторной геометрии'}.`;
  }
  if (outcome.dwg) {
    try {
      await fs.copyFile(outcome.dwg, published, constants.COPYFILE_EXCL);
    } catch (error) {
      if (error.code === 'EEXIST') return [collision];
      return [{ status: 'result_error', message: 'Не удалось сохранить DWG.', diagnostic: error.message }];
    }
    outcome.dwg = published;
  }
  return [{ ...outcome, range: pages, diagnostic }];
}

module.exports = { collectPdfs, convertPdf };
