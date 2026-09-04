# highlightjs-veryl

[Veryl](https://veryl-lang.org) syntax highlighting for [highlight.js](https://highlightjs.org).

`src/languages/veryl.js` is generated from the Veryl grammar by
[highlightgen](../highlightgen). Edit the template in
`support/highlightgen/src/templates/highlightjs.rs` instead of the grammar file
itself.

## Usage

### Static website

Load the grammar after highlight.js. The CDN build registers itself, so no extra
call is required.

```html
<script type="text/javascript" src="/path/to/highlight.min.js"></script>
<script type="text/javascript" src="/path/to/veryl.min.js"></script>
<script type="text/javascript">
  hljs.highlightAll();
</script>
```

### Directly from a CDN

```html
<script type="text/javascript" src="https://cdn.jsdelivr.net/npm/highlight.js@11/lib/highlight.min.js"></script>
<script type="text/javascript" src="https://cdn.jsdelivr.net/npm/highlightjs-veryl/dist/veryl.min.js"></script>
```

[unpkg](https://unpkg.com) serves the same file at
`https://unpkg.com/highlightjs-veryl/dist/veryl.min.js`.

### Node or a bundler

ESM:

```javascript
import hljs from 'highlight.js';
import veryl from 'highlightjs-veryl';

hljs.registerLanguage('veryl', veryl);
hljs.highlightAll();
```

CommonJS:

```javascript
const hljs = require('highlight.js');
const veryl = require('highlightjs-veryl');

hljs.registerLanguage('veryl', veryl);
hljs.highlightAll();
```

## Build

```bash
npm install
npm run build   # writes dist/
npm test        # builds, then runs the markup tests
```

`npm run build` writes the following files to `dist`:

| File | Format |
|---|---|
| `veryl.js` / `veryl.min.js` | CDN build; calls `hljs.registerLanguage` itself |
| `veryl.es.js` / `veryl.es.min.js` | ESM |
| `veryl.cjs` | CommonJS |

## License

MIT. See [LICENSE](LICENSE).
