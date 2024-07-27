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
        const keywordRe = new RegExp(`^(${keyword.replace(/(.)/g, '$1?')}$|keyword)`, 'i');
        const match = query.match(keywordRe);

        if (match) {
          const args = query.replace(new RegExp(`^${keyword}\\s+`), '');

          output.push(this.keyword(args, i, match[0].length));
        }
      } else {
        const match = query.match(keyword);

        if (match) {
          output.push(this.keyword(match, i, match[0].length));
        }
      }
    }

    return (await Promise.all(output)).flat(1);
  }

  /**
   * Triggered when a keyword gets matched
   * @param {Array<string>|string} match - regexp match or keyword arguments
   * @param {number} index - matched keyword index
   * @param {number} length - match length
   * @returns {Promise<Array<QueryResultRow>>|Array<QueryResultRow>}
   * @abstract
   */
  @abstract
  keyword (match, index, length) {

  }
}
