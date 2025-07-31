use crate::templates::Template;
use handlebars::Handlebars;
use std::path::PathBuf;

const TMPL: &str = r###"%YAML 1.2
---

name: Veryl
file_extensions: [veryl]

scope: source.veryl

contexts:
  main:
    - match: \b({{#each structure}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\b
      scope: keyword.declaration.veryl
    - match: \b({{#each statement}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}}|{{#each literal}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\b
      scope: keyword.other.veryl
    - match: \b({{#each conditional}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}}|{{#each repeat}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\b
      scope: keyword.control.veryl
    - match: \b({{#each type}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\b
      scope: storage.type.veryl
    - match: \b({{#each direction}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\b
      scope: storage.modifier.veryl
    - match: '`[a-zA-Z0-9_]+'
      scope: constant.language.veryl
    - match: '[a-zA-Z0-9_]+'
      scope: identifier.veryl
    - include: string
    - include: comments

  string:
    - match: '"'
      push:
        - meta_scope: string.quoted.double.veryl
        - match: \\.
          scope: constant.character.escape.veryl
        - match: '"'
          pop: true

  comments:
    - match: /\*
      captures:
        0: punctuation.definition.comment.veryl
      push:
        - meta_scope: comment.block.veryl
        - match: \*/
          pop: true
    - match: (//).*$\n?
      scope: comment.line.double-slash.veryl
      captures:
        1: punctuation.definition.comment.veryl
"###;

pub struct Sublime;

impl Template for Sublime {
    fn apply(&self, keywords: &crate::keywords::Keywords) -> String {
        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(TMPL, &keywords).unwrap()
    }

    fn path(&self) -> PathBuf {
        PathBuf::from("support/sublime/veryl.sublime-syntax")
    }
}
