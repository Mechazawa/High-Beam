# High Beam plugin SDK (TypeScript)

Ambient declarations for the `highbeam:*` modules. Use them to get
IntelliSense and type-checking when writing plugins in TypeScript.

## Usage

In your plugin directory:

```bash
plugins/my-plugin/
  manifest.json
  plugin.ts        # your source
  plugin.js        # compiled output (what manifest.entry points at)
  tsconfig.json
```

A minimal `tsconfig.json`:

```jsonc
{
    "compilerOptions": {
        "target": "ES2022",
        "module": "ES2022",
        "moduleResolution": "node",
        "strict": true,
        "noEmit": false,
        "outDir": ".",
        "paths": {
            "highbeam:*": ["./node_modules/@high-beam/sdk/*"]
        }
    },
    "include": ["plugin.ts"]
}
```

…and `npm install --save-dev <path-to-this-sdk-dir>` (or symlink it).

The compiled JS your plugin actually ships should keep the bare
`import { … } from 'highbeam:actions'` specifiers — `highbeam:*` is what
the host loader resolves at runtime. TypeScript only needs the `.d.ts`
files to know what those modules export at compile time; nothing from
`@high-beam/sdk` lands in the runtime bundle.

## What's included

| Module                | Purpose                                                | Capability                              |
| --------------------- | ------------------------------------------------------ | --------------------------------------- |
| `highbeam:actions`    | Action builders (`openUrl`, `copy`, `exec`, …)         | `actions`                               |
| `highbeam:clipboard`  | Read/write the system clipboard                        | `clipboard.read` / `clipboard.write`    |
| `highbeam:fs`         | Walk dirs, read files, plugin-scoped cache             | `fs.read` / `fs.cache`                  |
| `highbeam:icons`      | Native data-URI icons                                  | `icons`                                 |
| `highbeam:match`      | Fuzzy ranking with highlight ranges                    | —                                       |
| `highbeam:system`     | Subprocess + AppleScript escape hatches                | `system.exec` / `system.applescript`    |
| `highbeam:platform`   | OS / arch / version metadata                           | —                                       |
| `node:path`           | Node-style path helpers (llrt)                         | —                                       |
| `node:fs` (+`/promises`) | Full Node-style filesystem access (llrt)            | `fs`                                    |

HTTP is the global `fetch` (llrt), gated on the `http` capability — there
is no `highbeam:http` module. `URL`, `URLSearchParams`, `Buffer`, `Blob`,
`TextEncoder`/`TextDecoder`, `AbortController`/`AbortSignal`, and
`DOMException` are always-on globals.

The `types.d.ts` file has the shared shapes (`Result`, `Action`,
`AbortSignal`) every module uses. For `node:*` module types, add
`@types/node` to your plugin's devDependencies.

## Drift check

The host has a CI test that loads each module into a real rquickjs context
and asserts that the symbols it exports match an expected list. If you
change a `.d.ts` you have to update that test (and vice versa). See
`tests/sdk_shape.rs` in the host repo.
