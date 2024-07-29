// import AppIconFetcher from '../utils/AppIconFetcher';
import httpCodes from './http.json';
import { exec } from 'child_process';
import KeywordPlugin, {Keyword} from "./interfaces/KeywordPlugin";
import QueryResult from "./interfaces/QueryResult";

export default class HttpCodePlugin extends KeywordPlugin {
  debounce = 10;

  name = 'httpcode';

  public keywords = [
    /^http\s*(\d*)/i,
  ];

  // iconFetcher = new AppIconFetcher('/System/Library/CoreServices/Applications/Network Utility.app');

  public keyword (keyword: Keyword, match: string | RegExpMatchArray): QueryResult[] {
    const query = typeof match === 'string' ? match : match[1];
    const matches = httpCodes.filter(({ key }) => String(key).startsWith(query));

    return matches.map(({ key, title, description }) => ({
      title: `${key} - ${title}`,
      // icon: this.iconFetcher.icon,
      description,
      weight: Math.min(100 * (query.length / 3), 100),
      call: () => {exec(`open https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/${key}`);},
    }));
  }
}
