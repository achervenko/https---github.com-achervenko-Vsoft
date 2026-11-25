import os
import queue
import subprocess
import sys
import threading
import tkinter as tk
from pathlib import Path
from tkinter import filedialog, messagebox, ttk

HERE = Path(__file__).resolve().parent
DEFAULT_ACAD = Path(r'C:\Program Files\Autodesk\AutoCAD 2024\accoreconsole.exe')


def commands(source, destination, reference, pages):
    if source.is_dir():
        manifest = destination / 'queue.json'
        return [
            [HERE / 'make_queue.py', source, destination, manifest],
            [HERE / 'batch.py', manifest, reference],
        ]
    command = [HERE / 'convert.py', source, reference, destination]
    if pages:
        command.extend(['--pages', pages])
    return [command]


class Converter:
    def __init__(self, root):
        self.root = root
        self.events = queue.Queue()
        self.running = False
        self.source = tk.StringVar()
        self.destination = tk.StringVar()
        self.pages = tk.StringVar()
        self.status = tk.StringVar(value='Готово к запуску')
        root.title('PDF → AutoCAD')
        root.geometry('780x490')
        root.minsize(650, 400)
        panel = ttk.Frame(root, padding=16)
        panel.pack(fill='both', expand=True)
        panel.columnconfigure(1, weight=1)
        panel.rowconfigure(5, weight=1)
        ttk.Label(panel, text='Пакетный перенос PDF в DWG', font=('Segoe UI', 16, 'bold')).grid(row=0, columnspan=4, sticky='w', pady=(0, 14))
        ttk.Label(panel, text='Источник').grid(row=1, column=0, sticky='w', padx=(0, 10))
        ttk.Entry(panel, textvariable=self.source).grid(row=1, column=1, sticky='ew')
        ttk.Button(panel, text='PDF…', command=self.choose_pdf).grid(row=1, column=2, padx=6)
        ttk.Button(panel, text='Папка…', command=lambda: self.choose_dir(self.source)).grid(row=1, column=3)
        ttk.Label(panel, text='Результаты').grid(row=2, column=0, sticky='w')
        ttk.Entry(panel, textvariable=self.destination).grid(row=2, column=1, sticky='ew', pady=8)
        ttk.Button(panel, text='Папка…', command=lambda: self.choose_dir(self.destination)).grid(row=2, column=3)
        ttk.Label(panel, text='Листы').grid(row=3, column=0, sticky='w')
        ttk.Entry(panel, textvariable=self.pages, width=18).grid(row=3, column=1, sticky='w')
        ttk.Label(panel, text='Один PDF: например 1,5-12. Пусто — все листы.').grid(row=4, column=1, columnspan=3, sticky='w', pady=(1, 10))
        log_frame = ttk.Frame(panel)
        log_frame.grid(row=5, columnspan=4, sticky='nsew')
        self.log = tk.Text(log_frame, wrap='word', state='disabled', font=('Consolas', 10))
        self.log.pack(side='left', fill='both', expand=True)
        scroll = ttk.Scrollbar(log_frame, command=self.log.yview)
        scroll.pack(side='right', fill='y')
        self.log.configure(yscrollcommand=scroll.set)
        footer = ttk.Frame(panel)
        footer.grid(row=6, columnspan=4, sticky='ew', pady=(12, 0))
        self.start_button = ttk.Button(footer, text='Конвертировать', command=self.start)
        self.start_button.pack(side='left')
        ttk.Button(footer, text='Открыть результаты', command=self.open_results).pack(side='left', padx=8)
        ttk.Label(footer, textvariable=self.status).pack(side='right')
        root.protocol('WM_DELETE_WINDOW', self.close)
        root.after(150, self.poll)

    def choose_pdf(self):
        path = filedialog.askopenfilename(filetypes=[('PDF', '*.pdf')])
        if path:
            self.source.set(path)

    def choose_dir(self, variable):
        path = filedialog.askdirectory()
        if path:
            variable.set(path)

    def start(self):
        source_value, dest_value = self.source.get().strip(), self.destination.get().strip()
        source, destination = Path(source_value), Path(dest_value)
        pages = self.pages.get().strip()
        reference = HERE / 'reference.dxf'
        if not source_value or not (source.is_dir() or (source.is_file() and source.suffix.lower() == '.pdf')):
            messagebox.showerror('Источник', 'Выбери PDF или папку с PDF.')
            return
        if not dest_value or (destination.exists() and not destination.is_dir()):
            messagebox.showerror('Результаты', 'Укажи папку для результатов.')
            return
        if source.is_dir() and (pages or destination.resolve().is_relative_to(source.resolve())):
            messagebox.showerror('Режим папки', 'Оставь поле «Листы» пустым и выбери папку результатов вне папки источника.')
            return
        if not DEFAULT_ACAD.is_file() or not reference.is_file():
            messagebox.showerror('Подготовка', 'Нужны AutoCAD 2024 и файл reference.dxf рядом с программой.')
            return
        self.running = True
        self.start_button.configure(state='disabled')
        self.status.set('Обработка…')
        self.append(f'Источник: {source}\nРезультаты: {destination}\n')
        threading.Thread(target=self.run, args=(commands(source.resolve(), destination.resolve(), reference, pages),), daemon=True).start()

    def run(self, plan):
        env = dict(os.environ, PYTHONIOENCODING='utf-8', PYTHONUNBUFFERED='1')
        try:
            for arguments in plan:
                with subprocess.Popen([sys.executable, *map(str, arguments)], stdout=subprocess.PIPE,
                                      stderr=subprocess.STDOUT, text=True, encoding='utf-8', errors='replace',
                                      env=env, creationflags=subprocess.CREATE_NO_WINDOW) as process:
                    for line in process.stdout:
                        self.events.put(('log', line))
                    if process.wait():
                        self.events.put(('done', 'Есть ошибки. См. журнал.'))
                        return
        except OSError as error:
            self.events.put(('log', str(error) + '\n'))
            self.events.put(('done', 'Ошибка запуска'))
            return
        self.events.put(('done', 'Обработка завершена'))

    def append(self, value):
        self.log.configure(state='normal')
        self.log.insert('end', value)
        self.log.see('end')
        self.log.configure(state='disabled')

    def poll(self):
        while not self.events.empty():
            kind, value = self.events.get_nowait()
            if kind == 'log':
                self.append(value)
            else:
                self.running = False
                self.status.set(value)
                self.start_button.configure(state='normal')
        self.root.after(150, self.poll)

    def open_results(self):
        value = self.destination.get().strip()
        if value and Path(value).is_dir():
            os.startfile(value)
        else:
            messagebox.showinfo('Результаты', 'Папка результатов пока не создана.')

    def close(self):
        if self.running:
            messagebox.showinfo('Идёт обработка', 'Окно можно закрыть после завершения обработки.')
        else:
            self.root.destroy()


if __name__ == '__main__':
    root = tk.Tk()
    Converter(root)
    root.mainloop()
