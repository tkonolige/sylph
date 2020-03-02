sylph = {}

local function map(ary, fn)
  local out = {}
  for _, x in ipairs(ary) do
    out[#out+1]=fn(x)
  end
  return out
end

local providers = {}

-- API
-- register(name, initialization)
-- initialization(query, new_data_callback) -> close()
-- new_data_callback items:
-- { path: string }
function sylph:register(name, initializer)
  providers[name] = initializer
end

local bufname = "__sylph"
-- store a function that will kill the current process
local end_proc = nil

local function input_handler(initializer)
  return function(_, buf, changedtick, firstline, lastline)
    -- We only process updates to the first line, otherwise we infinite loop
    if firstline == 0 then
      local line = vim.api.nvim_buf_get_lines(buf, 0, 1, false)

      local win = vim.api.nvim_get_current_win()

      -- start the callback with empty lines to clear the result
      -- TODO: don't do this unless it takes too long to get results?
      vim.schedule(vim.schedule_wrap(function()
        vim.api.nvim_buf_set_lines(buf, 1, -1, false, {})
        vim.api.nvim_buf_set_var(buf, "selected", -1)

        if end_proc ~= nil then
          end_proc()
          end_proc = nil
        end
        end_proc = initializer(line[1], function(files)
          -- TODO: could use nvim_call_atomic to avoid redraws
          -- TODO: limit number of displayed elements
          local lines = {}
          for i, f in ipairs(files) do
            lines[i] = f.name
          end
          vim.schedule(vim.schedule_wrap(function()
            vim.api.nvim_buf_set_lines(buf, 1, -1, false, lines)
            vim.api.nvim_win_set_height(win, math.min(30, #lines+1))
            if #lines > 0 then
              vim.api.nvim_buf_set_var(buf, "selected", 1)
            else
              vim.api.nvim_buf_set_var(buf, "selected", -1)
            end
          end))
        end)
      end))
    end
  end
end

function sylph:create_window(name)
  local provider = providers[name]
  if provider == nil then
    local keys = {}
    for k,_ in pairs(providers) do
      keys[#keys+1] = k
    end
    vim.api.nvim_err_writeln(string.format("sylph: Error: provider %s not found. Available providers are %s", name, vim.inspect(keys)))
    return
  end
  local buf = vim.api.nvim_eval("bufnr(\""..bufname.."\")")
  if buf == -1 then
    buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_name(buf, bufname)
  end
  local win = vim.api.nvim_open_win(buf, true, {relative='win', row=20, col=20, width=80, height=1, style="minimal"})
  vim.api.nvim_buf_set_option(buf, "filetype", "sylph")
  vim.api.nvim_command("startinsert!")

  local ih = input_handler(provider)
  vim.api.nvim_buf_attach(buf, false, {on_lines=ih})

  -- keybindings
  vim.api.nvim_buf_set_keymap(buf, "n", "<esc>", ":lua sylph.close_window()<CR>", {nowait=true,silent=true})
  vim.api.nvim_buf_set_keymap(buf, "i", "<esc>", "<esc>:lua sylph.close_window()<CR>", {nowait=true,silent=true})
  vim.api.nvim_buf_set_keymap(buf, "v", "<esc>", "<esc>:lua sylph.close_window()<CR>", {nowait=true,silent=true})
  vim.api.nvim_buf_set_keymap(buf, "i", "<CR>", "<esc>:lua sylph.enter("..buf..")<CR>", {nowait=true,silent=true})

  -- automatically close window when we loose focus
  vim.api.nvim_command("au BufLeave <buffer> :lua sylph.close_window()")

  -- selected is one-indexed. -1 indicates nothing is selected
  -- TODO: keep a model of the data and use model-view-controller or some such
  vim.api.nvim_buf_set_var(buf, "selected", -1)
  local n = vim.api.nvim_buf_get_var(buf, "selected")
end

function sylph.close_window()
  local win = vim.api.nvim_get_current_win()
  local buf = vim.api.nvim_get_current_buf()
  vim.api.nvim_win_close(win, true)
  if vim.api.nvim_eval("bufnr(\""..bufname.."\")") ~= -1 then
    vim.api.nvim_command("bw! "..bufname)
  end
end

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


-- TODO: use virtual text for cursor
function sylph:move(buf, dir)
  local selected = vim.api.nvim_buf_get_var(0, "selected")
  vim.api.nvim_buf_set_var(0, "selected", (((selected+dir)-1) % (vim.api.nvim_buf_line_count()-1)) + 1)
end

function sylph:enter(buf)
  local selected = vim.api.nvim_buf_get_var(buf, "selected")
  if selected > 0 and selected <= vim.api.nvim_buf_line_count(buf) then
    local file = vim.api.nvim_buf_get_lines(buf, selected, selected+1, true)
    vim.schedule(function()
      vim.api.nvim_command(":e " .. file[1])
    end)
  end
end

function sylph:process(process_name, args, postprocess)
  return function(query, callback)
    local uv = vim.loop
    local stdout = uv.new_pipe(false)
    local stderr = uv.new_pipe(false)
    local stdin = uv.new_pipe(false)

    -- TODO: buffer data until newline
    function onread(err, chunk)
      assert(not err, err)
      if (chunk) then
        callback(map(split_lines(chunk), postprocess))
      end
    end

    function onreaderr(err, chunk)
      if chunk then
        vim.schedule(vim.schedule_wrap(vim.api.nvim_err_writeln(string.format("sylph. Error while running command: %s", chunk))))
        assert(not err, err)
      end
    end

    local exited = false
    function onexit(code, signal)
      if code > 0 then
        assert(not code, code)
      end
      exited = true
    end

    local args_ = vim.deepcopy(args)
    args_[#args_+1] = query
    handle, pid = uv.spawn(process_name, {args=args_, stdio={stdin, stdout, stderr}}, onexit)
    if pid == nil then
      vim.schedule(vim.schedule_wrap(vim.api.nvim_err_writeln(string.format("sylph. Error running command %s: %s", args_, handle))))
    end
    uv.read_start(stdout, onread)
    uv.read_start(stderr, onreaderr)

    return function()
      if handle ~= nil and not exited then
        uv.process_kill(handle, 9)
      end
    end
  end
end

sylph:register("files", sylph:process("fd", {"-t", "f"}, function(x) return {path=x, name=x} end))
sylph:register("grep", sylph:process("rg", {}, function(line)
  local path = line:gmatch("[^:]+")
  return {path=path, name=line}
end))
