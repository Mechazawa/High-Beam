import Plugin, {ResultCollection} from './Plugin';
import {PluginQueryResult} from "./QueryResult";

export type Keyword = RegExp | string;

export default abstract class KeywordPlugin extends Plugin {
  /**
   * List of keywords
   */
  public abstract keywords: Keyword[];

  /**
   * @inheritDoc
   */
  public async query(query: string): Promise<PluginQueryResult[]> {
    const output = [];

    for (const keyword of this.keywords) {
      if (typeof keyword === 'string') {
        const keywordRe = new RegExp(`^${keyword}(?:\\s+(.*)|\\s*)$`, 'i');
        const match = query.match(keywordRe);

        if (match) {
          output.push(this.keyword(keyword, match[1] ?? ''));
        }
      } else {
        const match = query.match(keyword);

        if (match) {
          output.push(this.keyword(keyword, match));
        }
      }
    }

    return (await Promise.all(output)).flat(1);
  }

  /**
   * Triggered when a keyword gets matched
   */
  public abstract keyword(keyword: Keyword, match: string | RegExpMatchArray): ResultCollection;
}
