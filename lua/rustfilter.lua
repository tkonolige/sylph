local filterer = require("filter")

-- create matcher object
local matcher = filterer.threaded_matcher()
local timer = nil
function handler(window, lines, query, callback)
  local err = matcher:query(query, window.launched_from_name, 10, lines)
  if err ~= nil then
    sylph.print_err(ffi.string(err))
    return
  end

  -- poll matcher to see if it has completed
  local timer_callback
  timer_callback = function()
    if timer ~= nil then
      timer:close()
      timer = nil
    end
    local res, err = matcher:get_result()
    if err ~= nil then
      sylph.print_err(ffi.string(err))
      return
    end
    if res == nil then
      vim.defer_fn(timer_callback, 5)
    else
      local matched_lines = {}
      for _, x in ipairs(res) do
        matched_lines[#matched_lines+1] = lines[x.index+1]
      end
      callback(matched_lines)
    end
  end
  timer_callback()
end

function on_selected(line)
  matcher:update(line.location.path)
end

sylph:register_filter("rust", {handler = handler, on_selected = on_selected})
