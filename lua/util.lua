local util = {}

function util.map(fn, ary)
  local out = {}
  for _, x in ipairs(ary) do
    out[#out + 1] = fn(x)
  end
  return out
end

function util.append(x, y)
  for _, l in ipairs(y) do
    x[#x + 1] = l
  end
  return x
end

function util.filter(x)
  local y = {}
  for _, l in ipairs(x) do
    if l ~= nil then
      y[#y + 1] = l
    end
  end
  return y
end

function util.keys(tbl)
  local ks = {}
  for k, _ in pairs(tbl) do
    ks[#ks + 1] = k
  end
  return ks
end

function util.split_lines(x)
  local lines = {}
  if x ~= nil then
    local i = 1
    for s in x:gmatch("[^\r\n]+") do
      lines[i] = s
      i = i + 1
    end
  end
  return lines
end

return util
