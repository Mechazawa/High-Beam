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
 *    html: ?boolean,
 * }} QueryResultRow
 */

/**
 * Abstract plugin
 * @abstract
 */
@abstract
class AbstractPlugin {
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
