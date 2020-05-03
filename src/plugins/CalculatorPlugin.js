import AbstractKeywordPlugin from './AbstractKeywordPlugin';
import clipboardy from 'clipboardy';
import fileIcon from 'file-icon';
import { evaluate } from 'mathjs';

export default class CalculatorPlugin extends AbstractKeywordPlugin {
  name = 'calculator';

  keywords = [
    /^=(.*)/,
    /^\s*(?:convert\s*)?([\d,]+\s+\w+.*)/,
  ];

  iconPath = '/System/Applications/Calculator.app';

  icon;

  constructor () {
    super();

    this._fetchIcon();
  }

  select (key) {
    console.log('select', key);

    if (key !== null) {
      clipboardy.writeSync(key);
    }
  }

  keyword ([, query], index) {
    switch (index) {
      case 0:
        return this.calculate(query);
      case 1:
        return this.convert(query);
    }
  }

  calculate (query) {
    try {
      const result = evaluate(query.trim() || '0');

      return [{
        icon: this.icon,
        title: String(result),
        key: String(result),
        pluginName: this.name,
        weight: 100,
      }];
    } catch {
      return [{
        icon: this.icon,
        title: 'Error',
        key: null,
        pluginName: this.name,
        weight: 100,
      }];
    }
  }

  convert (query) {
    try {
      const result = evaluate(query.trim());

      return [{
        icon: this.icon,
        title: String(result),
        key: String(result),
        pluginName: this.name,
        weight: 100,
      }];
    } catch {
      return [];
    }
  }

  async _fetchIcon () {
    const iconBuffer = await fileIcon.buffer(this.iconPath, { size: 128 });

    this.icon = `data:image/png;base64,${iconBuffer.toString('base64')}`;
  }
}
