local M = {}

function M.setup()
  vim.opt.number = true
  vim.opt.relativenumber = false
  -- Disabled: the host paints an animated cursorline overlay so the
  -- highlight slides smoothly between rows on cursor move. nvim's
  -- built-in cursorline re-emits the row's bg via grid_line every
  -- time the cursor moves, which made held-arrow scroll feel
  -- "spazzy" — the bg jumped per row while content slid via the
  -- editor scroll spring.
  vim.opt.cursorline = false
  vim.opt.scrolloff = 16
  vim.opt.sidescrolloff = 8
  vim.opt.signcolumn = "yes"
  vim.opt.swapfile = false
  vim.opt.expandtab = true
  vim.opt.tabstop = 4
  vim.opt.shiftwidth = 4
  vim.opt.smartindent = true
  vim.opt.termguicolors = true
  vim.opt.mouse = "a"
  vim.opt.hidden = true
  pcall(function()
    vim.opt.clipboard = "unnamedplus"
  end)

  vim.opt.laststatus = 0
  vim.opt.showmode = false
  vim.opt.ruler = false
  pcall(function()
    vim.opt.cmdheight = 0
  end)
  -- With ext_messages attached, showcmd renders as `msg_showcmd` UI
  -- events instead of grid cells — the Rust status line shows the
  -- pending count/operator next to the mode label, so a half-typed
  -- "2d" is visible instead of looking like a hang.
  vim.opt.showcmd = true
  vim.opt.shortmess:append("aoOTIcCsF")
  vim.opt.fillchars:append({ eob = " " })

  pcall(function()
    vim.cmd("filetype plugin indent on")
    -- Regex syntax is the baseline fallback. When Treesitter starts
    -- successfully for a buffer, `rio.treesitter` clears regex syntax
    -- for that buffer so we still avoid dual-highlighter flicker.
    vim.cmd("syntax enable")
  end)
  require("rio.large_file").setup()
end

return M
