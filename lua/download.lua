local download = {}

local d = debug.getinfo(1).source:match("@?(.*/)")
local soname = "so"
local libname = "ubuntu-latest-libfilter.so"
if io.popen("uname"):read() == "Darwin" then
	soname = "dylib"
  libname = "macos-latest-libfilter.dylib"
end

download.download_from_github = function()
  local handle = io.popen("curl -sL https://github.com/tkonolige/sylph/releases/latest/download/" .. libname .. " --output " .. d .. "/libfilter." .. soname .. " 2>&1", "r")
  local result = handle:read("*a")
  handle:close()
end

return download
