import AbstractPlugin from './AbstractPlugin';
import osApps from 'os-apps';
import fileIcon from 'file-icon';
import { basename, extname } from 'path';
import { exec } from 'child_process';
import Fuse from 'fuse.js';

export default class SpotlightPlugin extends AbstractPlugin {
  name = 'spotlight';

  debounce = 100;

  async query (query) {
    query = query.trim().toLowerCase();

    if (!query.length) {
      return [];
    }

    // @todo improve the performance and get better way to find os apps
    const apps = (await osApps.getAll()).map(path => ({
      path, appName: basename(path, extname(path)),
    }));

    const matches = new Fuse(apps, {
      includeScore: true,
      includeMatches: true,
      keys: ['appName'],
    }).search(query).filter(({score}) => score <= .7);

    matches.sort((a, b) => (1e9 * a.score) - (1e9 * b.score));
    matches.splice(10, matches.length);

    return Promise.all(matches.map(async match => {
      const { path } = match.item;
      const iconBuffer = await fileIcon.buffer(path, { size: 128 });
      const icon = `data:image/png;base64,${iconBuffer.toString('base64')}`;
      const highlighted = this._highlightMatches(match.matches);

      return {
        icon,
        title: highlighted.appName,
        description: path,
        key: path,
        pluginName: this.name,
        html: true,
        weight: 100 - (match.score * 100),
      };
    }));
  }

  _highlightMatches (matches) {
    const output = {};
    const insert = (strA, idx, strB) => strA.slice(0, idx) + strB + strA.slice(idx);

    for (const match of matches) {
      let str = match.value;

      for (let i = match.indices.length - 1; i >= 0; i--) {
        str = insert(str, match.indices[i][1] + 1, '</b>');
        str = insert(str, match.indices[i][0], '<b style="font-weight: bolder;">');
      }

      output[match.key] = str;
    }

    return output;
  }

  select (key) {
    exec(`open ${key}`);
  }
}
