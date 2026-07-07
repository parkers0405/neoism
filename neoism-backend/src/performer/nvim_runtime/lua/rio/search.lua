-- Buffer-local `/` search service for rio's command palette.
--
-- The palette in Search mode is a Rust-rendered overlay; instead of
-- routing keystrokes into nvim's native `/`, rio asks lua for the
-- matching lines in the current buffer and renders them as a
-- selectable list. Selecting a row previews the line in the editor
-- pane (cursor + temporary match highlight) so the user can scan results
-- visually before committing. Submit writes nvim's search register
-- and moves with nvim's own search function so post-commit `n` /
-- `N` / `hlsearch` behave as usual.
--
-- Incsearch semantics (matches nvim's native `/`):
--   * `begin` snapshots the pre-search view so Esc/cancel can restore
--     the cursor + scroll exactly where the user was.
--   * `search` orders results so the FIRST (auto-selected) row is the
--     nearest match at/after the cursor, wrapping to the top — the same
--     "jump forward from here" feel as typing `/pat` natively.
--   * `preview` moves + centers the view on the previewed match AND
--     lights up every occurrence live (`hlsearch`), so the buffer
--     behind the popup updates on every keystroke.
--
-- All entry points are designed to be called via `nvim_command(lua
-- ...)` from the rust side — no rpcrequest round-trip — so latency
-- stays bounded by the regular nvim_command pipe.

local M = {}

-- Single shared namespace so re-running `preview` blows away the
-- prior selection's highlight before drawing the new one. Buffer-
-- scoped via the buf id passed to `add_highlight`.
local NS = vim.api.nvim_create_namespace("rio_search_preview")

-- Cap the result list so a query that matches every line in a 30k-
-- line buffer doesn't ship 30k entries through the rpc channel.
-- The palette only displays a window anyway.
local MAX_MATCHES = 1000

-- Pre-search view snapshot (cursor + scroll), captured when a `/`
-- session opens. `winsaveview()` restores pixel-for-pixel on cancel.
M._origin = nil

local function search_ignores_case(query)
  if not vim.o.ignorecase then
    return false
  end
  return not (vim.o.smartcase and query:find("%u"))
end

local function literal_search_pattern(query)
  local case_prefix = search_ignores_case(query) and "\\c" or "\\C"
  return case_prefix .. "\\V" .. vim.fn.escape(query, "\\")
end

-- Trim/normalise a buffer line for display in the palette: collapse
-- tabs to spaces (so the proportional font in the popup doesn't
-- mis-align), and clamp length so a 5kB line doesn't murder the rpc.
local function display_line(s)
  if not s then
    return ""
  end
  s = s:gsub("\t", "    ")
  if #s > 200 then
    s = s:sub(1, 197) .. "..."
  end
  return s
end

-- Snapshot the current view so cancel can restore it. Called from rust
-- the instant the `/` palette opens (before the user types anything),
-- so the anchor is the true pre-search position.
function M.begin()
  local buf = vim.api.nvim_get_current_buf()
  if not vim.api.nvim_buf_is_valid(buf) then
    M._origin = nil
    return
  end
  local ok, view = pcall(vim.fn.winsaveview)
  M._origin = (ok and view) or nil
end

-- Lazily snapshot the origin if `begin` was never called (defensive —
-- keeps Esc-restore working even if the palette was opened by a path
-- that didn't fire `begin`).
local function ensure_origin()
  if M._origin == nil then
    M.begin()
  end
end

-- Restore the pre-search view captured by `begin`, if any.
local function restore_origin()
  if M._origin then
    pcall(vim.fn.winrestview, M._origin)
  end
end

-- Rotate the file-ordered match list so the first entry is the nearest
-- match at/after the origin cursor, wrapping around to the top when
-- every match sits before it. This makes the auto-selected row 0 (which
-- rust previews) land on the "next" match — nvim's forward-search feel —
-- instead of always snapping to the first match in the file.
local function rotate_to_nearest(matches, origin)
  if not origin or #matches <= 1 then
    return matches
  end
  local oline = origin.lnum or 1
  local ocol = origin.col or 0 -- 0-based byte col
  local pivot = nil
  for idx, m in ipairs(matches) do
    local mline = m[1]
    local mcol0 = (m[2] or 1) - 1
    if mline > oline or (mline == oline and mcol0 >= ocol) then
      pivot = idx
      break
    end
  end
  if not pivot or pivot == 1 then
    return matches
  end
  local rotated = {}
  for i = pivot, #matches do
    rotated[#rotated + 1] = matches[i]
  end
  for i = 1, pivot - 1 do
    rotated[#rotated + 1] = matches[i]
  end
  return rotated
end

function M.search(query)
  M.clear_preview()
  ensure_origin()
  if not query or query == "" then
    -- Emptying the pattern (backspacing it all out) returns the cursor
    -- to where the search started, exactly like nvim incsearch.
    restore_origin()
    pcall(vim.cmd, "nohlsearch")
    pcall(vim.rpcnotify, 1, "rio_search_matches", {})
    return
  end

  local buf = vim.api.nvim_get_current_buf()
  if not vim.api.nvim_buf_is_valid(buf) then
    pcall(vim.rpcnotify, 1, "rio_search_matches", {})
    return
  end

  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local ignore_case = search_ignores_case(query)
  local needle = ignore_case and query:lower() or query
  local needle_len = math.max(#needle, 1)
  local matches = {}
  for i, line in ipairs(lines) do
    -- Every (non-overlapping) occurrence on the line, like nvim's
    -- hlsearch — not just the first — so the count and highlights are
    -- complete.
    local haystack = ignore_case and line:lower() or line
    local from = 1
    while true do
      local start_col = haystack:find(needle, from, true)
      if not start_col then
        break
      end
      matches[#matches + 1] = { i, start_col, display_line(line) }
      if #matches >= MAX_MATCHES then
        break
      end
      from = start_col + needle_len
    end
    if #matches >= MAX_MATCHES then
      break
    end
  end
  matches = rotate_to_nearest(matches, M._origin)
  pcall(vim.rpcnotify, 1, "rio_search_matches", matches)
end

-- Move the cursor to `lnum`, center the view, and light up every
-- occurrence of `query` live (`hlsearch`) so the buffer behind the
-- popup reads exactly like nvim's incsearch. The current match keeps a
-- brighter overlay via the `Search`-group extmark.
function M.preview(lnum, col, query)
  if not lnum or lnum <= 0 then
    return
  end
  local buf = vim.api.nvim_get_current_buf()
  if not vim.api.nvim_buf_is_valid(buf) then
    return
  end
  -- Win-set-cursor takes 1-based lnum, 0-based col. Wrap in pcall —
  -- a stale lnum (buffer changed since search ran) shouldn't kill the
  -- preview command.
  pcall(vim.api.nvim_buf_clear_namespace, buf, NS, 0, -1)
  local start_col = math.max((tonumber(col) or 1) - 1, 0)
  pcall(vim.api.nvim_win_set_cursor, 0, { lnum, start_col })
  if query and query ~= "" then
    -- Live all-match highlight, like nvim incsearch. Setting the search
    -- register + hlsearch paints every occurrence; `clear` (Esc) tears
    -- it back down, and `commit` (Enter) leaves it in place so `n`/`N`
    -- keep working.
    pcall(vim.fn.setreg, "/", literal_search_pattern(query))
    vim.o.hlsearch = true
    pcall(
      vim.api.nvim_buf_add_highlight,
      buf,
      NS,
      "IncSearch",
      lnum - 1,
      start_col,
      start_col + #query
    )
  else
    pcall(vim.api.nvim_buf_add_highlight, buf, NS, "IncSearch", lnum - 1, 0, -1)
  end
  -- `zz` centers the line in the window so a series of arrow-down
  -- presses doesn't park results at the very bottom edge.
  pcall(vim.cmd, "normal! zz")
end

-- `backward` mirrors nvim's `?`: the freeform commit jumps to the
-- previous match instead of the next one, and `v:searchforward` flips
-- so a follow-up `n` keeps walking backward (and `N` forward), exactly
-- as if the user had typed `?pattern<CR>` natively.
function M.commit(query, lnum, col, backward)
  if not query or query == "" then
    return
  end
  local buf = vim.api.nvim_get_current_buf()
  if not vim.api.nvim_buf_is_valid(buf) then
    return
  end

  local pattern = literal_search_pattern(query)
  M.clear_preview()
  -- Committing "accepts" the previewed jump; forget the origin so a
  -- later `clear` (e.g. Esc in Normal mode) doesn't yank us back to
  -- where the search started.
  M._origin = nil
  pcall(vim.fn.setreg, "/", pattern)
  vim.o.hlsearch = true

  if lnum and lnum > 0 then
    local start_col = math.max((tonumber(col) or 1) - 1, 0)
    pcall(vim.api.nvim_win_set_cursor, 0, { lnum, start_col })
    pcall(vim.cmd, "normal! zz")
    vim.v.searchforward = backward and 0 or 1
    return
  end

  local ok, found = pcall(vim.fn.search, pattern, backward and "b" or "")
  if ok and found and found > 0 then
    pcall(vim.cmd, "normal! zz")
  end
  vim.v.searchforward = backward and 0 or 1
end

-- Drop ONLY the rio-side preview highlight, leaving nvim's native
-- `@/` register and `hlsearch` alone. Called after the palette
-- commits a pattern — at that point nvim's own search highlight takes
-- over and `n`/`N` cycle through matches, so we don't want to nuke the
-- highlights the user just asked for.
function M.clear_preview()
  local buf = vim.api.nvim_get_current_buf()
  if vim.api.nvim_buf_is_valid(buf) then
    pcall(vim.api.nvim_buf_clear_namespace, buf, NS, 0, -1)
  end
end

-- Tear down BOTH the rio-side preview highlight and nvim's native
-- hlsearch, and restore the pre-search view. Called from rust when the
-- user explicitly bails out of search:
--   * Esc inside the palette without committing
--   * Esc on the editor in Normal mode (the canonical "I'm done with
--     search" gesture, mirroring `:nohlsearch`'s usual binding)
function M.clear()
  M.clear_preview()
  -- Cancel = go back exactly where the `/` started (nvim incsearch Esc).
  restore_origin()
  M._origin = nil
  pcall(vim.cmd, "nohlsearch")
end

function M.setup()
  -- No autocmds needed — every entry point is called explicitly from
  -- rust. The require() itself is enough to register the module.
end

return M
