-- Rust-owned minimap bridge.
--
-- This module does not draw UI. It only streams compact, optional
-- snapshots of the current nvim buffer to the Rust renderer when Rust
-- explicitly enables the feature.

local M = {}

local enabled = false
local full_timer = nil
local viewport_timer = nil
local viewport_pending = false
local last_viewport_emit_ms = 0
local last_viewport_key = nil
local last_git_changes = {}
local autocmd_group = nil

local MAX_SAMPLED_LINES = 3000
local MAX_LINE_CHARS = 160
local FULL_DEBOUNCE_MS = 220
local INSERT_FULL_DEBOUNCE_MS = 1200
local QUIET_RETRY_MS = 300
local VIEWPORT_THROTTLE_MS = 50
local uv = vim.uv or vim.loop

local function rpc(name, ...)
  pcall(vim.rpcnotify, 1, name, ...)
end

local function close_timer()
  if full_timer then
    pcall(function()
      full_timer:stop()
      full_timer:close()
    end)
    full_timer = nil
  end
end

local function close_viewport_timer()
  if viewport_timer then
    pcall(function()
      viewport_timer:stop()
      viewport_timer:close()
    end)
    viewport_timer = nil
  end
  viewport_pending = false
end

local function now_ms()
  return uv.hrtime() / 1000000
end

local function completion_quiet()
  local ok, completion = pcall(require, "rio.completion")
  return ok and type(completion.is_suppressed) == "function" and completion.is_suppressed()
end

local function trimmed_line(line)
  line = line or ""
  line = line:gsub("\t", "  ")
  if #line > MAX_LINE_CHARS then
    line = line:sub(1, MAX_LINE_CHARS)
  end
  return line
end

local function collect_sampled_lines(buf, line_count)
  local stride = math.max(1, math.ceil(line_count / MAX_SAMPLED_LINES))
  local lines = {}

  if stride == 1 then
    local raw = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    for i, line in ipairs(raw) do
      lines[i] = trimmed_line(line)
    end
    return stride, lines
  end

  local out = 1
  for lnum = 1, line_count, stride do
    local raw = vim.api.nvim_buf_get_lines(buf, lnum - 1, lnum, false)[1]
    lines[out] = trimmed_line(raw)
    out = out + 1
  end
  return stride, lines
end

local function collect_git_changes(buf)
  local ok, changesigns = pcall(require, "rio.changesigns")
  if not ok or type(changesigns.changes) ~= "function" then
    return {}
  end
  local changes = changesigns.changes(buf)
  local out = {}
  for _, change in ipairs(changes or {}) do
    local line = tonumber(change.line)
    local kind = change.kind
    if line and line > 0 and type(kind) == "string" then
      out[#out + 1] = { line, kind }
    end
  end
  return out
end

local function emit(include_lines)
  if not enabled then
    return
  end

  local buf = vim.api.nvim_get_current_buf()
  if not vim.api.nvim_buf_is_valid(buf) or vim.bo[buf].buftype ~= "" then
    last_git_changes = {}
    rpc("rio_minimap_snapshot", "", 0, 0, 0, 0, 0, 1, false)
    return
  end
  if require("rio.large_file").is_large(buf) then
    include_lines = false
    last_git_changes = {}
  end

  local path = vim.api.nvim_buf_get_name(buf) or ""
  local line_count = math.max(1, vim.api.nvim_buf_line_count(buf))
  local cursor = vim.api.nvim_win_get_cursor(0)
  local top = tonumber(vim.fn.line("w0")) or cursor[1]
  local bottom = tonumber(vim.fn.line("w$")) or cursor[1]
  local changedtick = vim.b[buf].changedtick or 0
  local stride = 1
  local lines = false
  local git_changes = last_git_changes

  local viewport_key = table.concat({ buf, changedtick, line_count, top, bottom, cursor[1] }, ":")
  if not include_lines and viewport_key == last_viewport_key then
    return
  end
  last_viewport_key = viewport_key

  if include_lines then
    stride, lines = collect_sampled_lines(buf, line_count)
    git_changes = collect_git_changes(buf)
    last_git_changes = git_changes
  end

  rpc(
    "rio_minimap_snapshot",
    path,
    changedtick,
    line_count,
    top,
    bottom,
    cursor[1],
    stride,
    lines,
    git_changes
  )
end

local function schedule_full(delay_ms)
  if not enabled then
    return
  end
  close_timer()
  full_timer = uv.new_timer()
  full_timer:start(delay_ms or FULL_DEBOUNCE_MS, 0, vim.schedule_wrap(function()
    close_timer()
    if completion_quiet() then
      schedule_full(QUIET_RETRY_MS)
      return
    end
    emit(true)
  end))
end

local function emit_viewport_throttled()
  if not enabled then
    return
  end

  local elapsed = now_ms() - last_viewport_emit_ms
  if elapsed >= VIEWPORT_THROTTLE_MS then
    close_viewport_timer()
    last_viewport_emit_ms = now_ms()
    emit(false)
    return
  end

  viewport_pending = true
  if viewport_timer then
    return
  end

  viewport_timer = uv.new_timer()
  if not viewport_timer then
    last_viewport_emit_ms = now_ms()
    emit(false)
    return
  end

  local wait_ms = math.max(1, math.floor(VIEWPORT_THROTTLE_MS - elapsed))
  viewport_timer:start(wait_ms, 0, vim.schedule_wrap(function()
    close_viewport_timer()
    if not viewport_pending and enabled then
      last_viewport_emit_ms = now_ms()
      emit(false)
    end
  end))
end

local function clear_autocmds()
  if autocmd_group then
    pcall(vim.api.nvim_del_augroup_by_id, autocmd_group)
    autocmd_group = nil
  end
end

local function ensure_autocmds()
  if autocmd_group then
    return
  end

  autocmd_group = vim.api.nvim_create_augroup("RioMinimap", { clear = true })

  vim.api.nvim_create_autocmd({ "BufEnter", "BufWritePost", "TextChanged", "InsertLeave" }, {
    group = autocmd_group,
    callback = function()
      schedule_full(FULL_DEBOUNCE_MS)
    end,
  })

  vim.api.nvim_create_autocmd({ "TextChangedI", "TextChangedP" }, {
    group = autocmd_group,
    callback = function()
      if completion_quiet() or vim.fn.pumvisible() == 1 then
        emit_viewport_throttled()
      else
        schedule_full(INSERT_FULL_DEBOUNCE_MS)
      end
    end,
  })

  vim.api.nvim_create_autocmd({ "CursorMoved", "WinScrolled" }, {
    group = autocmd_group,
    callback = function()
      emit_viewport_throttled()
    end,
  })
end

function M.set_enabled(value)
  value = value and true or false
  if enabled == value then
    if enabled then
      schedule_full(0)
    end
    return
  end

  enabled = value
  if enabled then
    ensure_autocmds()
    last_viewport_key = nil
    schedule_full(0)
  else
    clear_autocmds()
    close_timer()
    close_viewport_timer()
    last_viewport_key = nil
    last_git_changes = {}
  end
end

function M.is_enabled()
  return enabled
end

function M.refresh()
  schedule_full(0)
end

function M.viewport()
  emit_viewport_throttled()
end

function M.setup()
  if enabled then
    ensure_autocmds()
  end
end

return M
