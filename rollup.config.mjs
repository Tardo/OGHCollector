/* eslint-disable */
import alias from '@rollup/plugin-alias';
import {nodeResolve} from '@rollup/plugin-node-resolve';
import commonjs from "@rollup/plugin-commonjs";
import terser from '@rollup/plugin-terser';
import autoprefixer from 'autoprefixer';
import cssnano from 'cssnano';
import path from 'path';
import analyze from 'rollup-plugin-analyzer';
import {libStylePlugin} from 'rollup-plugin-lib-style';

const is_production = process.env.NODE_ENV === 'production';

export default [
  {
    input: [
      'web/js/main.mjs',
      'web/js/pages/dashboard.mjs',
      'web/js/pages/osv.mjs',
      'web/js/pages/doodba-converter.mjs',
      'web/js/pages/api-doc.mjs',
      'web/js/pages/atlas.mjs',
      'web/js/pages/logs.mjs',
    ],
    output: {
      sourcemap: (!is_production && 'inline') || false,
      format: 'esm',
      dir: 'static/auto/',
      entryFileNames: '[name].mjs',
      chunkFileNames: '[name]-[hash].mjs',
    },
    plugins: [
      alias({
        entries: [
          {
            find: '@app',
            replacement: path.resolve('web/js'),
          },
          {
            find: '@scss',
            replacement: path.resolve('web/scss'),
          },
        ],
      }),
      nodeResolve({
        preferBuiltins: false,
        browser: true,
      }),
      commonjs({
        include: /node_modules/,
      }),

      libStylePlugin({
        importCSS: false,
        scopedName: '[local]',
        customPath: './web',
        postCssPlugins: is_production && [autoprefixer(), cssnano()] || [autoprefixer()],
      }),

      is_production && terser(),
      is_production && analyze(),
    ],
    watch: {
      clearScreen: false,
      include: [
        'web/js/**',
        'web/scss/**',
      ],
    },
  },
];