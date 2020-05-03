import { abstract } from '../utils/class';
import { incrementing } from '../utils/data';
import { EventEmitter } from 'events';

/**
 * A query result row
 * @typedef {{
 *    key: (string|number),
 *    title: string,
 *    icon: ?string,
 *    description: ?string,
 *    weight: ?number,
 *    pluginName: string,
 * }} QueryResultRow
 */

/**
 * Abstract plugin
 * @abstract
 */
@abstract
class AbstractPlugin {
  /**
   * Auto incrementing id for keeping track of plugins
   * @type {number}
   */
  id = incrementing();

  /**
   * plugin name
   * @type {string}
   * @abstract
   */
  @abstract
  name;

  /**
   *
   * @param query
   * @returns {Promise<Array<QueryResultRow>>}
   */
  @abstract
  async query (query) {
    return [];
  }

  /**
   * Select a query result row
   * @param {string|number} key
   * @returns {Promise<void>|void}
   * @abstract
   * @todo see plugin manager
   */
  @abstract
  select (key) {

  }
}

export default AbstractPlugin;
