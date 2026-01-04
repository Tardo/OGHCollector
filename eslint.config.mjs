import js from '@eslint/js';
import prettierPlugin from 'eslint-plugin-prettier';
import prettierConfig from 'eslint-config-prettier';
import flowPlugin from 'eslint-plugin-ft-flow';
import globals from 'globals';
import babelParser from '@babel/eslint-parser';

export default [
  js.configs.recommended,
  {
    ignores: ["data/**"],
  },
  {
    files: ['web/js/**/*.mjs'],
    plugins: {
      prettier: prettierPlugin,
      'ft-flow': flowPlugin,
    },
    languageOptions: {
      parser: babelParser,
      sourceType: 'module',
      parserOptions: {
        sourceType: 'module',
        requireConfigFile: false,
        babelOptions: {
          babelrc: false,
          configFile: false,
          presets: ['@babel/preset-flow'],
        },
      },
      globals: {
        ...globals.browser,
        ...globals.es2024,
        jquery: false,
      },
    },
    rules: {
      ...flowPlugin.configs.recommended.rules,
      ...prettierConfig.rules,
      'prettier/prettier': 'error',
      eqeqeq: 'error',
      'no-empty-function': 'error',
      'no-eval': 'error',
      'no-implicit-coercion': 'error',
      'no-implicit-globals': 'off',
      'no-implied-eval': 'error',
      'no-return-assign': 'error',
      'no-undef-init': 'error',
      'no-shadow': 'error',
      'no-script-url': 'error',
      'no-unneeded-ternary': 'error',
      'no-unused-expressions': 'error',
      'no-labels': 'error',
      'no-useless-call': 'error',
      'no-useless-computed-key': 'error',
      'no-useless-concat': 'error',
      'no-useless-constructor': 'error',
      'no-useless-rename': 'error',
      'no-useless-return': 'error',
      'no-void': 'error',
      'no-unused-vars': [
        'error',
        {
          destructuredArrayIgnorePattern: '^_',
          varsIgnorePattern: '^_',
          caughtErrorsIgnorePattern: '^_',
        },
      ],
      'no-console': [
        'warn',
        {
          allow: ['info', 'warn', 'error'],
        },
      ],
      'prefer-const': 'error',
      'prefer-numeric-literals': 'error',
      'prefer-object-has-own': 'error',
      'spaced-comment': 'error',
      radix: 'error',
      'prefer-arrow-callback': 'warn',
      'no-var': 'warn',
      'no-extra-bind': 'warn',
      'no-lone-blocks': 'warn',
    },
  },

  {
    ignores: [
      'node_modules/**',
      'scripts/**',
      'static/**',
    ],
  },

  // {
  //   files: ['tests/**/*.mjs'],
  //   languageOptions: {
  //     globals: {
  //       ...globals.jest, // Entorno para Jest
  //     },
  //   },
  // },
];