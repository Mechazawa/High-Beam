import AbstractKeywordPlugin from './AbstractKeywordPlugin';
import AppIconFetcher from "../utils/AppIconFetcher";
import httpCodes from "../assets/http.json";
import { exec } from 'child_process';

export default class HttpCodePlugin extends AbstractKeywordPlugin {
  debounce = 10;

  name = 'httpcode';

  keywords = [
    /^http\s*(\d*)/i,
  ];

  iconFetcher = new AppIconFetcher('/System/Library/CoreServices/Applications/Network Utility.app');

  select (key) {
    if (key !== null) {
      exec("open https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/" + key);
    }
  }

  keyword ([, query]) {
    const matches = httpCodes.filter(({ key }) => String(key).startsWith(query));

    return matches.map(({ key, title, description }) => ({
      key, title: `${key} - ${title}`, description,
      icon: this.iconFetcher.icon,
      pluginName: this.name,
      weight: Math.min(100 * (query.length / 3), 100),
    }));
  }
}
