local M = {}

M.timers = {}
M.times = {}

function M.start(name)
  M.timers[name] = vim.fn.reltimefloat(vim.fn.reltime())
end

function M.stop(name)
  if M.timers[name] ~= nil then
    local t = vim.fn.reltimefloat(vim.fn.reltime()) - M.timers[name]
    M.timers[name] = nil
    if M.times[name] == nil then
      M.times[name] = {t}
    else
      table.insert(M.times[name], t)
    end
  end
end

function M.statistic(name, subname, func)
  s = func(M.times[name][#M.times[name]])
  n = name .. " " .. subname
  if M.times[n] == nil then
    M.times[n] = {s}
  else
    table.insert(M.times[n], s)
  end
end

local function median(xs)
  if #xs % 2 == 1 then
    return xs[math.floor(#xs/2)+1]
  else
    return (xs[math.floor(#xs/2)] + xs[math.floor(#xs/2)+1]) / 2
  end
end

local function mean(xs)
  local sum = 0
  for _, x in ipairs(xs) do
    sum = sum + x
  end
  return sum/#xs
end

local function maximum(xs)
  local m = -1
  for _, x in ipairs(xs) do
    m = math.max(m, x)
  end
  return m
end

function M.report()
  print("name    mean    median    max")
  for name, times in pairs(M.times) do
    print(string.format("%s  %f  %f  %f", name, mean(times), median(times), maximum(times)))
  end
end

return M
