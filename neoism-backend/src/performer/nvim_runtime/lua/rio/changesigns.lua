-- Lightweight gitsigns-style change indicator. Diffs the buffer's current
-- content against git HEAD when available, falling back to the last saved
-- disk contents outside git repos, and places colored signs in the signcolumn:
--   GitSignsAdd    │ green   — line added since baseline
--   GitSignsChange │ yellow  — line modified since baseline
--   GitSignsDelete ▔ red    — line(s) deleted at this position
--
-- Lives in the embedded nvim runtime so the indicators ride through the
-- same msgpack-rpc grid_line events rio paints from. No plugin install
-- required — uses only `vim.diff()` and the built-in sign API.

local M = {}

local SIGN_GROUP = "rio_changesigns"
local SIGN_ADD = "RioChangeAdd"
local SIGN_CHANGE = "RioChangeChange"
local SIGN_DELETE = "RioChangeDelete"

-- Per-buffer baseline. Map of bufnr → string (git HEAD content when
-- available, otherwise the last saved content outside git repos).
-- Buffers without an entry have never had a baseline captured (e.g.
-- scratch buffers); we just skip them.
local baselines = {}
local changes_by_buf = {}

-- Coalesce diff calls — TextChanged on a fast typist fires many times
-- per second, and a 5k-line diff isn't free. Use a real per-buffer
-- debounce so the diff runs after a short pause, not once per event-loop
-- tick while the user is still typing.
local pending = {}
local RECOMPUTE_DEBOUNCE_MS = 220
local INSERT_RECOMPUTE_DEBOUNCE_MS = 1000
local COMPLETION_RECOMPUTE_DEBOUNCE_MS = 1200

local function completion_quiet()
  local ok, completion = pcall(require, "rio.completion")
  return ok and type(completion.is_suppressed) == "function" and completion.is_suppressed()
end

local function system_text(args)
  if vim.system then
    local ok, result = pcall(function()
      return vim.system(args, { text = true }):wait()
    end)
    if not ok or not result then
      return 1, ""
    end
    return result.code or 1, result.stdout or ""
  end

  local out = vim.fn.systemlist(args)
  return vim.v.shell_error, table.concat(out, "\n")
end

local function normalize_blob_text(text)
  text = text or ""
  if text:sub(-1) == "\n" then
    text = text:sub(1, -2)
  end
  return text
end

local function git_repo_root(path)
  if not path or path == "" then
    return nil
  end
  local dir = vim.fn.fnamemodify(path, ":p:h")
  local code, out = system_text({ "git", "-C", dir, "rev-parse", "--show-toplevel" })
  if code ~= 0 then
    return nil
  end
  local root = (out or ""):gsub("%s+$", "")
  if root == "" then
    return nil
  end
  return vim.fn.fnamemodify(root, ":p")
end

local function repo_relative_path(root, path)
  root = vim.fn.fnamemodify(root, ":p")
  path = vim.fn.fnamemodify(path, ":p")
  local prefix = root
  if prefix:sub(-1) ~= "/" then
    prefix = prefix .. "/"
  end
  if path:sub(1, #prefix) ~= prefix then
    return nil
  end
  return path:sub(#prefix + 1)
end

local function git_baseline_for_path(path)
  local root = git_repo_root(path)
  if not root then
    return nil
  end
  local rel = repo_relative_path(root, path)
  if not rel or rel == "" then
    return nil
  end

  local code, out = system_text({ "git", "-C", root, "show", "HEAD:" .. rel })
  if code == 0 then
    return normalize_blob_text(out)
  end

  code, out = system_text({
    "git",
    "-C",
    root,
    "status",
    "--porcelain=v1",
    "--untracked-files=all",
    "--",
    rel,
  })
  if code == 0 and out ~= "" then
    return ""
  end
  return nil
end

local function refresh_minimap()
  local ok, minimap = pcall(require, "rio.minimap")
  if ok and type(minimap.refresh) == "function" then
    if type(minimap.is_enabled) == "function" and not minimap.is_enabled() then
      return
    end
    minimap.refresh()
  end
end

local function buffer_text(buf)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  return table.concat(lines, "\n")
end

local function clear_signs(buf)
  pcall(vim.fn.sign_unplace, SIGN_GROUP, { buffer = buf })
end

local function sign_name_for_kind(kind)
  if kind == "add" then
    return SIGN_ADD
  elseif kind == "change" then
    return SIGN_CHANGE
  elseif kind == "delete" then
    return SIGN_DELETE
  end
  return nil
end

local function kind_for_sign_name(name)
  if name == SIGN_ADD then
    return "add"
  elseif name == SIGN_CHANGE then
    return "change"
  elseif name == SIGN_DELETE then
    return "delete"
  end
  return nil
end

local function current_signs_by_line(buf)
  local by_line = {}
  local ok, placed = pcall(vim.fn.sign_getplaced, buf, { group = SIGN_GROUP })
  if not ok or type(placed) ~= "table" or type(placed[1]) ~= "table" then
    return by_line
  end

  for _, sign in ipairs(placed[1].signs or {}) do
    local line = tonumber(sign.lnum)
    local id = tonumber(sign.id)
    local kind = kind_for_sign_name(sign.name)
    if line and id and kind then
      by_line[line] = by_line[line] or {}
      by_line[line][#by_line[line] + 1] = { id = id, kind = kind }
    end
  end

  return by_line
end

local function reconcile_signs(buf, desired)
  local actual = current_signs_by_line(buf)
  local kept = {}

  for line, signs in pairs(actual) do
    local desired_kind = desired[line]
    local kept_existing = false
    for _, sign in ipairs(signs) do
      if desired_kind and sign.kind == desired_kind and not kept_existing then
        kept_existing = true
        kept[line] = true
      else
        pcall(vim.fn.sign_unplace, SIGN_GROUP, { buffer = buf, id = sign.id })
      end
    end
  end

  for line, kind in pairs(desired) do
    if not kept[line] then
      local sign_name = sign_name_for_kind(kind)
      if sign_name then
        pcall(vim.fn.sign_place, 0, SIGN_GROUP, sign_name, buf, {
          lnum = line,
          priority = 8,
        })
      end
    end
  end
end

-- Walk vim.diff's `indices` output and place one sign per affected
-- line. The format is a list of {start_a, count_a, start_b, count_b}
-- hunks where _a is baseline (deleted/changed in the old) and _b is
-- buffer (added/changed in the new). 1-based line numbers.
local function place_from_hunks(buf, hunks)
  if not hunks or #hunks == 0 then
    changes_by_buf[buf] = nil
    reconcile_signs(buf, {})
    return
  end

  local desired = {}
  local function mark(line, kind)
    line = tonumber(line)
    if line and line > 0 then
      desired[line] = kind
    end
  end

  for _, h in ipairs(hunks) do
    local _start_a, count_a, start_b, count_b =
      h[1], h[2], h[3], h[4]
    if count_b == 0 then
      -- Pure deletion — nothing to mark on the new buffer's lines, so
      -- show a delete glyph on the line BEFORE the cut so the user
      -- still sees something (mirrors gitsigns' `topdelete`).
      local at = math.max(start_b, 1)
      mark(at, "delete")
    elseif count_a == 0 then
      -- Pure addition.
      for ln = start_b, start_b + count_b - 1 do
        mark(ln, "add")
      end
    else
      -- Mixed — mark the lines in `_b` as changed. Even if `count_a >
      -- count_b` we emit "change" rather than "delete" because the
      -- user sees the mutated content right there; deletion-only
      -- hunks already took the early return above.
      for ln = start_b, start_b + count_b - 1 do
        mark(ln, "change")
      end
    end
  end

  local lines = {}
  for line in pairs(desired) do
    lines[#lines + 1] = line
  end
  table.sort(lines)

  local changes = {}
  for _, line in ipairs(lines) do
    changes[#changes + 1] = { line = line, kind = desired[line] }
  end
  changes_by_buf[buf] = changes
  reconcile_signs(buf, desired)
end

local function clear_pending(buf)
  local timer = pending[buf]
  if timer then
    timer:stop()
    timer:close()
    pending[buf] = nil
  end
end

local function recompute(buf)
  pending[buf] = nil
  if not vim.api.nvim_buf_is_valid(buf) then
    changes_by_buf[buf] = nil
    return
  end
  if require("rio.large_file").is_large(buf) then
    baselines[buf] = nil
    changes_by_buf[buf] = nil
    clear_signs(buf)
    return
  end
  local baseline = baselines[buf]
  if baseline == nil then
    -- No baseline yet → nothing to compare against. The first
    -- BufReadPost will capture one.
    changes_by_buf[buf] = nil
    clear_signs(buf)
    refresh_minimap()
    return
  end
  local current = buffer_text(buf)
  if current == baseline then
    changes_by_buf[buf] = nil
    clear_signs(buf)
    refresh_minimap()
    return
  end
  local ok, hunks = pcall(vim.diff, baseline, current, {
    result_type = "indices",
    algorithm = "histogram",
    ctxlen = 0,
  })
  if not ok or type(hunks) ~= "table" then
    changes_by_buf[buf] = nil
    clear_signs(buf)
    refresh_minimap()
    return
  end
  place_from_hunks(buf, hunks)
  refresh_minimap()
end

local function schedule_recompute(buf, delay_ms)
  if require("rio.large_file").is_large(buf) then
    clear_pending(buf)
    changes_by_buf[buf] = nil
    clear_signs(buf)
    return
  end
  clear_pending(buf)
  pending[buf] = vim.defer_fn(function()
    pending[buf] = nil
    if completion_quiet() then
      schedule_recompute(buf, COMPLETION_RECOMPUTE_DEBOUNCE_MS)
      return
    end
    recompute(buf)
  end, delay_ms or RECOMPUTE_DEBOUNCE_MS)
end

local function capture_baseline(buf, baseline)
  if not vim.api.nvim_buf_is_valid(buf) then
    return
  end
  if require("rio.large_file").is_large(buf) then
    baselines[buf] = nil
    changes_by_buf[buf] = nil
    clear_signs(buf)
    return
  end
  -- Only track real on-disk buffers — special buftypes (terminal,
  -- nofile, prompt) don't have a meaningful "saved content."
  if vim.bo[buf].buftype ~= "" then
    baselines[buf] = nil
    changes_by_buf[buf] = nil
    return
  end
  local path = vim.api.nvim_buf_get_name(buf)
  baselines[buf] = baseline or git_baseline_for_path(path) or buffer_text(buf)
  schedule_recompute(buf, 0)
end

local function capture_enter_baseline(buf)
  local path = vim.api.nvim_buf_get_name(buf)
  local git_baseline = git_baseline_for_path(path)
  if git_baseline ~= nil then
    baselines[buf] = git_baseline
    schedule_recompute(buf, 0)
    return
  end
  -- New file (path set but file doesn't exist yet on disk): treat
  -- baseline as empty so every line shows as "added" until the user
  -- writes — matches gitsigns' staged-vs-buffer behavior on untracked
  -- files. Existing buffers just read by nvim use their current text;
  -- avoid a second disk read and startup diff on BufReadPost/BufEnter.
  if path ~= "" and vim.fn.filereadable(path) == 0 then
    baselines[buf] = ""
    schedule_recompute(buf, 0)
  else
    capture_baseline(buf)
  end
end

function M.refresh()
  local buf = vim.api.nvim_get_current_buf()
  capture_baseline(buf)
end

function M.changes(buf)
  buf = buf or vim.api.nvim_get_current_buf()
  return changes_by_buf[buf] or {}
end

function M.setup()
  pcall(function()
    -- Highlight groups. The colors mirror gitsigns.nvim's defaults so
    -- the indicators read familiar to anyone coming from NvChad.
    -- Uses `pcall` because the user's colorscheme may also define
    -- these later; we just want sane fallbacks.
    local set = vim.api.nvim_set_hl
    set(0, "RioChangeAddHl", { fg = "#9fe8c3" })
    set(0, "RioChangeChangeHl", { fg = "#fbdf90" })
    set(0, "RioChangeDeleteHl", { fg = "#ef8891" })

    -- Don't link to existing GitSigns* groups — those may not exist,
    -- and even when they do they sometimes carry a `bg` from the
    -- theme that paints a colored block instead of just the glyph.

    pcall(vim.fn.sign_define, SIGN_ADD, {
      text = "│",
      texthl = "RioChangeAddHl",
      numhl = "",
    })
    pcall(vim.fn.sign_define, SIGN_CHANGE, {
      text = "│",
      texthl = "RioChangeChangeHl",
      numhl = "",
    })
    pcall(vim.fn.sign_define, SIGN_DELETE, {
      text = "▁",
      texthl = "RioChangeDeleteHl",
      numhl = "",
    })

    -- Capture baseline whenever a buffer is read from disk or written.
    -- In git repos this remains the HEAD content, so saved-but-uncommitted
    -- changes stay visible when the file is reopened.
    vim.api.nvim_create_autocmd({ "BufReadPost", "BufWritePost" }, {
      callback = function(args)
        capture_baseline(args.buf)
      end,
    })

    -- BufEnter rather than BufNew/FileType: by the time the user
    -- focuses the buffer, the file is read and the path is final.
    -- Skips work for buffers we already have a baseline for.
    vim.api.nvim_create_autocmd("BufEnter", {
      callback = function(args)
        if baselines[args.buf] == nil then
          capture_enter_baseline(args.buf)
        else
          schedule_recompute(args.buf)
        end
      end,
    })

    vim.api.nvim_create_autocmd("TextChanged", {
      callback = function(args)
        schedule_recompute(args.buf)
      end,
    })

    vim.api.nvim_create_autocmd({ "TextChangedI", "TextChangedP" }, {
      callback = function(args)
        if completion_quiet() or vim.fn.pumvisible() == 1 then
          schedule_recompute(args.buf, COMPLETION_RECOMPUTE_DEBOUNCE_MS)
        else
          schedule_recompute(args.buf, INSERT_RECOMPUTE_DEBOUNCE_MS)
        end
      end,
    })

    vim.api.nvim_create_autocmd("InsertLeave", {
      callback = function(args)
        schedule_recompute(args.buf, 80)
      end,
    })

    vim.api.nvim_create_autocmd("BufWipeout", {
      callback = function(args)
        baselines[args.buf] = nil
        changes_by_buf[args.buf] = nil
        clear_pending(args.buf)
      end,
    })

    vim.api.nvim_create_user_command("RioChangeRefresh", function()
      M.refresh()
    end, { force = true })
  end)
end

return M
