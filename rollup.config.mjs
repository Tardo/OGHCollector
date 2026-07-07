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
      'web/js/pages/doodba_tools/converter.mjs',
      'web/js/pages/doodba_tools/dependency-resolver.mjs',
      'web/js/pages/doodba_tools/migration-plan.mjs',
      'web/js/pages/api-doc.mjs',
      'web/js/pages/mcp-info.mjs',
      'web/js/pages/atlas.mjs',
      'web/js/pages/logs.mjs',
      'web/js/pages/module.mjs',
      'web/js/pages/committer.mjs',
      'web/js/pages/committers.mjs',
    ],
    output: {
      sourcemap: (!is_production && 'inline') || false,
      format: 'esm',
      dir: 'static/auto/',
      entryFileNames: '[name].mjs',
      // Stable (unhashed) names for chunks we modulepreload from templates
      // (see minimal_layout.html / dashboard.html), so the browser can fetch
      // them in parallel with the entry script instead of discovering them
      // only after it's parsed.
      chunkFileNames: chunk =>
        ['mirlo', 'module-search'].includes(chunk.name)
          ? '[name].mjs'
          // content-hashed chunks live under chunks/ so the server can cache
          // that folder forever without risking stale unhashed files (see
          // crates/server/src/main.rs)
          : 'chunks/[name]-[hash].mjs',
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