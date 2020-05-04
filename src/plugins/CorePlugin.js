import AbstractKeywordPlugin from "./AbstractKeywordPlugin";

export class CorePlugin extends AbstractKeywordPlugin {
  debounce = 10;
  name = 'core';

  keywords = [];

  actions = {
    'exit': () => process.exit(),
  };

  constructor () {
    super();

    this.keywords.push(...Object.keys(this.actions));
  }

  keyword (query, index) {
    const keyword = this.keywords[index];

    if (this.actions.hasOwnProperty(keyword)) {
      return [{
        key: keyword,
        title: keyword,
        weight: 100,
        pluginName: this.name,
      }];
    }

    return [];
  }

  select (key) {
    if (this.actions.hasOwnProperty(key)) {
      this.actions[key].apply(this);
    }
  }
}