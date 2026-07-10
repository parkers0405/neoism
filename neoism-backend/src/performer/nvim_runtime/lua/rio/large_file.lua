-- Central large-file policy for the embedded editor. Expensive features must
-- consult this module before materializing or parsing the entire buffer.

local M = {}

M.max_bytes = 2 * 1024 * 1024
M.max_lines = 100000

local notified = {}

local function file_size(buf)
  local path = vim.api.nvim_buf_get_name(buf)
  if path ~= "" then
    local size = tonumber(vim.fn.getfsize(path)) or -1
    if size >= 0 then
      return size
    end
  end

  local line_count = vim.api.nvim_buf_line_count(buf)
  local ok, offset = pcall(vim.api.nvim_buf_get_offset, buf, line_count)
  if ok and type(offset) == "number" and offset >= 0 then
    return offset
  end
  return 0
end

function M.is_large(buf)
  buf = buf or vim.api.nvim_get_current_buf()
  if not vim.api.nvim_buf_is_valid(buf) then
    return false
  end
  -- `true` is sticky for the buffer lifetime; a false result is cheap to
  -- re-evaluate so a file that grows past the threshold while editing still
  -- enters large-file mode.
  if vim.b[buf].neoism_large_file == true then
    return true
  end
  local large = file_size(buf) > M.max_bytes
    or vim.api.nvim_buf_line_count(buf) > M.max_lines
  vim.b[buf].neoism_large_file = large
  return large
end

function M.apply(buf)
  buf = buf or vim.api.nvim_get_current_buf()
  if not vim.api.nvim_buf_is_valid(buf) or vim.bo[buf].buftype ~= "" then
    return false
  end
  -- Re-evaluate after BufReadPost: BufReadPre may only know the on-disk size,
  -- while a newly-created/modified buffer is measured from its line offsets.
  vim.b[buf].neoism_large_file = nil
  if not M.is_large(buf) then
    return false
  end

  pcall(vim.treesitter.stop, buf)
  pcall(vim.api.nvim_buf_call, buf, function()
    vim.bo.syntax = ""
    vim.b.current_syntax = nil
    vim.bo.swapfile = false
    vim.bo.undofile = false
    vim.bo.foldmethod = "manual"
    pcall(vim.cmd, "syntax clear")
  end)

  if not notified[buf] then
    notified[buf] = true
    vim.schedule(function()
      pcall(vim.rpcnotify, 1, "rio_notify",
        "Large-file mode: syntax tree, change diff, minimap sampling, and LSP are disabled",
        "info")
    end)
  end
  return true
end

function M.setup()
  local group = vim.api.nvim_create_augroup("RioLargeFile", { clear = true })
  vim.api.nvim_create_autocmd({ "BufReadPre", "BufReadPost", "FileType", "BufEnter" }, {
    group = group,
    callback = function(args)
      M.apply(args.buf)
    end,
  })
  vim.api.nvim_create_autocmd("BufWipeout", {
    group = group,
    callback = function(args)
      notified[args.buf] = nil
    end,
  })
  M.apply(vim.api.nvim_get_current_buf())
end

return M
