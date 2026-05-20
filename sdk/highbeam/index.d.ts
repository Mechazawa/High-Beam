// Barrel that re-declares each `highbeam:*` module as an ambient TypeScript
// module. Plugin authors point `tsconfig.json`'s `paths` at this file so
// `import { copy } from 'highbeam:actions'` resolves and provides IntelliSense.
//
// See README.md in this directory for the tsconfig recipe.

declare module 'highbeam:actions' {
    export * from './actions';
}

declare module 'highbeam:http' {
    export * from './http';
}

declare module 'highbeam:clipboard' {
    export * from './clipboard';
}

export * from './types';
