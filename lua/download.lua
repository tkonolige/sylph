local download = {}

local d = debug.getinfo(1).source:match("@?(.*/)")
local soname = "so"
local libname = "ubuntu-latest-libfilter.so"
if io.popen("uname"):read() == "Darwin" then
  soname = "dylib"
  libname = "macos-latest-libfilter.dylib"
end

download.download_from_github = function()
  print("Downloading from github")
  local cmd = "curl -sL https://github.com/tkonolige/sylph/releases/latest/download/" ..
  libname .. " --output " .. d .. "/libfilter." .. soname .. " 2>&1"
  local handle = io.popen(cmd, "r")
  if handle == nil then
    print("Could execute command " .. cmd)
  else
    local result = handle:read("*a")
    print(result)
    handle:close()
  end
end

return download
