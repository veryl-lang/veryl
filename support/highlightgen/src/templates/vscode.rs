use crate::templates::Template;
use handlebars::Handlebars;
use std::path::PathBuf;

const TMPL: &str = r###"{{#*inline "declaration"}}
				{
					"begin": "(?<![a-zA-Z0-9_$]){{keyword}}(?![a-zA-Z0-9_$])",
					"beginCaptures": {
						"0": {
							"name": "keyword.other.veryl"
						}
					},
					"end": "((?:r#)?[a-zA-Z_][a-zA-Z0-9_$]*)|(?=\\S)(?!//|/\\*)",
					"endCaptures": {
						"1": {
							"name": "entity.name.{{kind}}.veryl"
						}
					},
					"patterns": [
						{
							"include": "#comments"
						}
					]
				}{{/inline~}}
{
	"$schema": "https://raw.githubusercontent.com/martinring/tmlanguage/master/tmlanguage.json",
	"name": "Veryl",
	"fileTypes" : [
		"veryl"
	],
	"patterns": [
		{
			"include": "#declarations"
		},
		{
			"include": "#imports"
		},
		{
			"include": "#generics"
		},
		{
			"include": "#keywords"
		},
		{
			"include": "#storages"
		},
		{
			"include": "#strings"
		},
		{
			"include": "#comments"
		},
		{
			"include": "#numbers"
		},
		{
			"include": "#operators"
		},
		{
			"include": "#identifiers"
		}
	],
	"repository": {
		"keywords": {
			"patterns": [
				{
					"name": "keyword.control.veryl",
					"match": "(?<![a-zA-Z0-9_$])({{#each conditional}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}}|{{#each repeat}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})(?![a-zA-Z0-9_$])"
				},
				{
					"name": "keyword.other.veryl",
					"match": "(?<![a-zA-Z0-9_$])({{#each structure}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}}|{{#each statement}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}}|{{#each literal}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})(?![a-zA-Z0-9_$])"
				}
			]
		},
		"storages": {
			"patterns": [
				{
					"name": "storage.type.veryl",
					"match": "(?<![a-zA-Z0-9_$])({{#each type}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})(?![a-zA-Z0-9_$])"
				},
				{
					"name": "storage.modifier.veryl",
					"match": "(?<![a-zA-Z0-9_$])({{#each direction}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})(?![a-zA-Z0-9_$])"
				}
			]
		},
		"strings": {
			"name": "string.quoted.double.veryl",
			"begin": "\"",
			"end": "\"",
			"patterns": [
				{
					"name": "constant.character.escape.veryl",
					"match": "\\\\."
				}
			]
		},
		"comments": {
			"patterns": [
				{
					"begin": "/\\*",
					"beginCaptures": {
						"0": {
							"name": "punctuation.definition.comment.veryl"
						}
					},
					"end": "\\*/",
					"endCaptures": {
						"0": {
							"name": "punctuation.definition.comment.veryl"
						}
					},
					"name": "comment.block.veryl"
				},
				{
					"begin": "//",
					"beginCaptures": {
						"0": {
							"name": "punctuation.definition.comment.veryl"
						}
					},
					"end": "$\\n?",
					"name": "comment.line.double-slash.veryl"
				}
			]
		},
		"declarations": {
			"patterns": [
{{> declaration keyword="module" kind="type.module"}},
{{> declaration keyword="interface" kind="type.interface"}},
{{> declaration keyword="package" kind="type.namespace"}},
{{> declaration keyword="function" kind="function"}},
{{> declaration keyword="struct" kind="type.struct"}},
{{> declaration keyword="enum" kind="type.enum"}}

			]
		},
		"imports": {
			"begin": "(?<![a-zA-Z0-9_$])import(?![a-zA-Z0-9_$])",
			"beginCaptures": {
				"0": {
					"name": "keyword.other.veryl"
				}
			},
			"end": ";",
			"patterns": [
				{
					"name": "punctuation.definition.wildcard.veryl",
					"match": "\\*"
				},
				{
					"include": "$self"
				}
			]
		},
		"generics": {
			"begin": "(::)(<)",
			"beginCaptures": {
				"1": {
					"name": "punctuation.separator.namespace.veryl"
				},
				"2": {
					"name": "punctuation.definition.generic.begin.veryl"
				}
			},
			"end": "(>)|(?=[;{}])",
			"endCaptures": {
				"1": {
					"name": "punctuation.definition.generic.end.veryl"
				}
			},
			"patterns": [
				{
					"include": "$self"
				}
			]
		},
		"numbers": {
			"patterns": [
				{
					"name": "constant.numeric.integer.veryl",
					"match": "(?<![a-zA-Z0-9_$])(?:[0-9]+(?:_[0-9]+)*)?'(?:s?[bodh][0-9a-fA-FxzXZ]+(?:_[0-9a-fA-FxzXZ]+)*|[01xzXZ])(?![a-zA-Z0-9_$])"
				},
				{
					"name": "constant.numeric.float.veryl",
					"match": "(?<![a-zA-Z0-9_$])[0-9]+(?:_[0-9]+)*\\.[0-9]+(?:_[0-9]+)*(?:[eE][+-]?[0-9]+(?:_[0-9]+)*)?(?![a-zA-Z0-9_$])"
				},
				{
					"name": "constant.numeric.integer.veryl",
					"match": "(?<![a-zA-Z0-9_$])[0-9]+(?:_[0-9]+)*(?![a-zA-Z0-9_$])"
				}
			]
		},
		"operators": {
			"patterns": [
				{
					"name": "punctuation.separator.namespace.veryl",
					"match": "::"
				},
				{
					"name": "keyword.operator.veryl",
					"match": "<<<=|>>>=|<<=|>>=|<<<|>>>|<<|>>|==\\?|!=\\?|==|!=|<=|>=|<:|>:|<>|->|<-|\\+:|-:|\\.\\.=|\\.\\.|\\*\\*|\\|\\||&&|~[&|^]|[+*/%&|^\\-]=|[=+*/%&|^~!?\\-]"
				}
			]
		},
		"identifiers": {
			"patterns": [
				{
					"match": "(?<![a-zA-Z0-9_$])(?:r#|\\$)?[a-zA-Z_][a-zA-Z0-9_$]*",
					"name": "variable.other.identifier.veryl"
				}
			]
		}
	},
	"scopeName": "source.veryl"
}
"###;

pub struct Vscode;

impl Template for Vscode {
    fn apply(&self, keywords: &crate::keywords::Keywords) -> String {
        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(TMPL, &keywords).unwrap()
    }

    fn path(&self) -> PathBuf {
        PathBuf::from("support/vscode/syntaxes/veryl.tmLanguage.json")
    }
}
