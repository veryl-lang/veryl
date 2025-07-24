use crate::templates::Template;
use handlebars::Handlebars;
use std::path::PathBuf;

const TMPL: &str = r###"/**
 * Language: Veryl
 * Contributors:
 *   Naoya Hatta <dalance@gmail.com>
 */
module.exports = function (hljs)
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
          scope: 'number',
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
