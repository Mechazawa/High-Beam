// Ambient declarations for each `highbeam:*` module. Plugin authors point
// `tsconfig.json`'s `paths` at this file. See README in this directory for
// the tsconfig recipe.

declare module 'highbeam:actions' {
    export * from './actions';
}

declare module 'highbeam:clipboard' {
    export * from './clipboard';
}

declare module 'highbeam:fs' {
    export * from './fs';
}

declare module 'highbeam:icons' {
    export * from './icons';
}

declare module 'highbeam:match' {
    export * from './match';
}

declare module 'highbeam:system' {
    export * from './system';
}

declare module 'highbeam:platform' {
    export * from './platform';
}

declare module 'highbeam:settings' {
    export * from './settings';
}

declare module 'highbeam:view' {
    export * from './view';
}

export * from './types';
