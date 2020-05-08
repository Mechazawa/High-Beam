import AbstractPlugin from './AbstractPlugin';
import osApps from 'os-apps';
import fileIcon from 'file-icon';
import { basename, extname } from 'path';
import { exec } from 'child_process';
import Fuse from 'fuse.js';
import { highlightFuseMatches } from "../utils/data";

export default class SpotlightPlugin extends AbstractPlugin {
  name = 'spotlight';

  debounce = 100;

  iconCache = new Map();

  async getIcon (path) {
    if(this.iconCache.has(path)) {
      return this.iconCache.get(path);
    }

    const iconBuffer = await fileIcon.buffer(path, { size: 128 });
    const icon = `data:image/png;base64,${iconBuffer.toString('base64')}`;

    this.iconCache.set(path, icon);

    return icon;
  }

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
    }).search(query).filter(({ score }) => score <= .7);

    matches.sort((a, b) => (1e9 * a.score) - (1e9 * b.score));
    matches.splice(10, matches.length);

    return Promise.all(matches.map(async match => {
      const { path } = match.item;
      const highlighted = highlightFuseMatches(match.matches);

      return {
        icon: await this.getIcon(path),
        title: highlighted.appName,
        description: path,
        key: path,
        pluginName: this.name,
        html: true,
        weight: 100 - (match.score * 100),
      };
    }));
  }

  select (key) {
    exec(`open ${key}`);
  }
}
