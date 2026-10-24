import { createRequire } from 'module';
import terser from '@rollup/plugin-terser';

const require = createRequire(import.meta.url);
const hljsVersion = require('highlight.js/package.json').version;
const banner = `/*! \`veryl\` grammar built and tested with Highlight.js ${hljsVersion} */\n`;
const registerLanguage = 'hljs.registerLanguage("veryl", hljsVeryl);';

export default {
  input: 'src/languages/veryl.js',
  output: [
    {
      file: 'dist/veryl.es.js',
      format: 'es',
      banner
    },
    {
      file: 'dist/veryl.es.min.js',
      format: 'es',
      banner,
      plugins: [ terser() ]
    },
    {
      file: 'dist/veryl.cjs',
      format: 'cjs',
      exports: 'default',
      banner
    },
    {
      file: 'dist/veryl.js',
      format: 'iife',
      name: 'hljsVeryl',
      banner,
      footer: registerLanguage
    },
    {
      file: 'dist/veryl.min.js',
      format: 'iife',
      name: 'hljsVeryl',
      banner,
      footer: registerLanguage,
      plugins: [ terser() ]
    }
  ]
};
