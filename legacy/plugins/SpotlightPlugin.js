import AbstractPlugin from './AbstractPlugin';
import fileIcon from 'file-icon';
import { basename, extname } from 'path';
import { exec } from 'child_process';
import Fuse from 'fuse.js';
import { highlightFuseMatches } from '../utils/data';
import { dirname } from 'path';
import { getFiles } from "../utils/fs";

export default class SpotlightPlugin extends AbstractPlugin {
  name = 'spotlight';

  debounce = 100;

  iconCache = new Map();

  appDirectories = [
    '/System/Library/CoreServices',
    '/Applications',
    // '~/Applications',
    '/System/Applications',
  ];

  async getIcon (path) {
    if (this.iconCache.has(path)) {
      return this.iconCache.get(path);
    }

    const iconBuffer = await fileIcon.buffer(path, { size: 128 });
    const icon = `data:image/png;base64,${iconBuffer.toString('base64')}`;

    this.iconCache.set(path, icon);

    return icon;
  }

  async getAppsInPath (path, ext = ['.app']) {
    const isApp = str => ext.some(e => str.endsWith(e));
    const output = [];

    for await (const f of getFiles(path)) {
      if (isApp(f)) {
        output.push(f);
      }
    }

    return output;
  }

  async query (query) {
    query = query.trim().toLowerCase();

    if (!query.length) {
      return [];
    }

    const apps = (
      await Promise.all(this.appDirectories
                            .map(dir => this.getAppsInPath(dir)))
    ).flat(1).map(path => ({
      path, appName: basename(path, extname(path)),
    }));

    const matches = new Fuse(apps, {
      includeScore: true,
      includeMatches: true,
      keys: ['appName'],
    }).search(query).filter(({ score }) => score <= 0.7);

    matches.sort((a, b) => (1e9 * a.score) - (1e9 * b.score));
    matches.splice(10, matches.length);

    return Promise.all(matches.map(async match => {
      const { path } = match.item;
      const highlighted = highlightFuseMatches(match.matches);

      return {
        icon: await this.getIcon(path),
        title: highlighted.appName,
        description: path,
        descriptionExtended: 'Open in finder',
        key: path,
        pluginName: this.name,
        html: true,
        weight: 100 - (match.score * 100),
      };
    }));
  }

  select (key, meta) {
    if (meta) {
      const path = JSON.stringify(dirname(key));

      exec(`open /System/Library/CoreServices/Finder.app ${path}`);
    } else {
      const path = JSON.stringify(key);

      exec(`open ${key}`);
    }
  }
}
