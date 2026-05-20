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

| Module                | Purpose                                         | Capability                              |
| --------------------- | ----------------------------------------------- | --------------------------------------- |
| `highbeam:actions`    | Action builders (`openUrl`, `copy`, `exec`, …) | `actions`                               |
| `highbeam:http`       | Async HTTP client (`get`, `post`)               | `http`                                  |
| `highbeam:clipboard`  | Read/write the system clipboard                 | `clipboard.read` / `clipboard.write`    |

The `types.d.ts` file has the shared shapes (`Result`, `Action`,
`AbortSignal`, `HttpResponse`) every module uses.

## Drift check

The host has a CI test that loads each module into a real rquickjs context
and asserts that the symbols it exports match an expected list. If you
change a `.d.ts` you have to update that test (and vice versa). See
`tests/sdk_shape.rs` in the host repo.
