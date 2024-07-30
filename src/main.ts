import {app, globalShortcut} from 'electron';
import PluginManager from "./PluginManager";
import Window from "./windows/query/window";
import HttpCodePlugin from "./plugins/HttpCodePlugin";
import CalculatorPlugin from "./plugins/CalculatorPlugin";

// Handle creating/removing shortcuts on Windows when installing/uninstalling.
if (require('electron-squirrel-startup')) {
  app.quit();
}

const pluginManager = new PluginManager();
const window = new Window(pluginManager);

// todo dynamic plugin loading based on config
pluginManager.load(HttpCodePlugin);
pluginManager.load(CalculatorPlugin);

app.on('activate', () => window.open());
app.on('window-all-closed', (): void => void 0);
app.on('ready', async () => {
  globalShortcut.register('Shift+Space', () => window.open());

  await window.open();
});
