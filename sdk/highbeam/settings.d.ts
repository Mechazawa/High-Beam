// `highbeam:settings` — read this plugin's own option values, scoped
// internally by the host so `get('foo')` returns this plugin's `foo`, not
// another plugin's. No capability required.

/**
 * Look up an option by `key`. Returns the stored value (string/bool/int/enum)
 * or `undefined` when the plugin's manifest didn't declare the key, the user
 * never set it, or the host has no per-plugin bag installed (e.g. in some
 * unit-test contexts).
 */
export function get<T = unknown>(key: string): T | undefined;

/** Same as `get`, but only returns a value when it's a string. */
export function getString(key: string): string | undefined;

/** Same as `get`, but only returns a value when it's a boolean. */
export function getBool(key: string): boolean | undefined;

/** Same as `get`, but only returns a value when it's a number (`int` manifest type). */
export function getInt(key: string): number | undefined;
