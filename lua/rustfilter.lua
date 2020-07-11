local ffi = require("ffi")
ffi.cdef[[
  void free(void *ptr);
  void * malloc(size_t size);
]]

-- https://stackoverflow.com/questions/24112779/how-can-i-create-a-pointer-to-existing-data-using-the-luajit-ffi
local function alloc_c(typestr, finalizer)
  -- use free as the default finalizer
  if not finalizer then finalizer = ffi.C.free end

  -- automatically construct the pointer type from the base type
  local ptr_typestr = ffi.typeof(string.format("%s *", typestr))

  -- how many bytes to allocate?
  local typesize = ffi.sizeof(typestr)

  -- do the allocation and cast the pointer result
  local ptr = ffi.cast(ptr_typestr, ffi.C.malloc(typesize))

  -- install the finalizer
  ffi.gc(ptr, finalizer)

  return ptr
end

-- allocate array on the c heap
local function alloc_c_array(typestr, length)
  local ptr_typestr = ffi.typeof(string.format("%s *", typestr))
  local typesize = ffi.sizeof(typestr) * length
  local ptr = ffi.cast(ptr_typestr, ffi.C.malloc(typesize))
  ffi.gc(ptr, ffi.C.free)
  return ptr
end

-- Can't use RPC because the serialization overhead is large
local plugin_dir = vim.api.nvim_eval("expand('<sfile>:p:h:h')")
local exe = plugin_dir.."/rust/target/release/sylph"
local lib_path = plugin_dir.."/rust/target/release/libsylph.dylib"
local header = plugin_dir.."/rust/target/bindings.h"

-- read rust header
local lib = ffi.load(lib_path)
local f = io.open(header)
ffi.cdef(f:read("*a"))

-- create matcher object
local matcher_p = alloc_c("struct Matcher*")
local err = lib.new_matcher(matcher_p)
if err ~= nil then
  print_err(ffi.string(err))
end
local matcher = matcher_p[0]
local filter = {}
function filter.handler(window, lines, query, callback)
  local matches = alloc_c_array("Match", 10)
  local lines_ = alloc_c_array("RawLine", #lines)
  -- C structs are zero-indexed
  for i=0,(#lines-1) do
    lines_[i].name = lines[i+1].name
    lines_[i].path = lines[i+1].path
  end
  local num_results = alloc_c("uint64_t")
  local err = lib.best_matches_c(matcher, query, window.launched_from_name, 10, lines_, #lines, matches, num_results)
  if err == nil then
    local matched_lines = {}
    for i=1,tonumber(num_results[0]) do
      matched_lines[i] = lines[tonumber(matches[i-1].index+1)]
    end
    callback(matched_lines)
  else
    print_err(ffi.string(err))
  end
end

function filter.on_selected(line)
  lib.update_matcher(matcher,line.path)
end

return filter
