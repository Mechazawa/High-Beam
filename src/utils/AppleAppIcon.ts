import fileIcon from 'file-icon';

export default class AppleAppIcon {
  private static DEFAULT_ICON = 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7';

  /**
   * Path to the app
   * @type {string}
   */
  public readonly appPath: string;

  /**
   * Icon size
   * @type {number}
   */
  public readonly size: number;

  /**
   * Icon image
   * Defaults to transparent pixel
   * @type {string}
   */
  public icon: string;

  /**
   * Start fetching an app icon
   * @param appPath path to the app
   * @param size icon size
   */
  constructor (appPath: string, size = 128) {
    this.appPath = appPath;
    this.size = size;
    this.icon = AppleAppIcon.DEFAULT_ICON;

    this.refresh().then(() => null);
  }

  /**
   * If the app icon has been loaded
   */
  public get ready (): boolean {
    return this.icon !== AppleAppIcon.DEFAULT_ICON;
  }

  /**
   * Refresh app icon
   */
  public async refresh (): Promise<string> {
    if (process.platform !== 'darwin') {
      return;
    }

    this.icon = '';

    const iconBuffer = await fileIcon.buffer(this.appPath, { size: this.size });

    this.icon = `data:image/png;base64,${iconBuffer.toString('base64')}`;

    return this.icon;
  }
}
