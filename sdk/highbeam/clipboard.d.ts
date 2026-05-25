// `highbeam:clipboard` — read/write the system clipboard.
//
// The module loads if you declare *either* `clipboard.read` or
// `clipboard.write` in your manifest. Each function additionally gates
// itself on its specific capability — calling `write()` from a plugin that
// only declared `clipboard.read` throws a `CapabilityError`.

/** Read the current clipboard text. Requires the `clipboard.read` capability. */
export function read(): Promise<string>;

/** Set the clipboard text. Requires the `clipboard.write` capability. */
export function write(text: string): Promise<void>;
