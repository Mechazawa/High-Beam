import {app, globalShortcut} from 'electron';
import PluginManager from "./PluginManager";
import QueryWindow from "./QueryWindow";

// Handle creating/removing shortcuts on Windows when installing/uninstalling.
if (require('electron-squirrel-startup')) {
  app.quit();
}

const window = new QueryWindow(new PluginManager());

app.on('activate', () => window.open());
app.on('window-all-closed', (): void => void 0);
app.on('ready', async () => {
  globalShortcut.register('Shift+Space', () => window.open());

  await window.open();
});