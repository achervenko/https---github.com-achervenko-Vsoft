import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import './style.css';

const api = window.vsoft;
const names = {
  queued: 'В очереди', running: 'Конвертация…', ready: 'Готово',
  needs_review: 'Нужна сверка', error: 'Ошибка', result_error: 'Результат не подтверждён',
};
const flags = {
  dash_review: 'Проверить штриховые линии',
  plot_text_review: 'Сверить текст в печати',
  plot_title_review: 'Сверить название в штампе',
  unmapped_characters: 'Проверить символы',
  missing_text: 'Не найден текст',
  missing_geometry: 'Не найдена геометрия',
};

function filename(file) {
  return file.split(/[\\/]/).at(-1);
}

function Result({ job, onOpen }) {
  return <div className="job">
    <div className="job-head">
      {job.range && <span>Листы {job.range}</span>}
      <span className={'badge ' + job.status}>{names[job.status] || names.error}</span>
      {job.dwg && <button className="link" onClick={() => onOpen(job.dwg)}>Открыть DWG</button>}
    </div>
    {job.message && <p>{job.message}</p>}
    {job.sheets?.map(({ sheet, review }) =>
      <p className="notice" key={sheet}>Лист {sheet}: {review.map((flag) => flags[flag] || flag).join(', ')}</p>
    )}
    {job.diagnostic && !['ready', 'needs_review'].includes(job.status) &&
      <details><summary>Подробности</summary><pre>{job.diagnostic}</pre></details>}
  </div>;
}

function App() {
  const [{ files, selection }, setQueue] = useState({ files: [], selection: '' });
  const [destination, setDestination] = useState('');
  const [busy, setBusy] = useState(false);
  const [hover, setHover] = useState(false);
  const [notice, setNotice] = useState('');
  const done = files.filter((file) => !['queued', 'running'].includes(file.status)).length;

  async function safely(action) {
    try {
      await action();
    } catch (error) {
      setNotice(error.message);
    }
  }

  async function add(result) {
    const { files: inputs, skipped } = await result;
    setQueue((current) => {
      const sources = new Set(current.files.map((file) => file.source));
      const newFiles = [...new Set(inputs)]
        .filter((source) => !sources.has(source))
        .map((source) => ({ source, status: 'queued', jobs: [] }));
      const files = [...current.files, ...newFiles];
      return { files, selection: files.length > 1 ? '' : current.selection };
    });
    setNotice(skipped.length ? 'Не удалось прочитать некоторые пути.' : '');
  }

  async function convert() {
    if (!files.length || !destination || busy) return;
    setBusy(true);
    setNotice('');
    setQueue((current) => ({
      ...current,
      files: current.files.map((item) => ({ ...item, status: 'queued', jobs: [] })),
    }));
    const pages = files.length === 1 ? selection.trim() : '';
    try {
      for (const file of files) {
        update(file.source, 'running');
        try {
          const jobs = await api.convert(file.source, pages);
          const status = jobs.some((job) => job.status === 'error') ? 'error'
            : jobs.some((job) => job.status === 'result_error') ? 'result_error'
            : jobs.some((job) => job.status === 'needs_review') ? 'needs_review' : 'ready';
          update(file.source, status, jobs);
        } catch (error) {
          update(file.source, 'error', [{ status: 'error', message: error.message }]);
        }
      }
    } finally {
      setBusy(false);
    }
  }

  function update(source, status, jobs = []) {
    setQueue((current) => ({
      ...current,
      files: current.files.map((item) => item.source === source ? { ...item, status, jobs } : item),
    }));
  }

  async function open(action) {
    await safely(async () => {
      const error = await action();
      if (error) setNotice(error);
    });
  }

  return <main>
    <header>
      <div className="brand"><span className="mark">V</span><strong>VSoft</strong></div>
      <span className="format">PDF <span>→</span> DWG</span>
    </header>

    <section className={'drop ' + (hover ? 'hover' : '')}
      onDragOver={(event) => { event.preventDefault(); if (!busy) setHover(true); }}
      onDragLeave={() => setHover(false)}
      onDrop={(event) => {
        event.preventDefault(); setHover(false);
        if (!busy) safely(() => add(api.addDropped(Array.from(event.dataTransfer.files))));
      }}>
      <div className="drop-icon" aria-hidden="true">↓</div>
      <strong>Перетащите PDF или папку сюда</strong>
      <div className="actions">
        <button disabled={busy} onClick={() => safely(() => add(api.chooseInput(false)))}>Выбрать PDF</button>
        <button disabled={busy} onClick={() => safely(() => add(api.chooseInput(true)))}>Выбрать папку</button>
      </div>
    </section>

    <section className="output">
      <div>
        <label>Папка результатов</label>
        <div className="path" title={destination}>{destination || 'Папка не выбрана'}</div>
      </div>
      <button disabled={busy} onClick={() => safely(async () => {
        const value = await api.chooseOutput();
        if (value) setDestination(value);
      })}>Выбрать</button>
    </section>

    {files.length === 1 && <label className="pages">Листы
      <input value={selection} onChange={(event) => {
        const value = event.target.value;
        setQueue((current) => ({ ...current, selection: value }));
      }}
        placeholder="Все листы" maxLength={1024} disabled={busy} />
    </label>}

    {notice && <p className="notice" role="alert">{notice}</p>}
    {files.length > 0 && <section className="queue" aria-label="Очередь">
      <div className="queue-head"><h2>Файлы <span>{files.length}</span></h2>
        <button className="link" disabled={busy} onClick={() => {
          setQueue({ files: [], selection: '' }); setNotice('');
        }}>Очистить</button>
      </div>
      {files.map((file) => <article key={file.source}>
        <div className="file-head">
          <span className="file-name" title={file.source}>{filename(file.source)}</span>
          <span className={'badge ' + file.status}>{names[file.status]}</span>
        </div>
        {file.jobs.map((job, index) => <Result key={index} job={job} onOpen={(dwg) => open(() => api.openDwg(dwg))} />)}
      </article>)}
    </section>}

    <footer>
      <span aria-live="polite">{files.length ? `Завершено: ${done} из ${files.length}` : ' '}</span>
      <div className="actions">
        <button disabled={!destination || busy || !done} onClick={() => open(api.openOutput)}>Открыть результаты</button>
        <button className="primary" disabled={busy || !files.length || !destination} onClick={convert}>
          {busy ? 'Конвертация…' : 'Конвертировать'}
        </button>
      </div>
    </footer>
  </main>;
}

createRoot(document.getElementById('root')).render(<App />);
