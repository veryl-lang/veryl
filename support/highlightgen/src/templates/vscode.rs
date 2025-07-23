use crate::templates::Template;
use handlebars::Handlebars;
use std::path::PathBuf;

const TMPL: &str = r###"{
	"$schema": "https://raw.githubusercontent.com/martinring/tmlanguage/master/tmlanguage.json",
	"name": "Veryl",
	"fileTypes" : [
		"veryl"
	],
	"patterns": [
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
			"include": "#identifiers"
		}
	],
	"repository": {
		"keywords": {
			"patterns": [
				{
					"name": "keyword.control.veryl",
					"match": "\\b({{#each conditional}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}}|{{#each repeat}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\\b"
				},
				{
					"name": "keyword.other.veryl",
					"match": "\\b({{#each structure}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}}|{{#each statement}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}}|{{#each literal}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\\b"
				}
			]
		},
		"storages": {
			"patterns": [
				{
					"name": "storage.type.veryl",
					"match": "\\b({{#each type}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\\b"
				},
				{
					"name": "storage.modifier.veryl",
					"match": "\\b({{#each direction}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}})\\b"
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
		"identifiers": {
			"patterns": [
				{
					"match": "\\b[a-zA-Z_][a-zA-Z0-9_$]*\\b",
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
