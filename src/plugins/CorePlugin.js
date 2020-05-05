import AbstractKeywordPlugin from "./AbstractKeywordPlugin";
import { exec } from "child_process";
import AppIconFetcher from "../utils/AppIconFetcher";

export class CorePlugin extends AbstractKeywordPlugin {
  debounce = 0;

  name = 'core';

  keywords = [];

  actions = {
    'exit': () => process.exit(),
    'shutdown': () => exec(`osascript -e 'tell application "Finder" to shut down'`),
    'sleep': () => exec(`osascript -e 'tell application "Finder" to sleep'`),
    'restart': () => exec(`osascript -e 'tell application "Finder" to restart'`),
    'lock': () => exec(`osascript -e 'tell application "System Events" to keystroke "q" using {control down, command down}'`),
  };

  iconFetcher = new AppIconFetcher('/System/Applications/System Preferences.app');

  constructor () {
    super();

    this.keywords.push(...Object.keys(this.actions));
  }

  keyword (query, index, length) {
    const keyword = this.keywords[index];

    if (this.actions.hasOwnProperty(keyword)) {
      return [{
        icon: this.iconFetcher.icon,
        key: keyword,
        title: keyword,
        weight: 100 * (keyword.length / length),
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