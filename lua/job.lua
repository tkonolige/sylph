job = {}


local function deepcopy(orig)
  error(orig)
end
local function _id(v)
  return v
end
local deepcopy_funcs = {
  table = function(orig)
    local copy = {}
    for k, v in pairs(orig) do
      copy[deepcopy(k)] = deepcopy(v)
    end
    return copy
  end,
  number = _id,
  string = _id,
  ['nil'] = _id,
  boolean = _id,
  ['function'] = _id,
}
deepcopy = function(orig)
  return deepcopy_funcs[type(orig)](orig)
end

local msgpack = require("msgpack")

msgpack.packers['function'] = function (buffer, fct)
  fct(buffer)
end

local function u32(n)
  return function(buffer)
    buffer[#buffer+1] = string.char(0xCE,      -- uint32
                                    math.floor(n / 0x1000000),
                                    math.floor(n / 0x10000) % 0x100,
                                    math.floor(n / 0x100) % 0x100,
                                    n % 0x100)
  end
end

  --- Wrapper around |vim.loop| to provide a convenience method like |jobstart()|.
  -- *NOTE:* If your callbacks call any Vim functions, you *must* wrap them with |vim.schedule_wrap()|
  -- *before* passing them to |vim.jobstart()|.
  --@see |jobstart()|
  ---
  --@param cmd  Identical to {cmd} in |jobstart()|.
  --@param opts Identical to |jobstart-options|.
  --@returns a job handle on success and nil otherwise
function job.jobstart(cmd, opts)
  -- Determine if we need to split the command or not
  if type(cmd) == "string" then
    -- NOTE: We could call vim.fn.split here to maintain perfect compatibility
    local split_pattern = "%s+"
    local shell_cmd = vim.list_extend(vim.split(vim.o.shell, split_pattern), vim.split(vim.o.shellcmdflag, split_pattern))
    cmd = vim.list_extend(shell_cmd, {'"' .. cmd .. '"'})
  end

  local job_options = deepcopy(opts)
  job_options.args = unpack(cmd, 2)
  cmd = cmd[1]
  -- TODO: Check for valid options, like in src/nvim/eval/funcs.c
  if opts.clear_env then
    job_options.env = opts.env
  elseif opts.env then
    job_options.env = vim.tbl_extend(vim.fn.environ(), opts.env, "force")
  end

  job_options.detached = opts.detach
  -- TODO: Handle pty and rpc options
  local message_count = 0
  local callbacks = {}
  if opts.rpc then
    opts.on_stdout = function(err, lines)
      assert(not err, err)
      local msg = msgpack.unpack(lines)
      if msg[1] == 1 then
      if msg[3] then
        vim.schedule(vim.api.err_writeln(string.format("RPC error: %s", msg[3])))
      else
        callbacks[msg[2]](msg[4])
        callbacks[msg[2]] = nil
      end
    elseif msg[1] == 2 then
      vim.schedule(vim.api.nvim_eval(msg[2], msg[3]))
    end
    end
  end

  -- Set up pipes as needed
  local stdin = vim.loop.new_pipe(false)
  local stdout = nil
  if opts.on_stdout then
    stdout = vim.loop.new_pipe(false)
  end

  local stderr = nil
  if opts.on_stderr then
    stderr = vim.loop.new_pipe(false)
  end
  job_options.stdio = {stdin, stdout, stderr}

  -- Separate out the callback
  local on_exit = opts.on_exit

  -- Start the job
  local handle = nil
  handle = vim.loop.spawn(cmd, job_options, function(code, signal)
    if stdout then
      stdout:read_stop()
      stdout:close()
    end

    if stderr then
      stderr:read_stop()
      stderr:close()
    end

    handle:close()
    on_exit(code, signal)
  end)

  if stdout then
    vim.loop.read_start(stdout, opts.on_stdout)
  end

  if stderr then
    vim.loop.read_start(stderr, opts.on_stderr)
  end

  local wrapper = {handle=handle}

  function wrapper:write(str)
    vim.loop.write(stdin, str)
  end

  if opts.rpc then
    function wrapper:rpcrequest(name, args, callback)
      callbacks[message_count] = callback
      self:write(msgpack.pack({u32(0), u32(message_count), name, {args}}))
      message_count = message_count + 1
    end
  end

  return wrapper
end

return job
