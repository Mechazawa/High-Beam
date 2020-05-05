import AbstractKeywordPlugin from './AbstractKeywordPlugin';
import clipboardy from 'clipboardy';
import paperSizes from './paper-sizes.json';
import AppIconFetcher from "../utils/AppIconFetcher";

export default class PaperSizePlugin extends AbstractKeywordPlugin {
  name = 'paper-size';

  keywords = [
    /^\s*(\w+)\s*$/,
    /^\s*paper\s*(\w+)\s*$/i,
  ];

  iconFetcher = new AppIconFetcher('/Applications/Pages.app');

  select (key) {
    if (paperSizes[key]) {
      clipboardy.writeSync(paperSizes[key].mm);
    }
  }

  keyword ([, query], index) {
    query = query.toLowerCase();

    const matches = Object.keys(paperSizes)
      .filter(key => key.toLowerCase().includes(query));

    return matches.map(key => ({
      icon: this.iconFetcher.icon,
      key,
      title: key,
      description: `${paperSizes[key].mm} mm`,
      weight: 100 * (query.length / key.length),
    }));
  }
}
