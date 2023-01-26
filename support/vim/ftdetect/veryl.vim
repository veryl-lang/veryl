autocmd BufRead,BufNewFile *.vl call s:set_veryl_filetype()

function! s:set_veryl_filetype() abort
    if &filetype !=# 'veryl'
        set filetype=veryl
    endif
endfunction
