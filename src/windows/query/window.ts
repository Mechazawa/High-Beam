import {BrowserWindow, ipcMain, screen} from 'electron';
import path from "path";
import PluginManager from "../../PluginManager";
import QueryResult from "../../plugins/interfaces/QueryResult";
import Rectangle = Electron.Rectangle;
import IpcMainEvent = Electron.IpcMainEvent;
import WindowInterface from "../WindowInterface";

export type QueryResponse = Omit<QueryResult, "call">[];

export default class Window implements WindowInterface {
  static PATH = 'src/windows/query/';

  private window?: BrowserWindow;
  private pluginManager: PluginManager;
  private actions: { [key: string]: QueryResult['call'] } = {};

  constructor(pluginManager: PluginManager) {
    this.pluginManager = pluginManager;
  }

  /**
   * If the window is currently open
   */
  isOpen(): boolean {
    return Boolean(this.window) && this.window.isClosable();
  }

  /**
   * Open the query window and register listeners
   */
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
        contextIsolation: false,
        preload: path.join(__dirname, 'preload.js'),
        disableHtmlFullscreenWindowResize: true,
        spellcheck: false,
        // enablePreferredSizeMode: true,
        enableWebSQL: false,
      },
      opacity: 0.96,
      frame: false,
      resizable: false,
      focusable: true,
      alwaysOnTop: true,
      fullscreenable: false,
      minimizable: false,
    });

    if (MAIN_WINDOW_VITE_DEV_SERVER_URL) {
      await this.window.loadURL(`${MAIN_WINDOW_VITE_DEV_SERVER_URL}/${Window.PATH}`);

      this.window.webContents.openDevTools({mode: 'undocked'});
    } else {
      await this.window.loadFile(path.join(__dirname, `../renderer/${MAIN_WINDOW_VITE_NAME}/${Window.PATH}`));

      this.window.on('blur', () => this.close());
    }

    this.window.once('closed', () => {
      this.window = null;

      ipcMain.removeAllListeners();
    });

    this.center()

    this._initIpc();
  }

  center() {
    if (!this.isOpen()) {
      return;
    }

    const bounds = this.window.getBounds();
    const screenSize = screen.getDisplayNearestPoint(bounds).workAreaSize;

    const x = (screenSize.width / 2) - (bounds.width / 2);
    const y = screenSize.height / 2 - bounds.height; // todo better value, pulled this out of my ass

    this.window.setBounds({x, y});
  }

  /**
   * Close the window
   */
  close() {
    this.window?.close();

    this.window = null;
  }

  _initIpc() {
    ipcMain.on('setOpacity', (_, opacity: number) => this.window?.setOpacity(opacity));
    ipcMain.on('center', () => this.center());
    ipcMain.on('setBounds', (_, bounds: Partial<Rectangle>, animate?: boolean) => this.window?.setBounds(bounds, animate));
    ipcMain.on('query', (event, query: string) => this.onInputQuery(event, query));
    ipcMain.on('select', (_, token: string, meta: boolean) => this.onInputSelect(token, meta));
  }

  async onInputQuery(event: IpcMainEvent, query: string) {
    const promises = this.pluginManager
      .query(query)
      .map(promise => promise.catch((): QueryResult[] => []));

    const results = (await Promise.all(promises))
      .flat()
      .sort((a, b) => a.weight ?? 0 - b.weight ?? 0)
      .slice(0, 9) // 9 rows max, todo make configurable
      .map((result) => ({
        ...result,
        // todo subtype that contains token
        token: Math.random().toString(16).substring(2, 10),
      }));

    const response = results.map(result => Object.fromEntries(
      Object.entries(result).filter(([key]) => key !== "call")
    )) as QueryResponse;
    const actions = results.map(result => [
      result.token,
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