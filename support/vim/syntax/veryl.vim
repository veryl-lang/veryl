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
hi link verylNumber Number

" Operator
syn match verylOperator1           "\m[~!&|^*+-/%><]"
syn match verylAssignmentOperator1 "\m[=]"
syn match verylOperator2           "\M<<\|>>\|<=\|>=\|==\|!=\|&&\|||"
syn match verylAssignmentOperator2 "\M+=\|-=\|*=\|/=\|%=\|&=\||=\|\^="
syn match verylOperator3           "\M<<<\|>>>\|===\|==?\|!==\|!=?"
syn match verylAssignmentOperator3 "\M<<=\|>>="
syn match verylAssignmentOperator4 "\M<<<=\|>>>="

hi link verylOperator1           Operator
hi link verylOperator2           Operator
hi link verylOperator3           Operator
hi link verylAssignmentOperator1 Special
hi link verylAssignmentOperator2 Special
hi link verylAssignmentOperator3 Special
hi link verylAssignmentOperator4 Special

" Symbol
syn match verylSymbol "[)(#@:;}{,.\[\]]"
hi link verylSymbol Special

" Keyword
syn keyword verylStructure module interface function modport
syn keyword verylStructure enum struct
hi link verylStructure Structure

syn keyword verylStatement parameter localparam
syn keyword verylStatement posedge negedge
syn keyword verylStatement async_high async_low sync_high sync_low
syn keyword verylStatement always_ff always_comb assign return
hi link verylStatement Statement

syn keyword verylType logic bit
syn keyword verylType u32 u64 i32 i64 f32 f64
hi link verylType Type

syn keyword verylDirection input output inout ref
hi link verylDirection Keyword

syn keyword verylConditional if if_reset else for in
hi link verylConditional Conditional

syn keyword verylRepeat for in step
hi link verylRepeat Repeat

" Comment
syn region verylComment start="/\*" end="\*/"
syn match  verylComment "//.*"
hi link verylComment Comment

let b:current_syntax = 'veryl'
