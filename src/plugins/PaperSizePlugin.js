import AbstractKeywordPlugin from './AbstractKeywordPlugin';
import clipboardy from 'clipboardy';
import paperSizes from './paper-sizes.json';
import iconPath from '../assets/PaperSizePlugin.png';
import { readFileSync } from 'fs';

console.log(`${__dirname}/${iconPath}`);

export default class PaperSizePlugin extends AbstractKeywordPlugin {
  name = 'paper-size';

  keywords = [
    /^\s*(\w+)\s*$/,
    /^\s*paper\s*(\w+)\s*$/i,
  ];

  static icon = `data:image/png;base64,${readFileSync(`${__dirname}/${iconPath}`).toString('base64')}`;

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
      icon: PaperSizePlugin.icon,
      key,
      title: key,
      description: `${paperSizes[key].mm} mm`,
      weight: 100 * (query.length / key.length),
    }));
  }
}
