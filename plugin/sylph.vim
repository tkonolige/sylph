if exists('g:loaded_sylph')
  finish
endif
let g:loaded_colorizer = 1

lua require'sylph'

command! -nargs=* Sylph lua sylph:init(<f-args>)
