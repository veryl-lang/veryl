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

syn keyword verylStatement module interface function modport
syn keyword verylStatement enum struct
syn keyword verylStatement posedge negedge
syn keyword verylStatement always_ff always_comb
syn keyword verylStatement input output inout
hi link verylStatement Statement

syn keyword verylConditional if else
hi link verylConditional Conditional

syn match   verylSpecial "[&|~><!)(#@=?:;}{,.\^\-\[\]]"
hi link verylSpecial Special

syn match   verylOperator "[*%+/]"
hi link verylOperator Operator

syn region verylComment start="/\*" end="\*/"
syn match  verylComment "//.*"
hi link verylComment Comment

syn match   verylNumber "\(\<\d\+\|\)'[sS]\?[bB]\s*[0-1_xXzZ?]\+\>"
syn match   verylNumber "\(\<\d\+\|\)'[sS]\?[oO]\s*[0-7_xXzZ?]\+\>"
syn match   verylNumber "\(\<\d\+\|\)'[sS]\?[dD]\s*[0-9_xXzZ?]\+\>"
syn match   verylNumber "\(\<\d\+\|\)'[sS]\?[hH]\s*[0-9a-fA-F_xXzZ?]\+\>"
syn match   verylNumber "\<[+-]\=[0-9_]\+\(\.[0-9_]*\|\)\(e[0-9_]*\|\)\>"
hi link verylNumber Number

let b:current_syntax = 'veryl'
