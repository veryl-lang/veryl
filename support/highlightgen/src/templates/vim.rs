use crate::templates::Template;
use handlebars::Handlebars;
use std::path::PathBuf;

const TMPL: &str = r###"" Veryl syntax file
" Language:             Veryl
" Maintainer:           Naoya Hatta <dalance@gmail.com>
" URL:                  https://github.com/veryl-lang/veryl
" License:              MIT or Apache 2
" ----------------------------------------------------------------------------

" quit when a syntax file was already loaded
if exists("b:current_syntax")
    finish
endif

" Number
syn match verylNumber "\(\<\d\+\|\)'[sS]\?[bB]\s*[0-1_xXzZ?]\+\>"
syn match verylNumber "\(\<\d\+\|\)'[sS]\?[oO]\s*[0-7_xXzZ?]\+\>"
syn match verylNumber "\(\<\d\+\|\)'[sS]\?[dD]\s*[0-9_xXzZ?]\+\>"
syn match verylNumber "\(\<\d\+\|\)'[sS]\?[hH]\s*[0-9a-fA-F_xXzZ?]\+\>"
syn match verylNumber "\<[0-9_]\+\(\.[0-9_]*\|\)\([eE][+-][0-9_]*\|\)\>"
syn match verylNumber "'[01xzXZ]"
hi def link verylNumber Number

" Operator
syn match verylOperator1           "\m[~!&|^*+-/%><]"
syn match verylAssignmentOperator1 "\m[=]"
syn match verylOperator2           "\M<<\|>>\|<:\|>:\|<=\|>=\|==\|!=\|&&\|||"
syn match verylAssignmentOperator2 "\M+=\|-=\|*=\|/=\|%=\|&=\||=\|\^="
syn match verylOperator3           "\M<<<\|>>>\|===\|==?\|!==\|!=?"
syn match verylAssignmentOperator3 "\M<<=\|>>="
syn match verylAssignmentOperator4 "\M<<<=\|>>>="

hi def link verylOperator1           Operator
hi def link verylOperator2           Operator
hi def link verylOperator3           Operator
hi def link verylAssignmentOperator1 Operator
hi def link verylAssignmentOperator2 Operator
hi def link verylAssignmentOperator3 Operator
hi def link verylAssignmentOperator4 Operator

" Symbol
syn match verylSymbol "[)(#@:;}{,.\[\]]"
hi def link verylSymbol Special

" Keyword
syn keyword verylStructure {{#each structure}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
hi def link verylStructure Structure

syn keyword verylStatement {{#each statement}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
hi def link verylStatement Statement

syn keyword verylType {{#each type}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
hi def link verylType Type

syn keyword verylDirection {{#each direction}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
hi def link verylDirection Keyword

syn keyword verylConditional {{#each conditional}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
hi def link verylConditional Conditional

syn keyword verylRepeat {{#each repeat}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
hi def link verylRepeat Repeat

{{{{raw}}}}
" Clock Domain
syn match verylClockDomain "`[a-zA-Z_][a-zA-Z0-9_]*"
hi def link verylClockDomain Constant

" Constant
syn match verylConstant "\<[A-Z][0-9A-Z_]\+\>"
hi def link verylConstant Constant

" Comment
syn region verylComment start="/\*" end="\*/"
syn match  verylComment "//.*"
hi def link verylComment Comment

" String
syn region verylString start="\"" skip="\\\"" end="\""
hi def link verylString String

syn include @python syntax/python.vim
unlet b:current_syntax
syn region pyBlock matchgroup=verylStructure start="py{{{" end="}}}" contains=@python keepend

syn include @systemverilog syntax/systemverilog.vim
unlet b:current_syntax
syn region svBlock matchgroup=verylStructure start="sv{{{" end="}}}" contains=@systemverilog keepend

let b:current_syntax = 'veryl'
{{{{/raw}}}}
"###;

pub struct Vim;

impl Template for Vim {
    fn apply(&self, keywords: &crate::keywords::Keywords) -> String {
        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(TMPL, &keywords).unwrap()
    }

    fn path(&self) -> PathBuf {
        PathBuf::from("support/vim/syntax/veryl.vim")
    }
}
