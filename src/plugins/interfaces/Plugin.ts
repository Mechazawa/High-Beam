import QueryResultRow from "./QueryResultRow";

export type QueryResult = QueryResultRow[] | Promise<QueryResultRow[]>;

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
  public abstract query(query: string): QueryResult
}
