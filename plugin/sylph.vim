if exists('g:loaded_sylph')
  finish
endif
let g:loaded_sylph = 1

lua require('sylph')

command! -nargs=* Sylph lua sylph:init(<f-args>)
