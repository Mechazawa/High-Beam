module.exports = {
  presets: [
    '@vue/cli-plugin-babel/preset',
    // '@babel/preset-env',
  ],
  plugins: [
    '@babel/plugin-proposal-optional-chaining',

    // Stage 2
    // ["@babel/plugin-proposal-decorators", { decoratorsBeforeExport: true }],
    '@babel/plugin-proposal-export-namespace-from',
    '@babel/plugin-proposal-numeric-separator',
    '@babel/plugin-proposal-throw-expressions',

    // Stage 3
    '@babel/plugin-syntax-dynamic-import',
    '@babel/plugin-syntax-import-meta',
    // '@babel/plugin-proposal-class-properties',
    '@babel/plugin-proposal-json-strings',
  ],
};
