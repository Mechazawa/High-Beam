import { BrowserWindow, ipcMain } from "electron";
import { createProtocol } from "vue-cli-plugin-electron-builder/lib";
import { debounce } from "../utils/functions";
import PluginManager from "../PluginManager";

// @todo split into abstract version
export default class QueryWindow {

  constructor () {
    this.pluginManager = new PluginManager();
    this._initIpc();
  }

  isOpen () {
    return Boolean(this.browser);
  }

  async open (options = {}) {
    if (this.isOpen()) {
      return;
    }

    this.browser = new BrowserWindow({
      width: 800,
      height: 80,
      webPreferences: {
        nodeIntegration: true,
      },
      frame: false,
      resizable: false,
      ...options,
    });

    this.browser.center();

    if (process.env.WEBPACK_DEV_SERVER_URL) {
      // Load the url of the dev server if in development mode
      await this.browser.loadURL(process.env.WEBPACK_DEV_SERVER_URL);

      if (!process.env.IS_TEST) this.browser.webContents.openDevTools();
    } else {
      createProtocol('app');
      // Load the index.html when not in development
      await this.browser.loadURL('app://./index.html');
    }

    this.browser.once('closed', () => {
      this.browser = null;
    });
  }

  close () {
    if (!this.browser) {
      return;
    }

    this.browser.close();

    this.browser = null;
  }

  _initIpc () {
    ipcMain.on('window:bounds?', (event, ...args) => this.onWindowBounds(event, ...args));
    ipcMain.on('input:query?', (event, ...args) => this.onInputQuery(event, ...args));
  }

  onWindowBounds (event, bounds) {
    const animated = Boolean(bounds.animated);

    delete bounds.animated;

    this.browser.setBounds(bounds, animated);

    event.reply('window:bounds', this.browser.getBounds());
  }

  @debounce(300)
  onInputQuery(event, query) {

  }
}