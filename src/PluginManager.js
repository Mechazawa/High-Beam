/**
 * Manages loading of plugins and communication between
 * them and the rest of the application.
 */
import { asyncDebounce } from './utils/functions';

export default class PluginManager {
  /**
   * Collection of loaded plugins
   * @type {Set<AbstractPlugin>}
   */
  _plugins = null;

  /**
   * Last bounce lookup map
   * @type {WeakMap<object, any>}
   * @private
   */
  _bounced = new WeakMap();

  constructor () {
    this._plugins = new Set();
  }

  /**
   * Load a plugin
   * @param {string|constructor<AbstractPlugin>} path - node path
   * @returns {?AbstractPlugin} - loaded plugin
   */
  load (path) {
    try {
      const PluginConstructor = typeof path === 'string' ? require(path) : path;
      /** @type {AbstractPlugin} */
      const plugin = typeof PluginConstructor === 'function' ? new PluginConstructor() : PluginConstructor;

      this._plugins.add(plugin);

      if (plugin.debounce > 0) {
        this._bounced.set(plugin, asyncDebounce(plugin.query.bind(plugin), plugin.debounce, false, []));
      } else {
        this._bounced.set(plugin, plugin.query.bind(plugin));
      }

      console.log('PluginManager loaded', Object.getPrototypeOf(plugin).constructor.name);

      return plugin;
    } catch (e) {
      console.error(`FAILED TO LOAD PLUGIN: ${path} (cwd: ${__dirname})`);
      console.error(e);

      return undefined;
    }
  }

  /**
   * Query the plugins
   * To obtain results just listen for the query:response event
   * @param {string} str - query string
   * @returns {Array<Promise<Array<QueryResultRow>>>}
   */
  query (str) {
    const output = [];

    this._plugins.forEach(plugin => output.push(this._bounced.get(plugin)(str)));

    return output;
  }

  /**
   * Select a query result row
   * @param {string} name - plugin name
   * @param {string|number} key - row key
   * @returns {Promise<void>|void}
   * @todo decide how I wanna do stuff that doesn't just close the window, show extra info, a new view etc
   */
  select (name, key) {
    return this.getPlugin(name)?.select(key);
  }

  /**
   * Unload a plugin
   * @param {string} name - plugin name
   * @returns {boolean} - success
   */
  unload (name) {
    const plugin = this.getPlugin(name);

    if (!plugin) {
      return false;
    }

    plugin.removeAllListeners();

    return this._plugins.delete(plugin) && this._bounced.delete(plugin);
  }

  /**
   * Get a plugin by name
   * @param {string} name - plugin name
   * @returns {AbstractPlugin}
   */
  getPlugin (name) {
    return this.list().find(plugin => plugin.name === name);
  }

  /**
   * Get a list of loaded plugins
   */
  list () {
    return Array.from(this._plugins);
  }
}

