import fileIcon from 'file-icon';

export default class AppIconFetcher {
  /**
   * Path to the app
   * @type {string}
   */
  appPath;

  /**
   * Icon size
   * @type {number}
   */
  size;

  /**
   * Icon image
   * Defaults to transparent pixel
   * @type {string}
   */
  icon = 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7';

  /**
   * Start fetching an app icon
   * @param {string} appPath - path to the app
   * @param {number} size - icon size
   */
  constructor (appPath, size = 128) {
    this.appPath = appPath;
    this.size = size;

    // noinspection JSIgnoredPromiseFromCall
    this.refresh();
  }

  get ready () {
    return Boolean(this.icon);
  }

  async refresh () {
    this.icon = '';

    const iconBuffer = await fileIcon.buffer(this.appPath, { size: this.size });

    this.icon = `data:image/png;base64,${iconBuffer.toString('base64')}`;
  }
}
