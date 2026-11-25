const { app, BrowserWindow, dialog, ipcMain, shell } = require('electron');
const fs = require('node:fs/promises');
const path = require('node:path');
const { collectPdfs, convertPdf } = require('./engine.cjs');

const root = path.resolve(__dirname, '..');
const sources = new Set();
const results = new Set();
let destination = '';
let running = false;

async function addInputs(paths) {
  const found = await collectPdfs(paths);
  for (const source of found.files) sources.add(source);
  return found;
}

function registerHandlers() {
  ipcMain.handle('vsoft:choose-input', async (event, folder) => {
    const selected = await dialog.showOpenDialog(BrowserWindow.fromWebContents(event.sender), {
      properties: folder ? ['openDirectory'] : ['openFile', 'multiSelections'],
      ...(folder ? {} : { filters: [{ name: 'PDF', extensions: ['pdf'] }] }),
    });
    return selected.canceled ? { files: [], skipped: [] } : addInputs(selected.filePaths);
  });
  ipcMain.handle('vsoft:drop', (_, paths) => addInputs(paths));
  ipcMain.handle('vsoft:choose-output', async (event) => {
    const selected = await dialog.showOpenDialog(BrowserWindow.fromWebContents(event.sender), {
      properties: ['openDirectory', 'createDirectory'],
    });
    if (selected.canceled) return '';
    destination = await fs.realpath(selected.filePaths[0]);
    return destination;
  });
  ipcMain.handle('vsoft:convert', async (_, source, selection) => {
    if (running) throw new Error('Конвертация уже выполняется.');
    if (!destination || !sources.has(source)) throw new Error('Выберите PDF и папку результатов.');
    if (typeof selection !== 'string' || selection.length > 1024) throw new Error('Некорректный выбор листов.');
    running = true;
    try {
      const jobs = await convertPdf({
        root, source, destination, selection,
        beforeWrite(paths) {
          for (const file of paths) results.delete(file);
        },
      });
      for (const job of jobs) {
        if (['ready', 'needs_review'].includes(job.status) && job.dwg) results.add(job.dwg);
      }
      return jobs;
    } finally {
      running = false;
    }
  });
  ipcMain.handle('vsoft:open-dwg', (_, file) => {
    if (!results.has(file)) throw new Error('DWG не найден в результатах текущего сеанса.');
    return shell.openPath(file);
  });
  ipcMain.handle('vsoft:open-output', () => destination && shell.openPath(destination));
}

app.whenReady().then(() => {
  registerHandlers();
  const window = new BrowserWindow({
    title: 'VSoft', width: 980, height: 820, minWidth: 690, minHeight: 640,
    backgroundColor: '#f7f8fa', autoHideMenuBar: true,
    webPreferences: { preload: path.join(__dirname, 'preload.cjs'), contextIsolation: true, sandbox: true },
  });
  window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  window.webContents.on('will-navigate', (event) => event.preventDefault());
  window.on('close', (event) => {
    if (running) event.preventDefault();
  });
  window.loadFile(path.join(__dirname, 'dist', 'index.html'));
});
app.on('window-all-closed', () => app.quit());
