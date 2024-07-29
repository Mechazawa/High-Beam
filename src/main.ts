import {app, globalShortcut} from 'electron';
import PluginManager from "./PluginManager";
import Window from "./windows/query/window";
import HttpCodePlugin from "./plugins/HttpCodePlugin";

// Handle creating/removing shortcuts on Windows when installing/uninstalling.
if (require('electron-squirrel-startup')) {
  app.quit();
}

const pluginManager = new PluginManager();
const window = new Window(pluginManager);

pluginManager.load(HttpCodePlugin);

app.on('activate', () => window.open());
app.on('window-all-closed', (): void => void 0);
app.on('ready', async () => {
  globalShortcut.register('Shift+Space', () => window.open());

  await window.open();
});
