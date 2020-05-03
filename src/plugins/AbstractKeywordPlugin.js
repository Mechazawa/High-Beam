import { abstract } from '../utils/class';
import AbstractPlugin from './AbstractPlugin';

/**
 * @abstract
 */
export default class AbstractKeywordPlugin extends AbstractPlugin {
  /**
   * List of keywords
   * @type {Array<RegExp|string>}
   * @abstract
   */
  @abstract
  keywords = [
    'http',
  ];

  /**
   * @inheritDoc
   */
  async query (query) {
    const output = [];

    for (let i = 0; i < this.keywords.length; i++) {
      const keyword = this.keywords[i];

      if (typeof keyword === 'string') {
        if (query.startsWith(`${keyword} `)) {
          const args = query.replace(new RegExp(`^${keyword}\\s+`), '');

          output.push(this.keyword(args, i));
        }
      } else {
        const match = query.match(keyword);

        if (match) {
          output.push(this.keyword(match, i));
        }
      }
    }

    return (await Promise.all(output)).flat(1);
  }

  /**
   * Triggered when a keyword gets matched
   * @param {Array<string>|string} match - regexp match or keyword arguments
   * @param index
   * @returns {Promise<Array<QueryResultRow>>}
   * @abstract
   */
  @abstract
  keyword (match, index) {

  }
}
