local M = {}

local function rpc(name, ...)
  pcall(vim.rpcnotify, 1, name, ...)
end

function M.lsp_log(event, fields)
  if vim.env.NEOISM_LSP_LOG == nil or vim.env.NEOISM_LSP_LOG == "" then
    return
  end
  local payload = "{}"
  if vim.json and vim.json.encode then
    local ok, encoded = pcall(vim.json.encode, fields or {})
    if ok and type(encoded) == "string" then
      payload = encoded
    end
  end
  rpc("rio_lsp_log", tostring(event or "unknown"), payload)
end

function M.notify(msg, level)
  if msg == nil or msg == "" then
    return
  end

  local severity = "info"
  if level == "warn" or level == "warning" or level == 3 then
    severity = "warn"
  elseif level == "error" or level == 4 then
    severity = "error"
  end

  pcall(function()
    local ok, lsp = pcall(require, "rio.lsp")
    if ok and type(lsp.record_lsp_message) == "function" then
      lsp.record_lsp_message(tostring(msg), severity)
    end
  end)

  rpc("rio_notify", tostring(msg), severity)
end

local cached_winbar_symbol = ""
local winbar_timer = nil

local function clear_winbar_timer()
  if winbar_timer then
    winbar_timer:stop()
    winbar_timer:close()
    winbar_timer = nil
  end
end

local function current_winbar_symbol()
  local symbol = ""
  pcall(function()
    if require("rio.large_file").is_large(vim.api.nvim_get_current_buf()) then
      return
    end
    local ok, node = pcall(vim.treesitter.get_node)
    if ok and node then
      local cur = node
      while cur do
        local node_type = cur:type()
        if node_type == "function_declaration"
          or node_type == "function_definition"
          or node_type == "method_declaration"
          or node_type == "method_definition"
          or node_type == "function_item"
          or node_type == "class_declaration"
          or node_type == "class_definition"
          or node_type == "impl_item"
          or node_type == "trait_item"
          or node_type == "struct_item"
          or node_type == "enum_item"
        then
          local name_node = cur:field("name")[1] or cur:field("identifier")[1]
          if name_node then
            local text = vim.treesitter.get_node_text(name_node, 0)
            if type(text) == "string" and #text > 0 and #text < 80 then
              symbol = text
              break
            end
          end
        end
        cur = cur:parent()
      end
    end
  end)
  return symbol
end

local function emit_winbar(refresh_symbol)
  pcall(function()
    local pos = vim.api.nvim_win_get_cursor(0)
    local line, col = pos[1], pos[2]

    if refresh_symbol then
      cached_winbar_symbol = current_winbar_symbol()
    end

    rpc(
      "rio_winbar",
      line,
      col + 1,
      cached_winbar_symbol,
      vim.api.nvim_buf_line_count(0)
    )
  end)
end

local winbar_last_emit_ms = 0
local winbar_pending_refresh = false

local function schedule_winbar(refresh_symbol, delay_ms)
  -- THROTTLE, not debounce: a held arrow key streams CursorMoved
  -- faster than any debounce window, so a clear-and-rearm timer never
  -- fired and the status line's cur/total pill froze until keyup.
  -- Leading-edge emit + a single trailing timer (never reset by later
  -- events) keeps it counting live while the key is held.
  winbar_pending_refresh = winbar_pending_refresh or refresh_symbol
  local wait = delay_ms or 90
  local now = (vim.uv or vim.loop).now()
  if now - winbar_last_emit_ms >= wait then
    winbar_last_emit_ms = now
    local refresh = winbar_pending_refresh
    winbar_pending_refresh = false
    clear_winbar_timer()
    emit_winbar(refresh)
    return
  end
  if winbar_timer then
    return
  end
  winbar_timer = vim.defer_fn(function()
    winbar_timer = nil
    winbar_last_emit_ms = (vim.uv or vim.loop).now()
    local refresh = winbar_pending_refresh
    winbar_pending_refresh = false
    emit_winbar(refresh)
  end, wait)
end

local function emit_cwd()
  pcall(function()
    local cwd = vim.fn.getcwd()
    if cwd ~= nil and cwd ~= "" then
      rpc("rio_cwd", cwd)
    end
  end)
end

function M.setup()
  vim.notify = function(msg, level)
    M.notify(msg, level)
  end

  -- Multiplayer convergence for CODE buffers: md files ride the CRDT
  -- hub, but each client gets its own nvim, so another client's save
  -- only reaches this session through the disk. autoread + checktime
  -- make unmodified buffers reload silently; the poll timer matters
  -- because daemon-hosted nvims never receive OS focus events, so
  -- FocusGained alone would leave an idle session stale forever.
  -- (Locally-modified buffers are left alone — last writer wins, as
  -- before.)
  vim.o.autoread = true
  vim.api.nvim_create_autocmd({ "FocusGained", "BufEnter", "CursorHold" }, {
    callback = function()
      pcall(function()
        vim.cmd("silent! checktime")
      end)
    end,
  })
  local function poll_checktime()
    vim.defer_fn(function()
      pcall(function()
        -- Skip while typing: a mid-insert reload of some OTHER
        -- window's buffer is fine, but churning checktime between
        -- keystrokes is wasted work.
        if vim.api.nvim_get_mode().mode:sub(1, 1) ~= "i" then
          vim.cmd("silent! checktime")
        end
      end)
      poll_checktime()
    end, 2000)
  end
  poll_checktime()

  emit_cwd()

  vim.api.nvim_create_autocmd("DirChanged", {
    callback = emit_cwd,
  })

  vim.api.nvim_create_autocmd("BufModifiedSet", {
    callback = function(args)
      local name = vim.api.nvim_buf_get_name(args.buf)
      if name == "" then
        return
      end
      rpc("rio_buf_modified", name, vim.bo[args.buf].modified)
    end,
  })

  vim.api.nvim_create_autocmd("BufEnter", {
    callback = function(args)
      emit_cwd()
      local name = vim.api.nvim_buf_get_name(args.buf)
      if name == "" or vim.bo[args.buf].buftype ~= "" then
        return
      end
      rpc("rio_buf_enter", name)
    end,
  })

  vim.api.nvim_create_autocmd("CursorMoved", {
    callback = function()
      schedule_winbar(false, 50)
    end,
  })

  vim.api.nvim_create_autocmd({ "CursorHold", "CursorHoldI", "BufEnter" }, {
    callback = function()
      schedule_winbar(true, 120)
    end,
  })

  vim.api.nvim_create_autocmd({ "InsertLeave", "BufLeave", "BufWipeout" }, {
    callback = function()
      clear_winbar_timer()
    end,
  })
end

return M
