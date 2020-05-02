import { abstract } from './utils/class';

/**
 * Manages loading of plugins and communication between
 * them and the rest of the application.
 */
export default class PluginManager {
  /**
   * Collection of loaded plugins
   * @type {Set<AbstractPlugin>}
   */
  _plugins = new Set();

  /**
   * Load a plugin
   * @param {string} path - node path
   * @returns {?AbstractPlugin} - loaded plugin
   */
  load (path) {
    try {
      const PluginConstructor = require(path);
      const plugin = new PluginConstructor();

      this._plugins.add(plugin);

      return plugin;
    } catch (e) {
      console.error(`FAILED TO LOAD PLUGIN: ${path}`);
      console.error(e);

      return undefined;
    }
  }

  /**
   * Unload a plugin
   * @param {string} name - plugin name
   * @returns {boolean} - success
   */
  unload (name) {
    return this._plugins.delete(this.getPlugin(name));
  }

  /**
   * Get a plugin by name
   * @param {string} name - plugin name
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

/**
 * A query result row
 * @typedef {{
 *    icon: string,
 *    weight: number,
 *    description: string,
 *    title: string,
 *    key: ?(string|number)
 * }} QueryResultRow
 */

/**
 * Abstract plugin
 */
@abstract
class AbstractPlugin {
  @abstract
  name;

  /**
   *
   * @param query
   * @returns QueryResultRow[]
   */
  @abstract
  onQuery (query) {
    return [
      {
        weight: 50, // 0-100
        title: '',
        description: '',
        icon: '', // can be missing or use a sprite
        key: 'asdasdasd',
      },
    ];
  }
}

export { AbstractPlugin };

export class AbstractKeywordPlugin extends AbstractPlugin {
  /**
   * List of keywords
   * @type {Array<RegExp|string>}
   */
  @abstract
  keywords = [
    'http',
  ];

  /**
   * @inheritDoc
   */
  onQuery (query) {
    const output = [];

    for (let i = 0; i < this.keywords.length; i++) {
      const keyword = this.keywords[i];

      if (typeof keyword === 'string') {
        if (query.startsWith(`${keyword} `)) {
          const args = query.replace(new RegExp('^' + keyword), '');

          output.push(...[this.onKeyword(args, i)].flat(1));
        }
      } else {
        const match = query.match(keyword);

        if (match) {
          output.push(...[this.onKeyword(match, i)].flat(1));
        }
      }
    }

    return output;
  }

  /**
   * Triggered when a keyword gets matched
   * @param {Array<string>|string} match - regexp match or keyword arguments
   * @param index
   * @returns Array<QueryResultRow>|QueryResultRow
   */
  @abstract
  onKeyword (match, index) {

  }
}
