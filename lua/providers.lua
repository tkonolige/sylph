-- Providers for lsp, grep, and files
--
-- The handler for each provider takes a window, query, matcher, and finished function.
-- Call `matcher:feed(lines)` to incrementally provide lines to the filter/matcher.
-- Call `matcher:done()` and `finished(lines)` when all lines have been collected.
local util = require("util")

-- Provider for the built in neovim lsp server
local lsp_util = require("vim.lsp.util")
local function lsp_handle(window, query, matcher, finished)
  local clients = vim.lsp.get_active_clients()
  if #clients == 0 then
    sylph.print_err("There are currently no running LSP clients")
  end
  local num_requests = #clients

  local lines = {} -- We accumulate lines because there are multiple clients
  local function h(err, result, ctx, config)
    local cwd = vim.fn.getcwd()
    num_requests = num_requests - 1
    if err then
      sylph.print_err("LSP error: %s", err)
    else
      if result ~= nil then
        for _, x in ipairs(result) do
          local protocol, path = x.location.uri:gmatch("([a-z]+)://(.+)")()
          if string.sub(path, 1, cwd:len()) == cwd then
            path = string.sub(path, cwd:len() + 2)
          end
          local col = x.location.range.start.character + 1
          local row = x.location.range.start.line + 1
          lines[#lines + 1] = {
            line = string.format("%s:%d:%d: %s", path, row, col, x.name),
            location = {
              path = path,
              col = col,
              row = row,
            },
          }
        end
      end
    end
    if num_requests == 0 then
      matcher:feed(lines)
      matcher:done()
      finished(lines)
    end
  end

  local cancels = {}
  for _, lsp in ipairs(clients) do
    local status, id = lsp.request("workspace/symbol", { query = query }, h)
    if not status then
      sylph.print_err("LSP server %s has shut down", lsp.name)
    end
    cancels[#cancels + 1] = function()
      lsp.cancel_request(id)
    end
  end
  return function()
    for _, f in ipairs(cancels) do
      f()
    end
  end
end
sylph:register_provider("lsp", { handler = lsp_handle, run_on_input = true })

-- Helper to create a providers from the output of a terminal command
-- Accumulates all results of the running process before passing results to the callback.
-- TODO: buffer data until newline and incrementally process lines
function sylph:process(process_name, args, postprocess)
  return function(window, query, matcher, finished)
    local uv = vim.loop
    local stdout = uv.new_pipe(false)
    local stderr = uv.new_pipe(false)
    local lines = {}

    local function onread(err, chunk)
      assert(not err, err)
      if chunk then
        local chunk_lines = util.filter(util.map(function(x)
          return postprocess(window, x)
        end, util.split_lines(chunk)))
        matcher:feed(chunk_lines)
        util.append(lines, chunk_lines)
      end
    end

    local function onreaderr(err, chunk)
      if chunk then
        sylph.print_err("sylph: Error while running command: %s", chunk)
        return
      end
    end

    local exited = false
    local function onexit(code, signal)
      if code > 0 and not exited then
        sylph.print_err("Command %s exited with code %s", process_name, code)
        return
      end
      if not exited then
        finished(lines)
        matcher:done()
        exited = true
      end
    end

    local args_ = vim.deepcopy(args)
    args_[#args_ + 1] = query
    local handle
    local pid
    handle, pid = uv.spawn(process_name, { args = args_, stdio = { nil, stdout, stderr } }, onexit)
    if pid == nil then
      sylph.print_err("sylph. Error running command %s: %s", args_, handle)
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
  handler = sylph:process("fd", { "-t", "f" }, function(window, x)
    -- filter out our current file
    if window.launched_from_name == x then
      return nil
    else
      return { location = { path = x }, line = x }
    end
  end),
  run_on_input = false,
})
sylph:register_provider("grep", {
  handler = sylph:process("rg", { "-n", "--column" }, function(window, line)
    local m = line:gmatch("[^:]+")
    local path = m()
    local row = m()
    local col = m()
    local line = m()
    if path == nil or row == nil or line == nil then
      return nil
    end
    return { location = { path = path, row = row, col = col }, line = path .. ":" .. line }
  end),
  run_on_input = true,
})
