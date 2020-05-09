import { BrowserWindow, globalShortcut, ipcMain } from 'electron';
import { createProtocol } from 'vue-cli-plugin-electron-builder/lib';
import PluginManager from '../PluginManager';
import SpotlightPlugin from '../plugins/SpotlightPlugin';
import CalculatorPlugin from '../plugins/CalculatorPlugin';
import PaperSizePlugin from '../plugins/PaperSizePlugin';
import { CorePlugin } from '../plugins/CorePlugin';
import HttpCodePlugin from '../plugins/HttpCodePlugin';
import { DnDPlugin } from '../plugins/DnDPlugin';

// @todo split into abstract version
export default class QueryWindow {
  constructor () {
    this.pluginManager = new PluginManager();

    this.pluginManager.load(SpotlightPlugin);
    this.pluginManager.load(CalculatorPlugin);
    this.pluginManager.load(PaperSizePlugin);
    this.pluginManager.load(CorePlugin);
    this.pluginManager.load(HttpCodePlugin);
    this.pluginManager.load(DnDPlugin);

    globalShortcut.register('Meta+Space', () => this.open());
  }

  isOpen () {
    return Boolean(this.browser);
  }

  async open (options = {}) {
    if (this.isOpen()) {
      this.browser.focus();

      return;
    }

    this.browser = new BrowserWindow({
      width: 800,
      height: 73,
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

      this.browser.on('blur', () => this.close());
    }

    this.browser.once('closed', () => {
      this.browser = null;
      ipcMain.removeAllListeners();
    });

    this._initIpc();
  }

  close () {
    if (!this.browser) {
      return;
    }

    this.browser.close();

    this.browser = null;
  }

  _initIpc () {
    ipcMain.on('setBounds', (event, ...args) => event.reply('windowBounds', this.setBounds(...args)));
    ipcMain.on('input:query?', (event, ...args) => this.onInputQuery(event, ...args));
    ipcMain.on('input:select?', (event, ...args) => this.onInputSelect(event, ...args));
  }

  setBounds (bounds) {
    const animated = Boolean(bounds.animated);

    delete bounds.animated;

    this.browser.setBounds(bounds, animated);

    return this.browser.getBounds();
  }

  onInputQuery (event, replyKey, query) {
    const results = this.pluginManager.query(query);

    for (const result of results) {
      result.then(rows => {
        if (rows.length > 0) {
          event.reply(replyKey, rows);
        }
      });
    }
  }

  onInputSelect (event, pluginName, key, meta) {
    this.pluginManager.select(pluginName, key, meta);

    // @todo more functionality then just closing
    this.close();
  }
}
