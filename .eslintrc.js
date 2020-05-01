module.exports = {
  extends: [
    '@mapcreator',
  ],
  root: true,
  env: {
    node: true,
  },
  parserOptions: {
    parser: 'babel-eslint',
  },
  rules: {
    'no-console': process.env.NODE_ENV === 'production' ? 'warn' : 'off',
    'no-debugger': process.env.NODE_ENV === 'production' ? 'warn' : 'off',
    'prefer-destructuring': 'off',
    'max-depth': 'off',
    'max-params': 'off',
    'max-len': 'off',
    'max-lines': 'off',
    'max-lines-per-function': 'off',
    'no-mixed-operators': 'off',
    'no-unused-vars': 'off',
    'camelcase': 'off',
  },
  overrides: [
    {
      files: [
        '**/__tests__/*.{j,t}s?(x)',
        '**/tests/unit/**/*.spec.{j,t}s?(x)',
      ],
      env: {
        jest: true,
      },
    },
  ],
};
