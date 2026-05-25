# echo-ts

TypeScript variant of the `echo` plugin. The host only ever loads
`plugin.js` — `plugin.ts` is the source you edit, and `tsc` produces the
JS the host runs.

## Build

```sh
npm install
npm run build   # tsc → regenerates plugin.js
```

`npm install` runs `tsc` as a `prepare` step, so the first install is
enough on a fresh clone. Re-run `npm run build` after editing `plugin.ts`.

## Test

```sh
npm test
```

Vitest runs against `plugin.js` (the same file the host loads) so the
plugin author and the host see the same artifact.
