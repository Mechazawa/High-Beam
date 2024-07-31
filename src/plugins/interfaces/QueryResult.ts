export default interface QueryResult {
  title: string;
  icon?: string;
  description?: string;
  descriptionAlt?: string;
  weight?: number;
  html?: boolean;
}

export interface PluginQueryResult extends QueryResult {
  call(meta: boolean): unknown;
}

export interface DisplayQueryResult extends QueryResult {
  token?: string;
}