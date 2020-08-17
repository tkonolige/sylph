sylph = {} -- not local so we can all into this module from viml callbacks

local jobstart = require("job")
local json = require("json")

--------------------------------
-- Globals
--------------------------------
local default_provider_args = {
  run_on_input = false,
  handler = nil -- function(query: String, callback: Function(List<String>))
}

local providers = {}
local filters = {}

local window -- need to store a reference to the shown window so we can set keymaps for it

local output_file = vim.api.nvim_eval("expand(\"~/.cache/nvim/sylph.log\")")

--------------------------------
-- Utility functions
--------------------------------

local function split_lines(x)
  lines = {}
  if x ~= nil then
    i = 1
    for s in x:gmatch("[^\r\n]+") do
      lines[i] = s
      i = i + 1
    end
  end
  return lines
end

local function map(fn, ary)
  local out = {}
  for _, x in ipairs(ary) do
    out[#out+1]=fn(x)
  end
  return out
end

local function append(x, y)
  for _, l in ipairs(y) do
    x[#x+1] = l
  end
  return x
end

local function filter(x)
  local y = {}
  for _, l in ipairs(x) do
    if l ~= nil then
      y[#y+1] = l
    end
  end
  return y
end


local function keys(tbl)
  ks = {}
  for k, _ in pairs(tbl) do
    ks[#ks+1] = k
  end
  return ks
end

local function print_err(fmt, ...)
  args = {...}
  vim.schedule(function()
    err_msg = (args == nil) and fmt or string.format(fmt, unpack(args))
    -- Close window first so error message is displayed afterwards
    sylph:close_window()
    vim.api.nvim_err_writeln(string.format("Sylph error: %s", err_msg))
  end)
end

sylph.print_err = print_err

--------------------------------
-- Window creation and handlers
--------------------------------

local default_filter = "rust"

function sylph:init(provider_name, filter_name)
  vim.api.nvim_command("stopinsert!")
  local provider = providers[provider_name]
  if provider == nil then
    print_err("sylph: Error: provider %s not found. Available providers are %s", name, vim.inspect(keys(providers)))
    return
  end
  -- Use rust as the default filter
  if filter_name == nil then
    filter_name = default_filter
  end
  local filter = filters[filter_name]
  if filter == nil then
    print_err("sylph: Error: filter %s not found. Available filters are %s", name, vim.inspect(keys(filters)))
    return
  end

  -- Window holds all information for the current fuzzy finder session
  window = {
    provider = provider,
    filter = filter,
    launched_from = vim.api.nvim_eval("bufnr(\"%\")"),
    launched_from_name = vim.api.nvim_eval("expand(\"%\")"),
    query = "",
    running_proc = nil,
    selected = 1,
    lines = {}
  }
  -- Ensure window.launched_from_name is a string
  if window.launched_from_name == nil then
    window.launched_from_name = ""
  end

  function window:create()
    self.buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_name(self.buf, "__sylph")
    self.win = vim.api.nvim_open_win(self.buf, true, {relative="win", row=20, col=20, width=80, height=1, style="minimal"})
    vim.api.nvim_buf_set_option(self.buf, "filetype", "sylph")
    vim.api.nvim_buf_set_option(self.buf, "bufhidden", "wipe")
    vim.api.nvim_command("startinsert!")

    vim.api.nvim_buf_attach(self.buf, false, {on_lines=function(_, _, _, f, l) self:on_input(f,l) end})

    vim.api.nvim_buf_set_keymap(buf, "i", "<esc>", "<esc>:lua sylph:close_window()<CR>", {nowait=true,silent=true})
    vim.api.nvim_buf_set_keymap(buf, "i", "<CR>", "<C-o>:lua sylph:enter()<CR>", {nowait=true,silent=true})
    vim.api.nvim_buf_set_keymap(buf, "i", "<C-J>", "<C-o>:lua sylph:move(1)<CR>", {nowait=true,silent=true})
    vim.api.nvim_buf_set_keymap(buf, "i", "<C-K>", "<C-o>:lua sylph:move(-1)<CR>", {nowait=true,silent=true})

    -- automatically close window when we loose focus
    vim.api.nvim_command("au WinLeave <buffer> :lua sylph:close_window()")
    -- Leave the user in normal mode when the window closes. Its confusing if
    -- the user ends up in insert mode
    vim.api.nvim_command("au WinLeave <buffer> stopi")

    -- run initial provider
    if not self.provider.run_on_input then
      self.running_proc = self.provider.handler(self, self.query,
      function(lines)
        if lines ~= nil then
          self.stored_lines = lines
          self.filter.handler(self, lines, self.query, function(lines) self:draw(lines) end)
        end
      end)
    end
  end

  function window:on_input(firstline, lastline)
    -- First line contains the users input, so we only check for changes there
    if firstline == 0 then
      self.query = vim.api.nvim_buf_get_lines(self.buf, 0, 1, false)[1]
      if self.provider.run_on_input then
        if self.running_proc ~= nil then
          self.running_proc()
          self.running_proc = nil
        end
        self.running_proc = self.provider.handler(self, self.query,
          function(lines)
            self.stored_lines = lines
            self.filter.handler(self, lines, self.query, function(lines) self:draw(lines) end)
          end)
      else
        if self.stored_lines ~= nil then
          self.filter.handler(self, self.stored_lines, self.query, function(lines) self:draw(lines) end)
        end
      end
    end
  end

  function window:write_selected(selected)
    -- TODO: run in background thread
    if output_file ~= nil and #self.stored_lines < 1000 then
      vim.loop.fs_open(output_file, "a", 438, function(err, f)
        if err then
          print_err("Could not open output file for writing")
          return
        end
        vim.loop.fs_write(f,
          json.encode({lines=self.stored_lines,
          launched_from=self.launched_from_name,
          selected=selected,
          provider=self.provider.name,
          filter=self.filter.name,
          query=self.query
          }),
          -1 -- use existing file offset
        )
        vim.loop.fs_write(f, "\n", -1)
        vim.loop.fs_close(f)
      end)
    end
  end

  function window:draw(lines)
    if lines ~= nil then
      self.lines = lines
      self.selected = 1
      -- TODO: move to config
      local num_lines = math.min(10, #lines)
      local formatted = map(function(x) return x.line end, {unpack(lines, 1, num_lines)})
      for i,x in ipairs(formatted) do
        if type(x) ~= "string" then
          error(string.format("Line %d in filter lines is not a string. Actual value: %s",i,vim.inspect(x)))
        end
      end
      vim.schedule(function()
        vim.api.nvim_buf_set_lines(self.buf, 1, -1, false, formatted)
        vim.api.nvim_win_set_height(self.win, num_lines+1)
        self:update_highlights()
      end)
    end
  end

  window.namespace = vim.api.nvim_create_namespace("sylph")
  function window:update_highlights()
    vim.schedule(function()
      vim.api.nvim_buf_clear_namespace(self.buf, self.namespace, 0, -1)
      vim.api.nvim_buf_add_highlight(self.buf, self.namespace, "StatusLine", self.selected, 0, -1)
    end)
  end

  function window:move(dir)
    self.selected = (((self.selected+dir)-1) % #self.lines) + 1
    self:update_highlights()
  end

  function window:enter()
    if self.selected > 0 and self.selected <= #self.lines then
      self:write_selected(self.lines[self.selected])
      if self.filter.on_selected ~= nil then
        self.filter.on_selected(self.lines[self.selected])
      end
      local loc = self.lines[self.selected].location
      -- open current buffer for file if it exists
      local buf = vim.api.nvim_eval("bufnr(\""..loc.path.."\")")
      window:close()
      local cmd
      if buf ~= -1 then
        cmd = ":b " .. buf
      else
        cmd = ":e " .. loc.path
      end
      vim.schedule(function()
        vim.api.nvim_command(cmd)
        if loc.row ~= nil then
          vim.call("cursor", loc.row, loc.col)
          vim.api.nvim_command("normal! zz")
        end
      end)
    end
  end

  function window:close()
      vim.api.nvim_command("bw! "..self.buf)
      window = nil
  end

  window:create()
end

function sylph:close_window()
  if window ~= nil then
    window:close()
  end
end

function sylph:enter()
  window:enter()
end

function sylph:move(dir)
  window:move(dir)
end

function sylph:register_provider(name, initializer)
  if providers[name] ~= nil then
    print_err("sylph: Error: provider with name %s already exists", name)
  else
    providers[name] = initializer
    providers[name].name = name
  end
end

function sylph:register_filter(name, initializer)
  if filters[name] ~= nil then
    print_err("sylph: Error: filter with name %s already exists", name)
  else
    filters[name] = initializer
    filters[name].name = name
  end
end

function sylph:process(process_name, args, postprocess)
  return function(window, query, callback)
    local uv = vim.loop
    local stdout = uv.new_pipe(false)
    local stderr = uv.new_pipe(false)
    local stdin = uv.new_pipe(false)
    local lines = {}

    -- TODO: buffer data until newline
    function onread(err, chunk)
      assert(not err, err)
      if (chunk) then
        append(lines, filter(map(function(x) return postprocess(window, x) end, split_lines(chunk))))
      end
    end

    function onreaderr(err, chunk)
      if chunk then
        print_err("sylph: Error while running command: %s", chunk)
        return
      end
    end

    local exited = false
    function onexit(code, signal)
      -- if code > 0 and not exited then
      --   print_err("Command %s exited with code %s", process_name, code)
      --   return
      -- end
      if not exited then
        callback(lines)
        exited = true
      end
    end

    local args_ = vim.deepcopy(args)
    args_[#args_+1] = query
    local handle
    local pid
    handle, pid = uv.spawn(process_name, {args=args_, stdio={stdin, stdout, stderr}}, onexit)
    if pid == nil then
      print_err("sylph. Error running command %s: %s", args_, handle)
    end
    uv.read_start(stdout, onread)
    uv.read_start(stderr, onreaderr)

    return function()
      if handle ~= nil and not exited then
        exited = true
        uv.process_kill(handle, 9)
        handle = nil
      end
    end
  end
end

sylph:register_provider("files", {
  handler = sylph:process("fd", {"-t", "f"},
                          function(window, x)
                            -- filter out our current file
                            if window.launched_from_name == x then
                              return nil
                            else
                              return {location={path=x}, line=x}
                            end
                          end),
  run_on_input = false,
})
sylph:register_provider("grep", {
  handler = sylph:process("rg", {"-n", "--column"}, function(window, line)
    local m = line:gmatch("[^:]+")
    local path = m()
    local row = m()
    local col = m()
    local line = m()
    return {location={path=path, row=row, col=col}, line=line}
  end),
  run_on_input = true
})
sylph:register_filter("identity", {handler=function(window, data, query, callback) callback(data) end})

local function rust_filter_rpc()
  local plugin_dir = vim.api.nvim_eval("expand('<sfile>:p:h:h')")
  local exe = plugin_dir.."/rust/target/release/sylph"
  local function on_err(err, lines)
    print_err("Sylph: error in rust filter: %s", vim.inspect(lines))
  end
  local function on_exit(code, signal)
    print_err("Sylph: rust filter exited with code %s, signal %s", code, signal)
  end
  local job = jobstart.jobstart({exe}, {rpc=true,
                                        on_stderr=on_err,
                                        on_exit=on_exit})
  if job == -1 then
    print_err("Could not launch "..exe)
    return
  end
  if job == 0 then
    print_err("Invalid arguments to "..exe)
    return
  end

  local filter = {}
  function filter.handler(window, data, query, callback)
    job:rpcrequest("match", { query = query
                            , lines = data
                            , context = window.launched_from_name
                            , num_matches = 10
                          }, function(resp)
                            local ls = {}
                            for i,x in ipairs(resp) do
                              ls[i] = data[x.index + 1]
                            end
                            callback(ls)
                            end)
  end
  function filter.on_selected(line)
    job:rpcrequest("selected", line, nil)
  end
  return filter
end
-- local rfilter = rust_filter_rpc()
-- sylph:register_filter("rust", {handler = rfilter.handler, on_selected = rfilter.on_selected})

local rfilter = require("rustfilter")
if rfilter ~= nil then
  sylph:register_filter("rust", {handler = rfilter.handler, on_selected = rfilter.on_selected})
end

vim.api.nvim_command("doautocmd <nomodeline> User SylphStarted")

return sylph
