const { contextBridge, ipcRenderer, webUtils } = require('electron');

contextBridge.exposeInMainWorld('vsoft', {
  chooseInput: (folder) => ipcRenderer.invoke('vsoft:choose-input', folder),
  addDropped: (files) => ipcRenderer.invoke('vsoft:drop', files.map((file) => webUtils.getPathForFile(file)).filter(Boolean)),
  chooseOutput: () => ipcRenderer.invoke('vsoft:choose-output'),
  convert: (source, selection) => ipcRenderer.invoke('vsoft:convert', source, selection),
  openDwg: (file) => ipcRenderer.invoke('vsoft:open-dwg', file),
  openOutput: () => ipcRenderer.invoke('vsoft:open-output'),
});
