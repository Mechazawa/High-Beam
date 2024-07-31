import {PluginQueryResult} from "./plugins/interfaces/QueryResult";

export default class ActionBuilder {
  static url(url: string): PluginQueryResult['call'] {
    return async (meta: boolean) => {
      if (meta) {
        this.copy(url)(false);
        return;
      }

      const {default: open} = await import('open');

      await open(url, {
        app: {name: 'browser'},
        wait: false,
      });
    };
  }

  static copy(text: string): PluginQueryResult['call'] {
    return async (meta: boolean) => {
      if (meta) return;

      const {default: {write}} = await import('clipboardy');

      await write(text);
    }
  }
}