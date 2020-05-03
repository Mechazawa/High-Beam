import AbstractPlugin from './AbstractPlugin';
import osApps from 'os-apps';
import fileIcon from 'file-icon';
import { basename, extname } from 'path';
import { exec } from 'child_process';

export default class SpotlightPlugin extends AbstractPlugin {
  name = 'spotlight';

  async query (query) {
    if (!query.length) {
      return [];
    }

    const apps = (await osApps.getAll()).map(path => ({
      appName: basename(path, extname(path)),
      path,
    })).filter(({ appName }) => appName.startsWith(query));

    return Promise.all(apps.map(async ({ path, appName }) => {
      const iconBuffer = await fileIcon.buffer(path, { size: 128 });
      const icon = `data:image/png;base64,${iconBuffer.toString('base64')}`;

      return {
        icon,
        title: appName,
        description: path,
        key: path,
        pluginName: this.name,
      };
    }));
  }

  select (key) {
    exec('open ' + key);
  }
}
