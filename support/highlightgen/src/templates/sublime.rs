use crate::templates::Template;
use handlebars::Handlebars;
use std::path::PathBuf;

// Escape Sublime variable references so Handlebars preserves {{...}}.
const TMPL: &str = r###"%YAML 1.2
---
name: Veryl
file_extensions: [veryl]
version: 2
scope: source.veryl

variables:
  identifier: '(?:r#)?[a-zA-Z_][0-9a-zA-Z_$]*'
  keyword_start: '(?<![0-9a-zA-Z_$#])'
  keyword_end: '(?![0-9a-zA-Z_$])'
  digits: '[0-9]+(?:_[0-9]+)*'

contexts:
  main:
    - include: comments
    - include: string
    - match: '\{{keyword_start}}embed\{{keyword_end}}'
      scope: keyword.declaration.veryl
      push: embed-header
    - match: '\{{keyword_start}}(?:{{#each structure}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\{{keyword_end}}'
      scope: keyword.declaration.veryl
    - match: '\{{keyword_start}}(?:{{#each statement}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}}|{{#each literal}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\{{keyword_end}}'
      scope: keyword.other.veryl
    - match: '\{{keyword_start}}(?:{{#each conditional}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}}|{{#each repeat}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\{{keyword_end}}'
      scope: keyword.control.veryl
    - match: '\{{keyword_start}}(?:{{#each type}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\{{keyword_end}}'
      scope: storage.type.veryl
    - match: '\{{keyword_start}}(?:{{#each direction}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\{{keyword_end}}'
      scope: storage.modifier.veryl
    # Numeric forms must precede clock-domain quotes and ordinary digits.
    - include: numbers
    - match: "'\\{"
      scope: punctuation.section.sequence.begin.veryl
    - match: "'"
      scope: constant.language.clock-domain.veryl
      push: clock-domain
    - match: '\$[a-zA-Z_][0-9a-zA-Z_$]*'
      scope: variable.other.veryl
    - match: '\{{identifier}}'
      scope: variable.other.veryl

  numbers:
    # Do not consume numeric prefixes of identifiers such as 'data_bus or 'xray.
    - match: '\{{digits}}\.\{{digits}}[eE][+-]?\{{digits}}\{{keyword_end}}'
      scope: constant.numeric.float.veryl
    - match: '\{{digits}}\.\{{digits}}\{{keyword_end}}'
      scope: constant.numeric.float.veryl
    - match: "(?:\{{digits}})?'s?[bodh][0-9a-fA-FxzXZ]+(?:_[0-9a-fA-FxzXZ]+)*\{{keyword_end}}"
      scope: constant.numeric.integer.veryl
    - match: "(?:\{{digits}})?'[01xzXZ]\{{keyword_end}}"
      scope: constant.numeric.integer.veryl
    - match: '\{{digits}}\{{keyword_end}}'
      scope: constant.numeric.integer.veryl

  clock-domain:
    - include: comments
    - match: '\s+'
    - match: '\{{identifier}}'
      scope: constant.language.clock-domain.veryl
      pop: true
    - match: '(?=\S)'
      pop: true

  embed-header:
    - include: comments
    - match: '\{\{\{'
      set: embed-body
    - match: '\{{identifier}}'
      scope: variable.other.veryl
    - match: '[;}]'
      pop: true

  embed-body:
    # Deliberately no Veryl comments/strings/keywords/operators here.
    - match: '\\.'
    - match: '\}\}\}'
      pop: true
    - match: '\{'
      push: embed-inner

  embed-inner:
    - match: '\\.'
    - match: '\{'
      push: embed-inner
    - match: '\}'
      pop: true

  string:
    - match: '"'
      push:
        - meta_scope: string.quoted.double.veryl
        - match: '\\["\\fnt]'
          scope: constant.character.escape.veryl
        - match: '"|$'
          pop: true

  comments:
    - match: '/\*'
      captures:
        0: punctuation.definition.comment.veryl
      push:
        - meta_scope: comment.block.veryl
        - match: '\*/'
          pop: true
    - match: '(//).*$'
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
