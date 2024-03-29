local util = require("./util")
local function file_exists(name)
  local f = io.open(name, "r")
  if f ~= nil then
    io.close(f)
    return true
  else
    return false
  end
end

local d = debug.getinfo(1).source:match("@?(.*/)")
local soname = "so"
if io.popen("uname"):read() == "Darwin" then
  soname = "dylib"
end

-- Load locally compiled filter plugin. If it doesn't exist, load a downloaded one.
local filterer
if file_exists(d .. "../rust/target/release/libfilter." .. soname) then
  filterer = package.loadlib(d .. "../rust/target/release/libfilter." .. soname, "luaopen_filter")()
elseif file_exists(d .. "/libfilter." .. soname) then
  filterer = package.loadlib(d .. "/libfilter." .. soname, "luaopen_filter")()
else
  sylph.download.download_from_github()
  filterer = package.loadlib(d .. "/libfilter." .. soname, "luaopen_filter")()
end

-- create matcher object
local matcher = filterer.threaded_matcher()
local function handler(window, query, callback)
  -- local lines = vim.deepcopy(lines) -- Cache local lines because it may be updated
  local command_num, err = matcher:query(query, window.launched_from_name, 10)
  if err ~= nil then
    sylph.print_err(err)
    return
  end

  return {
    lines = {},
    feed = function(self, lines)
      util.append(self.lines, lines)
      matcher:feed(lines)
    end,
    done = function(self)
      matcher:done()
      self.timer = vim.loop.new_timer()

      -- poll matcher to see if it has completed
      local timer_callback = function()
        local res, err = matcher:get_result(command_num)
        if err ~= nil then
          if self.timer ~= nil then
            self.timer:stop()
            self.timer:close()
            self.timer = nil
          end
          -- sylph.print_err(err)
          return
        end
        if res ~= nil then
          if self.timer ~= nil then
            self.timer:stop()
            self.timer:close()
            self.timer = nil
          end
          local matched_lines = {}
          for _, x in ipairs(res) do
            local l = self.lines[x.index + 1]
            if l ~= nil then
              l.frequency_score = x.frequency_score
              l.context_score = x.context_score
              l.query_score = x.query_score
              matched_lines[#matched_lines + 1] = l
            end
          end
          callback(matched_lines)
        else
        end
      end

      self.timer:start(10, 10, timer_callback)
    end,
    exit = function(self)
      if self.timer ~= nil then
        self.timer:stop()
        self.timer:close()
        self.timer = nil
      end
    end
  }
end

local function on_selected(line)
  matcher:update(line.location.path)
end

sylph:register_filter("rust", { handler = handler, on_selected = on_selected })
