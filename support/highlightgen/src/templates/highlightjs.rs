use crate::templates::Template;
use handlebars::Handlebars;
use std::path::PathBuf;

// `className` is used instead of the newer `scope` key on purpose: `scope` was
// introduced in highlight.js v11, and mdbook still bundles v10, which would
// silently drop the highlighting of the mode. v11 maps `className` to `scope`.
const TMPL: &str = r###"/*
Language: Veryl
Author: Naoya Hatta <dalance@gmail.com>
Description: Veryl is a modern hardware description language which transpiles to SystemVerilog.
Website: https://veryl-lang.org
Category: hardware
*/

export default function (hljs)
{
  return {
    name: 'Veryl',
    aliases: [
        'veryl'
    ],
    case_insensitive: false,
    keywords:
      {
        keyword: '{{#each this}}{{#each this}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}{{#unless @last}} {{/unless}}{{/each}}',
        literal: ''
      },
    contains:
      [
        hljs.QUOTE_STRING_MODE,
        hljs.C_BLOCK_COMMENT_MODE,
        hljs.C_LINE_COMMENT_MODE,
        {
          className: 'number',
          contains: [ hljs.BACKSLASH_ESCAPE ],
          variants: [
            { begin: /\b((\d+'([bhodBHOD]))[0-9xzXZa-fA-F_]+)/ },
            { begin: /\B(('([bhodBHOD]))[0-9xzXZa-fA-F_]+)/ },
            { // decimal
              begin: /\b[0-9][0-9_]*/,
              relevance: 0
            }
          ]
        }
      ]
  }
}
"###;

pub struct Highlightjs;

impl Template for Highlightjs {
    fn apply(&self, keywords: &crate::keywords::Keywords) -> String {
        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(TMPL, &keywords).unwrap()
    }

    fn path(&self) -> PathBuf {
        PathBuf::from("support/highlightjs/src/languages/veryl.js")
    }
}
