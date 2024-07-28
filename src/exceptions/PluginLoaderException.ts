export default class PluginLoaderException extends Error {
  public previous?: Error;

  constructor(message: string, previous?: Error) {
    super(message);

    this.previous = previous;
  }
}