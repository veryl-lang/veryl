" Veryl syntax file
" Language:             Veryl
" Maintainer:           Naoya Hatta <dalance@gmail.com>
" URL:                  https://github.com/dalance/veryl
" License:              Apache 2
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
syn match verylNumber "\<\=[0-9_]\+\(\.[0-9_]*\|\)\([eE][+-][0-9_]*\|\)\>"
hi def link verylNumber Number

" Operator
syn match verylOperator1           "\m[~!&|^*+-/%><]"
syn match verylAssignmentOperator1 "\m[=]"
syn match verylOperator2           "\M<<\|>>\|<=\|>=\|==\|!=\|&&\|||"
syn match verylAssignmentOperator2 "\M+=\|-=\|*=\|/=\|%=\|&=\||=\|\^="
syn match verylOperator3           "\M<<<\|>>>\|===\|==?\|!==\|!=?"
syn match verylAssignmentOperator3 "\M<<=\|>>="
syn match verylAssignmentOperator4 "\M<<<=\|>>>="

hi def link verylOperator1           Operator
hi def link verylOperator2           Operator
hi def link verylOperator3           Operator
hi def link verylAssignmentOperator1 Special
hi def link verylAssignmentOperator2 Special
hi def link verylAssignmentOperator3 Special
hi def link verylAssignmentOperator4 Special

" Symbol
syn match verylSymbol "[)(#@:;}{,.\[\]]"
hi def link verylSymbol Special

" Keyword
syn keyword verylStructure module interface function modport
syn keyword verylStructure enum struct
hi def link verylStructure Structure

syn keyword verylStatement parameter localparam
syn keyword verylStatement posedge negedge
syn keyword verylStatement async_high async_low sync_high sync_low
syn keyword verylStatement always_ff always_comb assign return
hi def link verylStatement Statement

syn keyword verylType logic bit
syn keyword verylType u32 u64 i32 i64 f32 f64
hi def link verylType Type

syn keyword verylDirection input output inout ref
hi def link verylDirection Keyword

syn keyword verylConditional if if_reset else for in
hi def link verylConditional Conditional

syn keyword verylRepeat for in step
hi def link verylRepeat Repeat

" Identifier
syn match verylIdentifier "[a-zA-Z_][0-9a-zA-Z_]*"
hi def link verylIdentifier Identifier

" Comment
syn region verylComment start="/\*" end="\*/"
syn match  verylComment "//.*"
hi def link verylComment Comment

let b:current_syntax = 'veryl'
