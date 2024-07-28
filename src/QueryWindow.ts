import {BrowserWindow, ipcMain} from 'electron';
import path from "path";
import PluginManager from "./PluginManager";
import QueryResultRow from "./plugins/interfaces/QueryResultRow";
import Rectangle = Electron.Rectangle;
import IpcMainEvent = Electron.IpcMainEvent;

export type QueryResponse = Omit<QueryResultRow, "call">[];

export default class QueryWindow {
  private window?: BrowserWindow;
  private pluginManager: PluginManager;
  private actions: { [key: string]: QueryResultRow['call'] } = {};

  constructor(pluginManager: PluginManager) {
    this.pluginManager = pluginManager;
  }

  isOpen() {
    return Boolean(this.window) && this.window.isClosable();
  }

  async open() {
    if (this.isOpen()) {
      this.window.focus();

      return;
    }

    this.window = new BrowserWindow({
      width: 800,
      height: 74,
      webPreferences: {
        nodeIntegration: true,
        preload: path.join(__dirname, 'preload.js'),
        disableHtmlFullscreenWindowResize: true,
        spellcheck: false,
        // enablePreferredSizeMode: true,
        enableWebSQL: false,
      },
      opacity: 0,
      frame: false,
      resizable: false,
      center: true,
      focusable: true,
    });

    if (MAIN_WINDOW_VITE_DEV_SERVER_URL) {
      await this.window.loadURL(MAIN_WINDOW_VITE_DEV_SERVER_URL);
    } else {
      await this.window.loadFile(path.join(__dirname, `../renderer/${MAIN_WINDOW_VITE_NAME}/index.html`));
    }

    this.window.on('blur', () => this.close());

    this.window.once('closed', () => {
      this.window = null;

      ipcMain.removeAllListeners();
    });

    this._initIpc();
  }

  close() {
    this.window?.close();
  }

  _initIpc() {
    ipcMain.on('setOpacity', (_, opacity: number) => this.window?.setOpacity(opacity));
    ipcMain.on('center', () => this.window?.center());
    ipcMain.on('setBounds', (_, bounds: Partial<Rectangle>, animate?: boolean) => this.window?.setBounds(bounds, animate));
    ipcMain.on('input:query?', (event, query: string) => this.onInputQuery(event, query));
    ipcMain.on('input:select?', (_, token: string, meta: boolean) => this.onInputSelect(token, meta));
  }

  async onInputQuery(event: IpcMainEvent, query: string) {
    const promises = this.pluginManager
      .query(query)
      .map(promise => promise.catch((): QueryResultRow[] => []));

    const results = (await Promise.all(promises)).flat();

    results.sort((a, b) => a.weight ?? 0 - b.weight ?? 0);

    const response = results.map(result => Object.fromEntries(
      Object.entries(result).filter(([key]) => key !== "call")
    )) as QueryResponse;
    const actions = results.map(result => [
      Math.random().toString(16).substring(2, 10),
      result.call,
    ]);

    this.actions = Object.fromEntries(actions);

    event.reply('result', response)
  }

  onInputSelect(key: string, meta: boolean) {
    const action = this.actions[key];

    if (typeof action === 'function') {
      action(meta);

      this.close();
    }
  }
}