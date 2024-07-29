// See the Electron documentation for details on how to use preload scripts:
// https://www.electronjs.org/docs/latest/tutorial/process-model#preload-scripts

import { ipcRenderer } from 'electron'
window.ipcRenderer = ipcRenderer; // todo fix type error

console.log("Preload done");