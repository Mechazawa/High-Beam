// import AppIconFetcher from '../utils/AppIconFetcher';
import httpCodes from './http.json';
import KeywordPlugin, {Keyword} from "./interfaces/KeywordPlugin";
import {ResultCollection} from "./interfaces/Plugin";
import ActionBuilder from "../ActionBuilder";

export default class HttpCodePlugin extends KeywordPlugin {
  debounce = 10;

  name = 'httpcode';

  public keywords = [
    /^http\s*(\d*)/i,
  ];

  public keyword (keyword: Keyword, match: string | RegExpMatchArray): ResultCollection {
    const query = typeof match === 'string' ? match : match[1];
    const matches = httpCodes.filter(({ key }) => String(key).startsWith(query));

    return matches.map(({ key, title, description }) => ({
      title: `${key} - ${title}`,
      // icon: this.iconFetcher.icon,
      description,
      weight: Math.min(100 * (query.length / 3), 100),
      call: ActionBuilder.url(`https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/${key}`),
    }));
  }
}
