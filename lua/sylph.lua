sylph = {} -- not local so we can all into this module from viml callbacks

local json = require("json")
local util = require("util")
sylph.download = require("download")
sylph.timer = require("timer")

--------------------------------
-- Globals
--------------------------------
local default_provider_args = {
	run_on_input = false,
	handler = nil, -- function(query: String, callback: Function(List<String>))
}

local providers = {}
local filters = {}

local window -- need to store a reference to the shown window so we can set keymaps for it

local output_file = vim.api.nvim_eval('expand("~/.cache/nvim/sylph.log")')

local function print_err(fmt, ...)
	local args = { ... }
	vim.schedule(function()
		local err_msg = (args == nil) and fmt or string.format(fmt, unpack(args))
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
		print_err(
			"sylph: Error: provider %s not found. Available providers are %s",
			provider_name,
			vim.inspect(util.keys(providers))
		)
		return
	end
	-- Use rust as the default filter
	if filter_name == nil then
		filter_name = default_filter
	end
	local filter = filters[filter_name]
	if filter == nil then
		print_err(
			"sylph: Error: filter %s not found. Available filters are %s",
			filter_name,
			vim.inspect(util.keys(filters))
		)
		return
	end

	-- Window holds all information for the current fuzzy finder session
	local window = {
		provider = provider,
		filter = filter,
		launched_from = vim.api.nvim_eval('bufnr("%")'),
		launched_from_name = vim.api.nvim_eval('expand("%")'),
		query = "",
		running_proc = nil,
		selected = 0,
		lines = {},
	}
	-- Ensure window.launched_from_name is a string
	if window.launched_from_name == nil then
		window.launched_from_name = ""
	end

	function window:create()
		self.inp = vim.api.nvim_create_buf(false, true)
		vim.api.nvim_buf_set_name(self.inp, "__sylph_input")
		self.buf = vim.api.nvim_create_buf(false, true)
		vim.api.nvim_buf_set_name(self.buf, "__sylph")

		-- Set the window size and location
		local current_height = vim.api.nvim_win_get_height(vim.api.nvim_get_current_win())
		local top = math.floor((current_height - 10) * 0.4)
		local margin_side = 20
		local current_width = vim.api.nvim_win_get_width(vim.api.nvim_get_current_win())
		self.width = math.min(math.max(80, current_width - margin_side * 2), 100)
		self.win = vim.api.nvim_open_win(
			self.buf,
			false,
			{ relative = "win", row = top, col = margin_side, width = self.width, height = 1, style = "minimal" }
		)
		self.inp_win = vim.api.nvim_open_win(
			self.inp,
			true,
			{ relative = "win", row = top - 1, col = margin_side, width = self.width, height = 1, style = "minimal" }
		)

		vim.api.nvim_buf_set_option(self.buf, "filetype", "sylph")
		vim.api.nvim_buf_set_option(self.inp, "filetype", "sylph")
		vim.api.nvim_buf_set_option(self.inp, "buftype", "prompt")
		vim.api.nvim_buf_set_option(self.inp, "bufhidden", "wipe")
		vim.api.nvim_buf_set_option(self.buf, "bufhidden", "wipe")
		vim.api.nvim_call_function("prompt_setprompt", { self.inp, "" })
		vim.api.nvim_command("startinsert!")

		vim.api.nvim_buf_attach(self.inp, false, {
			on_lines = function(_, _, _, f, l)
				self:on_input(f, l)
			end,
		})

		vim.api.nvim_buf_set_keymap(
			self.inp,
			"i",
			"<esc>",
			"<C-[>:lua sylph:close_window()<CR>",
			{ nowait = true, silent = true }
		)
		vim.api.nvim_buf_set_keymap(
			self.inp,
			"i",
			"<CR>",
			"<C-o>:lua sylph:enter()<CR>",
			{ nowait = true, silent = true }
		)
		vim.api.nvim_buf_set_keymap(
			self.inp,
			"i",
			"<C-J>",
			"<C-[>:lua sylph:move(1)<CR>a",
			{ nowait = true, silent = true, noremap = true }
		)
		vim.api.nvim_buf_set_keymap(
			self.inp,
			"i",
			"<C-K>",
			"<C-[>:lua sylph:move(-1)<CR>a",
			{ nowait = true, silent = true, noremap = true }
		)

		-- automatically close window when we loose focus
		vim.api.nvim_command("au WinLeave <buffer> :lua sylph:close_window()")
		-- Leave the user in normal mode when the window closes. Its confusing if
		-- the user ends up in insert mode
		vim.api.nvim_command("au WinLeave <buffer> stopi")

		-- run initial provider
		if not self.provider.run_on_input then
      sylph.timer.start("all")
      sylph.timer.start("provider")
			self.running_proc = self.provider.handler(self, self.query, function(lines)
        sylph.timer.stop("provider")
        sylph.timer.statistic("provider", "time/line", function(t) return t/#lines end)
        sylph.timer.statistic("provider", "lines", function(t) return #lines end)
				if lines ~= nil then
          self.stored_lines = lines -- prevent on input handler from running until this one has run
          sylph.timer.start("filter")
					self.running_proc = self.filter.handler(self, lines, self.query, function(matched_lines) sylph.window:lines_callback(matched_lines) end)
				end
			end)
		end
	end

	function window:on_input(firstline, lastline)
		-- First line contains the users input, so we only check for changes there
		if firstline == 0 then
      sylph.timer.start("all")
			self.query = vim.api.nvim_buf_get_lines(self.inp, 0, 1, false)[1]
			if self.provider.run_on_input then
				-- schedule query to run after 100ms
				if self.throttle ~= nil then
					self.throttle:stop()
					self.throttle:close()
					self.throttle = nil
				end
				self.throttle = vim.defer_fn(function()
					if self.running_proc ~= nil then
						self.running_proc()
						self.running_proc = nil
					end
          sylph.timer.start("provider")
					self.running_proc = self.provider.handler(self, self.query, function(lines)
            sylph.timer.stop("provider")
            sylph.timer.statistic("provider", "time/line", function(t) return t / #lines end)
            sylph.timer.statistic("provider", "lines", function(t) return #lines end)
						self.stored_lines = lines
            sylph.timer.start("filter")
						self.running_proc = self.filter.handler(self, lines, self.query, function(matched_lines) self:lines_callback(matched_lines) end)
					end)
					self.throttle = nil
				end, 100)
			else
				if self.stored_lines ~= nil then
          sylph.timer.start("filter")
					self.running_proc = self.filter.handler(self, self.stored_lines, self.query, function(matched_lines) self:lines_callback(matched_lines) end)
				end
			end
		end
	end

  function window:lines_callback(matched_lines)
    sylph.timer.stop("filter")
    -- sylph.timer.statistic("filter", "time/line", function(t) return t/#self.stored_lines end)
    self:draw(matched_lines)
  end

	function window:write_selected(selected)
		-- TODO: run in background thread
		if output_file ~= nil then
			vim.loop.fs_open(output_file, "a", 438, function(err, f)
				if err then
					print_err("Could not open output file for writing")
					return
				end
				vim.loop.fs_write(
					f,
					json.encode({
						lines = self.stored_lines,
						launched_from = self.launched_from_name,
						selected = selected,
						provider = self.provider.name,
						filter = self.filter.name,
						query = self.query,
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
      sylph.timer.start("draw")
			self.lines = lines
			self.selected = 0
			-- TODO: move to config
			local num_lines = math.min(10, #lines)
			local format = function(x)
				local width_left = math.max(self.width - 18, 0)
				-- Manual pad string because format cannot handle long string formats
				local s
				if x.line:len() > width_left then
					s = x.line:sub(-width_left, -1)
				else
					s = x.line .. string.rep(" ", width_left - x.line:len())
				end
				return string.format("%s %5.1f %5.1f %5.1f", s, x.query_score, x.frequency_score, x.context_score)
			end
			local formatted = util.map(format, { unpack(lines, 1, num_lines) })
			for i, x in ipairs(formatted) do
				if type(x) ~= "string" then
					error(string.format("Line %d in filter lines is not a string. Actual value: %s", i, vim.inspect(x)))
				end
			end
			vim.schedule(function()
				if self.buf ~= -1 then
					vim.api.nvim_buf_set_lines(self.buf, 0, -1, false, formatted)
					vim.api.nvim_win_set_height(self.win, num_lines)
					self:update_highlights()
          sylph.timer.stop("draw")
          sylph.timer.stop("all")
				end
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
		self.selected = (self.selected + dir) % #self.lines
		self:update_highlights()
	end

	function window:enter()
		if self.selected >= 0 and self.selected < #self.lines then
			-- selected is 0-indexed
			self:write_selected(self.lines[self.selected + 1])
			if self.filter.on_selected ~= nil then
				self.filter.on_selected(self.lines[self.selected + 1])
			end
			window:close()

			local loc = self.lines[self.selected + 1].location
			-- try to jump to line if it exists
			local offset = ""
			if loc.row ~= nil then
				offset = "+" .. loc.row .. " "
			end
			-- open current buffer for file if it exists
			local buf = vim.api.nvim_eval('bufnr("' .. loc.path .. '")')
			local cmd
			if buf ~= -1 then
				cmd = ":b " .. offset .. buf
			else
				cmd = ":e " .. offset .. loc.path
			end
			vim.schedule(function()
				vim.api.nvim_command(cmd)
				if loc.row ~= nil then
					vim.call("cursor", loc.row, loc.col)
					-- center cursor in window
					vim.api.nvim_command("normal! zz")
				end
			end)
		end
	end

	function window:close()
		if self.win ~= -1 then
			vim.api.nvim_win_close(self.win, true)
			self.win = -1
		end
		if self.inp_win ~= -1 then
			vim.api.nvim_win_close(self.inp_win, true)
			self.inp_win = -1
		end
		window = nil
		if self.running_proc ~= nil then
			self.running_proc()
		end
		self.buf = -1
		self.inp = -1
	end

	window:create()

	sylph.window = window
end

function sylph:close_window()
	if sylph.window ~= nil then
		sylph.window:close()
	end
end

function sylph:enter()
	sylph.window:enter()
end

function sylph:move(dir)
	sylph.window:move(dir)
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

require("rustfilter")
require("providers")

-- Let the user know that sylph has been initialized
vim.api.nvim_command("doautocmd <nomodeline> User SylphStarted")

return sylph
