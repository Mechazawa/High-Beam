import QueryResult from "./QueryResult";

export type ResultCollection = QueryResult[] | Promise<QueryResult[]>;

export default abstract class Plugin {
  /**
   * plugin name
   */
  public name: string;

  /**
   * Debounce for query calls
   */
  public debounce: number | undefined | null;

  /**
   * Called when the user queries the launcher
   * @param query Launcher query
   */
  public abstract query(query: string): ResultCollection
}
