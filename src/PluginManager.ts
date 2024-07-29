import Plugin, {ResultCollection} from './plugins/interfaces/Plugin';
import PluginLoaderException from "./exceptions/PluginLoaderException";

type QueryFn = (...args: Parameters<Plugin['query']>) => Promise<ResultCollection>;
interface PluginEntry {
  plugin: Plugin,
  query: QueryFn,
}

/**
 * Manages loading of plugins and communication between
 * them and the rest of the application.
 */
export default class PluginManager {
  /**
   * LoadedPlugins
   */
  private plugins: Map<string, PluginEntry> = new Map();

  /**
   * Load a plugin
   * @param target Require path or
   */
  public load(target: Plugin | (new () => Plugin)): Plugin {
    try {
      const plugin: Plugin = typeof target === 'function' ? new target() : target;
      const query = PluginManager.buildDebouncedQuery(plugin, plugin.debounce);

      this.plugins.set(plugin.name, {
        plugin, query,
      });

      console.log('PluginManager loaded', plugin.name);

      return plugin;
    } catch (err) {
      const name = typeof target === 'function' ? target.prototype.name : target.name;

      throw new PluginLoaderException(`Failed to load plugin: ${name} (cwd: ${__dirname})`, err);
    }
  }

  /**
   * Query the plugins
   * @param str query string
   */
  public query(str: string): Promise<ResultCollection>[] {
    return Array.from(this.plugins.values()).map(entry => entry.query(str));
  }

  /**
   * Unload a plugin
   * @param name plugin name
   * @returns success
   */
  public unload(name: string): boolean {
    return this.plugins.delete(name);
  }

  /**
   * Get a plugin by name
   * @param name plugin name
   */
  public getPlugin(name: string): PluginEntry {
    return this.plugins.get(name);
  }

  /**
   * Get a list of loaded plugins
   */
  public list(): Plugin[] {
    return Array.from(this.plugins.values()).map(entry => entry.plugin);
  }

  /**
   * Build debounced query function
   * @param plugin plugin instance
   * @param ms
   * @private
   */
  private static buildDebouncedQuery(plugin: Plugin, ms: number): QueryFn {
    let timeout: NodeJS.Timeout;
    let promise: Promise<ResultCollection>;
    let resolve: (output: ResultCollection) => void;
    let reject: (err?: unknown) => void;

    const queryFn = plugin.query.bind(plugin);

    return (...args: Parameters<Plugin['query']>): Promise<ResultCollection> => {
      clearTimeout(timeout);

      reject?.();

      promise = new Promise((_resolve, _reject) => {
        resolve = _resolve;
        reject = _reject;
      });

      timeout = setTimeout(() => {
        resolve?.(queryFn(...args));
      }, ms)

      return promise;
    };
  }
}

