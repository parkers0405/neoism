local M = {}

local notified_missing = {}
local redraw_timers = {}

local filetypes = {
  rust = { "rust" },
  python = { "python" },
  typescript = { "typescript" },
  typescriptreact = { "tsx", "typescript" },
  javascript = { "javascript" },
  javascriptreact = { "javascript" },
  go = { "go" },
  lua = { "lua" },
  json = { "json" },
  jsonc = { "json" },
  toml = { "toml" },
  yaml = { "yaml" },
  markdown = { "markdown" },
  nix = { "nix" },
  sh = { "bash" },
  bash = { "bash" },
  zsh = { "bash" },
  cs = { "c_sharp" },
  gitdiff = { "diff" },
  help = { "vimdoc" },
  ps1 = { "powershell" },
  bib = { "bibtex" },
  dosini = { "ini" },
  confini = { "ini" },
  svg = { "xml" },
  xsd = { "xml" },
  xslt = { "xml" },
  mysql = { "sql" },
  ["terraform-vars"] = { "terraform" },
}

local function notify_once(key, message, filetype)
  if notified_missing[key] then
    return
  end
  notified_missing[key] = true
  pcall(vim.rpcnotify, 1, "rio_treesitter_missing", key, filetype or "")
  require("rio.events").notify(message, "warn")
end

local function clear_query_cache()
  local get = vim.treesitter and vim.treesitter.query and vim.treesitter.query.get
  if type(get) == "table" and type(get.clear) == "function" then
    pcall(get.clear, get)
  end
end

local function register_query_predicates()
  local query = vim.treesitter and vim.treesitter.query
  if not query or not query.add_predicate then
    return
  end

  -- Some grammar queries (notably tree-sitter-nix) use nvim-treesitter's
  -- `#is-not? local` predicate. Stock nvim does not ship that predicate,
  -- so the highlighter throws from its decoration provider on every redraw.
  -- Treat it as a permissive predicate; the query still highlights correctly,
  -- and missing predicate support no longer bricks cursor movement.
  pcall(query.add_predicate, "is-not?", function()
    return true
  end, { force = true })
end

local function restore_regex_syntax(buf, filetype, previous_syntax)
  if not filetype or filetype == "" then
    return
  end

  pcall(vim.treesitter.stop, buf)
  pcall(vim.api.nvim_buf_call, buf, function()
    local syntax = previous_syntax
    if not syntax or syntax == "" then
      syntax = filetype
    end
    if vim.bo.syntax == "" then
      vim.bo.syntax = syntax
    end
  end)
end

local function has_highlight_query(lang)
  if not vim.treesitter or not vim.treesitter.query then
    return false
  end

  local ok, query = pcall(vim.treesitter.query.get, lang, "highlights")
  return ok and query ~= nil
end

local function schedule_highlight_redraw(buf)
  if redraw_timers[buf] then
    redraw_timers[buf]:stop()
    redraw_timers[buf]:close()
    redraw_timers[buf] = nil
  end

  local timer = vim.loop.new_timer()
  redraw_timers[buf] = timer
  timer:start(20, 0, vim.schedule_wrap(function()
    if redraw_timers[buf] == timer then
      redraw_timers[buf] = nil
    end
    timer:stop()
    timer:close()
    if not vim.api.nvim_buf_is_valid(buf) or not vim.api.nvim_buf_is_loaded(buf) then
      return
    end
    if vim.api.nvim__redraw then
      pcall(vim.api.nvim__redraw, { buf = buf, valid = false, flush = true })
    else
      pcall(vim.cmd, "redraw")
    end
  end))
end

function M.start_for_buffer(buf)
  buf = buf or vim.api.nvim_get_current_buf()
  if not vim.api.nvim_buf_is_valid(buf) or not vim.api.nvim_buf_is_loaded(buf) then
    return
  end
  if require("rio.large_file").is_large(buf) then
    pcall(vim.treesitter.stop, buf)
    return
  end

  local ft = vim.bo[buf].filetype
  local candidates = filetypes[ft]
  if not candidates and vim.treesitter.language and vim.treesitter.language.get_lang then
    local ok, lang = pcall(vim.treesitter.language.get_lang, ft)
    if ok and type(lang) == "string" and lang ~= "" then
      candidates = { lang }
    end
  end
  if not candidates and ft ~= "" then
    candidates = { ft }
  end
  if not candidates then
    return
  end

  if not vim.treesitter or not vim.treesitter.start then
    return
  end

  local previous_syntax = vim.bo[buf].syntax

  for _, lang in ipairs(candidates) do
    local add_ok = pcall(vim.treesitter.language.add, lang)
    if add_ok and has_highlight_query(lang) then
      local start_ok = pcall(vim.treesitter.start, buf, lang)
      if start_ok then
        -- HARD OFF: kill Vim's regex syntax engine for this buffer
        -- as soon as treesitter takes over. Previous (concurrent
        -- highlighters) path produced per-row color flips as
        -- treesitter re-painted regex's output a frame or two
        -- later. So we MUST clear regex — but if we clear it the
        -- same tick that treesitter starts, there's a one-frame
        -- gap with NO highlighting at all (treesitter's async
        -- decoration provider hasn't fired yet) and the user sees
        -- the buffer briefly flash to plain text. Defer the
        -- clear-and-redraw by a short tick so treesitter paints
        -- the FIRST highlighted frame before regex disappears
        -- underneath — net result: no flash, no flicker.
        vim.defer_fn(function()
          if not vim.api.nvim_buf_is_valid(buf) or not vim.api.nvim_buf_is_loaded(buf) then
            return
          end
          pcall(vim.api.nvim_buf_call, buf, function()
            vim.bo.syntax = ""
            vim.b.current_syntax = nil
            pcall(vim.cmd, "syntax clear")
          end)
          schedule_highlight_redraw(buf)
        end, 40)
        return
      end
      restore_regex_syntax(buf, ft, previous_syntax)
    end
  end

  restore_regex_syntax(buf, ft, previous_syntax)
  notify_once(candidates[1], "Treesitter syntax support missing for " .. candidates[1], ft)
end

function M.start_current_buffer()
  M.start_for_buffer(vim.api.nvim_get_current_buf())
end

function M.refresh_buffer(buf)
  buf = buf or vim.api.nvim_get_current_buf()
  if not vim.api.nvim_buf_is_valid(buf) or not vim.api.nvim_buf_is_loaded(buf) then
    return
  end
  if not vim.treesitter.highlighter.active[buf] then
    return
  end

  local ft = vim.bo[buf].filetype
  local candidates = filetypes[ft]
  if not candidates then
    return
  end

  if vim.api.nvim__redraw then
    pcall(vim.api.nvim__redraw, { valid = false, flush = true })
  else
    pcall(vim.cmd, "redraw")
  end
end

local function syntax_info_body()
  local buf = vim.api.nvim_get_current_buf()
  local ft = vim.bo[buf].filetype
  local candidates = filetypes[ft] or {}
  local lines = {}
  local ts_active = vim.treesitter.highlighter.active[buf] ~= nil
  local regex_opt = vim.bo[buf].syntax ~= "" and vim.bo[buf].syntax or "off"
  local regex_current = tostring(vim.b[buf].current_syntax or "none")

  table.insert(lines, "Filetype: " .. (ft ~= "" and ft or "unknown"))
  table.insert(lines, "Regex fallback: " .. (ts_active and "disabled (Treesitter active)" or regex_opt))
  table.insert(lines, "Current regex syntax: " .. regex_current)
  table.insert(lines, "Treesitter highlighter active: " .. tostring(ts_active))

  if #candidates == 0 then
    table.insert(lines, "")
    table.insert(lines, "Configured Treesitter languages: none")
  else
    table.insert(lines, "")
    table.insert(lines, "Configured Treesitter languages:")
    for _, lang in ipairs(candidates) do
      local add_ok = pcall(vim.treesitter.language.add, lang)
      local query_ok = has_highlight_query(lang)
      local parser_ok = pcall(vim.treesitter.get_parser, buf, lang)
      table.insert(lines, string.format(
        "- %s: parser=%s query=%s usable=%s",
        lang,
        tostring(add_ok and parser_ok),
        tostring(query_ok),
        tostring(add_ok and parser_ok and query_ok)
      ))
    end
  end

  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local ok, inspected = pcall(vim.inspect_pos, buf, row - 1, col)
  if ok and inspected then
    table.insert(lines, "")
    table.insert(lines, "Captures at cursor:")
    local any = false
    for _, item in ipairs(inspected.treesitter or {}) do
      table.insert(lines, "- TS " .. (item.hl_group or "") .. " @" .. (item.capture or ""))
      any = true
    end
    for _, item in ipairs(inspected.semantic_tokens or {}) do
      table.insert(lines, "- LSP semantic " .. (item.opts and item.opts.hl_group or item.hl_group or ""))
      any = true
    end
    if not any then
      table.insert(lines, "- none")
    end
  end

  return table.concat(lines, "\n")
end

function M.info()
  pcall(vim.rpcnotify, 1, "rio_modal", "Syntax Info", syntax_info_body(), "info")
end

function M.retry_all_buffers()
  notified_missing = {}
  clear_query_cache()
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(buf) then
      M.start_for_buffer(buf)
    end
  end
  pcall(function()
    vim.api.nvim__redraw({ valid = false, flush = true })
  end)
  pcall(vim.cmd, "redraw!")
end

function M.setup()
  register_query_predicates()
  vim.api.nvim_create_autocmd("FileType", {
    callback = function(args)
      M.start_for_buffer(args.buf)
    end,
  })
  vim.api.nvim_create_autocmd("BufWipeout", {
    callback = function(args)
      local timer = redraw_timers[args.buf]
      if timer then
        timer:stop()
        timer:close()
        redraw_timers[args.buf] = nil
      end
    end,
  })
  M.start_current_buffer()

  vim.api.nvim_create_user_command("SyntaxInfo", function()
    M.info()
  end, { force = true })
end

return M
