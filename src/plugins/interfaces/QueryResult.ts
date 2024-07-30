export default interface QueryResult<T = never> {
  call(meta: boolean): Promise<void> | void;

  title: string;
  icon?: string;
  description?: string;
  descriptionExtended?: string;
  weight?: number;
  html?: boolean;
  token?: string;
  meta?: T;
}