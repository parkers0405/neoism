use super::*;

/// Build the tokio Command that launches nvim.
pub(crate) fn build_nvim_command(config: &NvimSpawnConfig) -> TokioCommand {
    let bin = config
        .nvim_binary
        .clone()
        .unwrap_or_else(|| PathBuf::from("nvim"));
    let mut cmd = TokioCommand::new(bin);
    // `--clean` skips the user's init.lua / plugins (NvimTree, lualine,
    // etc.) — our chrome owns the file tree, tabs, and statusline, so
    // we want a bare nvim acting purely as the buffer editor.
    cmd.arg("--clean");
    cmd.arg("--embed");
    if let Some(cwd) = &config.cwd {
        cmd.current_dir(cwd);
    }
    // Put nvim in its OWN process group (pgid == nvim's pid). nvim
    // launches its LSP servers (rust-analyzer, tsserver, …) via libuv
    // jobs that inherit this group, so on shutdown a single
    // `kill(-pgid, SIGKILL)` reaps the entire subtree at once. Without
    // this, killing nvim alone reparented the language servers to init,
    // where idle multi-GB rust-analyzers piled up in swap and dragged
    // down every subsequent window/app launch. Piped stdio (no
    // controlling tty) means the new group raises no SIGTTOU/SIGTTIN
    // concerns.
    #[cfg(unix)]
    cmd.process_group(0);
    prepend_lsp_path(&mut cmd);
    cmd
}

/// Root directory for managed Treesitter scratch state (parser source
/// clones, build artefacts). LSP binaries used to live under
/// `<root>/bin` etc.; that scaffolding is gone — the `neoism_extensions`
/// install runner now drops LSPs into `~/.local/share/neoism/extensions/`
/// and `lsp.lua`'s `managed_bin` map maps `cmd[1]` to the absolute path,
/// so embedded nvim doesn't need PATH games to find them.
pub fn rio_lsp_root_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("rio")
        .join("lsp")
}

/// Prepend the neoism extensions bin dir to embedded nvim's PATH so
/// user-launched shell commands (e.g. inside a buffer's `:!` or a
/// terminal pane) discover managed binaries. LSP resolution itself goes
/// through `managed_bin_map`, not PATH — keeping this is purely a
/// quality-of-life fallback.
fn prepend_lsp_path(cmd: &mut TokioCommand) {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(data) = dirs::data_dir() {
        paths.push(data.join("neoism").join("extensions").join("bin"));
    }
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }

    if let Ok(path) = std::env::join_paths(paths) {
        cmd.env("PATH", path);
    }
}

/// Build the rpc command that opens `path` in the current window.
/// We hand off to `vim.cmd.edit(<lua string>)` instead of formatting
/// `:edit '...'` ourselves — vim's `:edit` parser doesn't strip
/// quotes (it'd try to open a file literally named `'foo.rs'`),
/// while the lua entry point takes the path as a normal string and
/// applies fnameescape semantics for us.
pub fn vim_edit_command(path: &str) -> String {
    format!(r#"lua vim.cmd.edit({})"#, lua_string_literal(path))
}

/// Select `path` without re-running `:edit` when nvim already has the
/// buffer loaded. This keeps Rust-side tab activation idempotent:
/// returning from a terminal/editor tab to the same file should not
/// trigger a BufEnter/FileType/LSP churn cycle just to redisplay an
/// already-current buffer.
pub fn vim_select_file_command(path: &str) -> String {
    let path = lua_string_literal(path);
    format!(
        r#"lua local ok, err = pcall(function() vim.opt.laststatus = 0; vim.opt.showtabline = 0; vim.opt.winbar = ''; vim.opt.cmdheight = 0; local path = {path}; vim.o.swapfile = false; local target = vim.fn.fnamemodify(path, ":p"); vim.o.hidden = true; local current = vim.api.nvim_buf_get_name(0); if current ~= "" and vim.fn.fnamemodify(current, ":p") == target then return end; for _, buf in ipairs(vim.api.nvim_list_bufs()) do if vim.api.nvim_buf_is_loaded(buf) then local name = vim.api.nvim_buf_get_name(buf); if name ~= "" and vim.fn.fnamemodify(name, ":p") == target then vim.api.nvim_set_current_buf(buf); return end end end; vim.cmd.edit({{ args = {{ target }} }}) end); if ok then pcall(vim.cmd, 'redraw!') else vim.rpcnotify(1, "rio_modal", "File Open Failed", tostring(err), "error") end"#
    )
}

pub fn vim_cd_command(path: &str) -> String {
    format!(r#"lua vim.cmd.cd({})"#, lua_string_literal(path))
}

/// Build a `:bwipeout` for `path` using the same lua wrapper so it
/// stays robust against shell-style quoting bugs.
pub fn vim_bwipeout_command(path: &str) -> String {
    format!(
        r#"lua pcall(function() vim.cmd.bwipeout({{ args = {{ {} }}, bang = true }}) end)"#,
        lua_string_literal(path)
    )
}

pub fn vim_scratch_init_command(scratch_id: usize) -> String {
    format!(
        "lua pcall(function() vim.b.rio_scratch_id = {scratch_id}; vim.bo.buflisted = true end)"
    )
}

pub fn vim_scratch_new_command(scratch_id: usize) -> String {
    format!(
        "lua pcall(function() vim.cmd.enew(); vim.b.rio_scratch_id = {scratch_id}; vim.bo.buflisted = true end)"
    )
}

pub fn vim_scratch_select_command(scratch_id: usize) -> String {
    format!(
        r#"lua local ok = pcall(function()
  vim.opt.laststatus = 0
  vim.opt.showtabline = 0
  vim.opt.winbar = ''
  vim.opt.cmdheight = 0
  for _, bufnr in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(bufnr) then
      local ok, id = pcall(vim.api.nvim_buf_get_var, bufnr, 'rio_scratch_id')
      if ok and id == {scratch_id} then
        vim.api.nvim_set_current_buf(bufnr)
        return
      end
    end
  end
  vim.cmd.enew()
  vim.b.rio_scratch_id = {scratch_id}
  vim.bo.buflisted = true
end); if ok then pcall(vim.cmd, 'redraw!') end"#
    )
}

pub fn vim_scratch_delete_command(scratch_id: usize) -> String {
    format!(
        r#"lua pcall(function()
  for _, bufnr in ipairs(vim.api.nvim_list_bufs()) do
    local ok, id = pcall(vim.api.nvim_buf_get_var, bufnr, 'rio_scratch_id')
    if ok and id == {scratch_id} then
      pcall(vim.api.nvim_buf_delete, bufnr, {{ force = true }})
      return
    end
  end
end)"#
    )
}

/// Quote `s` as a Lua double-quoted string literal — escape `\` and
/// `"`. Sufficient for filesystem paths since we don't pass control
/// characters through this path.
pub fn lua_string_literal(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

struct RuntimeFile {
    relative_path: &'static str,
    contents: &'static str,
}

macro_rules! runtime_file {
    ($relative_path:literal) => {
        RuntimeFile {
            relative_path: $relative_path,
            contents: include_str!(concat!("../nvim_runtime/", $relative_path)),
        }
    };
}

const RIO_NVIM_RUNTIME_FILES: &[RuntimeFile] = &[
    runtime_file!("lua/rio/init.lua"),
    runtime_file!("lua/rio/options.lua"),
    runtime_file!("lua/rio/events.lua"),
    runtime_file!("lua/rio/theme.lua"),
    runtime_file!("lua/rio/treesitter.lua"),
    runtime_file!("lua/rio/clipboard.lua"),
    runtime_file!("lua/rio/completion.lua"),
    runtime_file!("lua/rio/command.lua"),
    runtime_file!("lua/rio/changesigns.lua"),
    runtime_file!("lua/rio/search.lua"),
    runtime_file!("lua/rio/minimap.lua"),
];

pub fn rio_nvim_runtime_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("rio")
        .join("nvim-runtime")
}

pub fn rio_nvim_parser_dir() -> PathBuf {
    rio_nvim_runtime_dir().join("parser")
}

fn write_runtime_file(root: &Path, file: &RuntimeFile) -> io::Result<()> {
    let path = root.join(file.relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let desired = file.contents.as_bytes();
    match fs::read(&path) {
        Ok(existing) if existing == desired => return Ok(()),
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    fs::write(path, desired)
}

fn prepare_rio_nvim_runtime() -> io::Result<PathBuf> {
    let root = rio_nvim_runtime_dir();
    fs::create_dir_all(&root)?;
    for file in RIO_NVIM_RUNTIME_FILES {
        write_runtime_file(&root, file)?;
    }
    Ok(root)
}

/// IDE-mode init for embedded nvim. We still start with `--clean`, then
/// prepend Rio's managed runtime and run only our owned Lua modules.
pub fn ide_init_commands() -> Vec<String> {
    let runtime_dir = match prepare_rio_nvim_runtime() {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(target: "neoism_backend::nvim", "failed to prepare Rio nvim runtime: {err}");
            return Vec::new();
        }
    };

    let runtime_dir = lua_string_literal(&runtime_dir.display().to_string());
    vec![
        format!(
            "lua vim.opt.runtimepath:prepend({0}); vim.opt.packpath:prepend({0})",
            runtime_dir
        ),
        String::from(
            "lua local ok, rio = pcall(require, 'rio'); if ok and rio.setup then local setup_ok, err = pcall(rio.setup); if not setup_ok then vim.api.nvim_err_writeln(tostring(err)) end else vim.api.nvim_err_writeln(tostring(rio)) end",
        ),
    ]
}

pub fn vim_copy_active_command() -> String {
    String::from("lua pcall(function() require('rio.clipboard').copy_active() end)")
}

pub fn vim_paste_command(text: &str) -> String {
    format!(
        "lua pcall(function() require('rio.clipboard').paste({}) end)",
        lua_string_literal(text)
    )
}

pub fn vim_run_ex_command(cmd: &str) -> String {
    format!(
        "lua pcall(function() require('rio.command').run({}) end)",
        lua_string_literal(cmd)
    )
}

pub fn vim_treesitter_retry_command() -> String {
    String::from(
        "lua pcall(function() require('rio.treesitter').retry_all_buffers() end)",
    )
}

pub fn vim_apply_theme_command(name: &str) -> String {
    format!(
        "lua pcall(function() require('rio.theme').apply({}) end)",
        lua_string_literal(name)
    )
}

/// `/`-palette: ask the lua side to enumerate matches for the given
/// query in the current buffer. Lua replies via `rio_search_matches`.
pub fn vim_search_query_command(query: &str) -> String {
    format!(
        "lua pcall(function() require('rio.search').search({}) end)",
        lua_string_literal(query)
    )
}

/// `/`-palette: preview a row from the dropdown — moves the cursor
/// and adds a temporary match highlight so the user sees the match
/// in the buffer behind the popup.
pub fn vim_search_preview_command(lnum: u64, col: u64, query: &str) -> String {
    format!(
        "lua pcall(function() require('rio.search').preview({lnum}, {col}, {}) end)",
        lua_string_literal(query)
    )
}

/// `/`-palette: commit a literal search. Keeping this in lua avoids raw
/// `/{query}` command parsing, so paths and regex metacharacters search
/// for the text the palette preview actually matched.
pub fn vim_search_commit_command(query: &str, location: Option<(u64, u64)>) -> String {
    let query = lua_string_literal(query);
    match location {
        Some((lnum, col)) => format!(
            "lua pcall(function() require('rio.search').commit({query}, {lnum}, {col}) end)"
        ),
        None => {
            format!("lua pcall(function() require('rio.search').commit({query}) end)")
        }
    }
}

/// Tear down both the rio-side preview highlight and nvim's native
/// hlsearch. Called when the user bails out (Esc on palette, Esc on
/// buffer in Normal).
pub fn vim_search_clear_command() -> String {
    String::from("lua pcall(function() require('rio.search').clear() end)")
}

/// Drop ONLY the rio preview overlay, leaving nvim's hlsearch alone.
/// Used right after committing `/<pattern>` so the match the user
/// just submitted stays highlighted (and `n` / `N` cycle through it).
pub fn vim_search_clear_preview_command() -> String {
    String::from("lua pcall(function() require('rio.search').clear_preview() end)")
}

pub fn vim_minimap_set_enabled_command(enabled: bool) -> String {
    format!(
        "lua pcall(function() require('rio.minimap').set_enabled({}) end)",
        if enabled { "true" } else { "false" }
    )
}

/// IDE-mode init snippets — passed as `--cmd` flags so they run
/// before any user `init.lua` (we already strip user config with
/// `--clean`, so this is purely additive).
///
/// Two responsibilities:
///   1. Fire `rio_buf_modified` rpc notifications when buffer dirty
///      state flips. Renderer's BridgeHandler picks them up and the
///      buffer-tab strip's modified dot lights up. Channel id 1 is
///      the `--embed` parent, which is always us.
///   2. Set up `vim.lsp.start({...})` for common languages on the
///      matching `FileType` event. We probe for the executable so
///      machines without the LSP installed simply don't try.
#[allow(dead_code)]
fn legacy_ide_init_commands() -> Vec<String> {
    let mut out = Vec::new();

    // Editor look-and-feel defaults. We can't rely on the user's
    // init.lua (`--clean` strips it), so the absolute essentials get
    // baked in here: line numbers, sane scrolloff, no swapfile noise,
    // 4-space soft tabs as a sensible default.
    out.push(String::from(
        r#"lua vim.opt.number = true; vim.opt.relativenumber = false; vim.opt.cursorline = false; vim.opt.scrolloff = 16; vim.opt.sidescrolloff = 8; vim.opt.signcolumn = 'yes'; vim.opt.swapfile = false; vim.opt.expandtab = true; vim.opt.tabstop = 4; vim.opt.shiftwidth = 4; vim.opt.smartindent = true; vim.opt.termguicolors = true"#,
    ));

    // Enable nvim's built-in syntax/filetype stack. Regex syntax is
    // the fallback baseline; rio.treesitter clears it per-buffer after
    // a Tree-sitter highlighter successfully starts.
    out.push(String::from(
        r#"lua pcall(function() vim.cmd('filetype plugin indent on'); vim.cmd('syntax enable') end)"#,
    ));

    // Suppress nvim's native chrome — Rust owns the statusline,
    // command line, and notifications surface, so nvim should not
    // paint any of them.
    //   laststatus=0 : never draw the status line
    //   showmode=false / ruler=false : drop the "-- INSERT --"
    //                  / line:col indicator that would otherwise
    //                  bleed into the cmdline area
    //   cmdheight=0  : reclaim the command/message line entirely
    //                  (Neovim 0.8+). Our palette will take over `:`,
    //                  search, and notifications.
    //   showtabline=0 / winbar='' : Rust owns buffer tabs and
    //                  breadcrumbs; nvim must never allocate a native
    //                  tabline/winbar row when multiple buffers or
    //                  tabpages exist.
    //   showcmd=false: don't echo partial keypresses to the cmdline
    //   shortmess+=aoOTIcCsF : strip every preset message we don't
    //                  want to flash through nvim's own UI before our
    //                  notifications layer is wired.
    out.push(String::from(
        r#"lua vim.opt.laststatus = 0; vim.opt.showtabline = 0; vim.opt.winbar = ''; vim.opt.showmode = false; vim.opt.ruler = false; vim.opt.cmdheight = 0; vim.opt.showcmd = false; vim.opt.shortmess:append('aoOTIcCsF')"#,
    ));

    // Replace `~` end-of-buffer markers with a blank glyph so empty
    // lines below the file content read as plain background instead of
    // nvim's leftover decoration. (#3 — fillchars suffices on its own;
    // the broader theme work is deferred until the user asks for it.)
    out.push(String::from(
        r#"lua vim.opt.fillchars:append({ eob = ' ' })"#,
    ));

    // Workspace-shell palette — match the chrome (file_tree, buffer_tabs,
    // breadcrumbs, status_line) so the editor pane reads as one
    // continuous black surface with the IDE chrome instead of a darker
    // grey rectangle floating inside it. Without these overrides nvim's
    // default `Normal` highlight is `#1c1c1c`, which paints in the
    // gutter / signcolumn / EndOfBuffer area and clashes with the
    // pure-black tree+tabs.
    //
    //   Normal/NormalNC : main editor bg+fg
    //   EndOfBuffer     : empty-line marker (now black, invisible)
    //   SignColumn/Number: gutters → black so they blend into the bg
    //   LineNr          : muted grey (matches breadcrumbs muted text)
    //   CursorLine/Nr   : single-step lift (#0a0a0a) so the active row
    //                     is barely-visible without screaming
    //   Visual          : selection band uses the canonical #1f1f1f
    //                     selection color
    //   Cursor          : white block, matches the terminal cursor
    out.push(String::from(
        r#"lua local set = vim.api.nvim_set_hl; set(0,'Normal',{bg='#000000',fg='#e8e8e8'}); set(0,'NormalNC',{bg='#000000',fg='#e8e8e8'}); set(0,'NormalFloat',{bg='#000000',fg='#e8e8e8'}); set(0,'FloatBorder',{bg='#000000',fg='#1f1f1f'}); set(0,'EndOfBuffer',{bg='#000000',fg='#000000'}); set(0,'SignColumn',{bg='#000000'}); set(0,'FoldColumn',{bg='#000000',fg='#5a5a5a'}); set(0,'LineNr',{bg='#000000',fg='#5a5a5a'}); set(0,'CursorLine',{bg='#0a0a0a'}); set(0,'CursorLineNr',{bg='#0a0a0a',fg='#e8e8e8',bold=true}); set(0,'Visual',{bg='#1f1f1f'}); set(0,'VisualNOS',{bg='#1f1f1f'}); set(0,'Search',{bg='#3d2e00',fg='#e8e8e8'}); set(0,'IncSearch',{bg='#665100',fg='#000000'}); set(0,'MatchParen',{bg='#1f1f1f',fg='#e8e8e8',bold=true}); set(0,'Pmenu',{bg='#0a0a0a',fg='#e8e8e8'}); set(0,'PmenuSel',{bg='#1f1f1f',fg='#e8e8e8'}); set(0,'PmenuSbar',{bg='#0a0a0a'}); set(0,'PmenuThumb',{bg='#1f1f1f'}); set(0,'StatusLine',{bg='#000000',fg='#e8e8e8'}); set(0,'StatusLineNC',{bg='#000000',fg='#5a5a5a'}); set(0,'WinSeparator',{bg='#000000',fg='#1f1f1f'}); set(0,'VertSplit',{bg='#000000',fg='#1f1f1f'}); set(0,'TabLine',{bg='#000000',fg='#7a7a7a'}); set(0,'TabLineSel',{bg='#1f1f1f',fg='#e8e8e8'}); set(0,'TabLineFill',{bg='#000000'}); set(0,'Cursor',{bg='#e8e8e8',fg='#000000'}); set(0,'lCursor',{bg='#e8e8e8',fg='#000000'}); set(0,'TermCursor',{bg='#e8e8e8'})"#,
    ));

    // Re-apply on ColorScheme so a `:colorscheme` command from a
    // plugin doesn't blow away our workspace palette. The autocmd
    // re-runs the same nvim_set_hl block above (factored into the
    // global helper `_rio_apply_theme` so the autocmd body stays
    // small).
    out.push(String::from(
        r#"lua _G._rio_apply_theme = function() local set = vim.api.nvim_set_hl; set(0,'Normal',{bg='#000000',fg='#e8e8e8'}); set(0,'NormalNC',{bg='#000000',fg='#e8e8e8'}); set(0,'EndOfBuffer',{bg='#000000',fg='#000000'}); set(0,'SignColumn',{bg='#000000'}); set(0,'LineNr',{bg='#000000',fg='#5a5a5a'}); set(0,'CursorLine',{bg='#0a0a0a'}); set(0,'CursorLineNr',{bg='#0a0a0a',fg='#e8e8e8',bold=true}); set(0,'Visual',{bg='#1f1f1f'}) end; vim.api.nvim_create_autocmd('ColorScheme',{callback=function() pcall(_G._rio_apply_theme) end})"#,
    ));

    // Syntax + semantic-token palette. The grid renderer receives the
    // final composed hl_attr_define colors from nvim, so richer TS/LSP
    // groups only need to be defined here; Rust just faithfully paints
    // the resulting truecolor/style ids.
    out.push(String::from(
        r#"lua _G._rio_apply_syntax_theme = function() local set = vim.api.nvim_set_hl; local groups = { Comment={fg='#6a9955',italic=true}, Constant={fg='#b5cea8'}, String={fg='#ce9178'}, Character={fg='#ce9178'}, Number={fg='#b5cea8'}, Boolean={fg='#569cd6'}, Float={fg='#b5cea8'}, Identifier={fg='#9cdcfe'}, Function={fg='#dcdcaa'}, Statement={fg='#c586c0'}, Conditional={fg='#c586c0'}, Repeat={fg='#c586c0'}, Label={fg='#c586c0'}, Operator={fg='#d4d4d4'}, Keyword={fg='#569cd6'}, Exception={fg='#c586c0'}, PreProc={fg='#c586c0'}, Include={fg='#c586c0'}, Define={fg='#c586c0'}, Macro={fg='#dcdcaa'}, PreCondit={fg='#c586c0'}, Type={fg='#4ec9b0'}, StorageClass={fg='#569cd6'}, Structure={fg='#4ec9b0'}, Typedef={fg='#4ec9b0'}, Special={fg='#d7ba7d'}, SpecialChar={fg='#d7ba7d'}, Tag={fg='#569cd6'}, Delimiter={fg='#808080'}, SpecialComment={fg='#6a9955',italic=true}, Debug={fg='#d16969'}, DiagnosticError={fg='#f44747'}, DiagnosticWarn={fg='#d7ba7d'}, DiagnosticInfo={fg='#75beff'}, DiagnosticHint={fg='#4ec9b0'}, DiagnosticUnderlineError={sp='#f44747',undercurl=true}, DiagnosticUnderlineWarn={sp='#d7ba7d',undercurl=true}, DiagnosticUnderlineInfo={sp='#75beff',undercurl=true}, DiagnosticUnderlineHint={sp='#4ec9b0',undercurl=true}, ['@variable']={fg='#e8e8e8'}, ['@variable.parameter']={fg='#9cdcfe'}, ['@variable.member']={fg='#9cdcfe'}, ['@constant']={fg='#b5cea8'}, ['@string']={fg='#ce9178'}, ['@number']={fg='#b5cea8'}, ['@boolean']={fg='#569cd6'}, ['@function']={fg='#dcdcaa'}, ['@function.method']={fg='#dcdcaa'}, ['@constructor']={fg='#4ec9b0'}, ['@keyword']={fg='#569cd6'}, ['@keyword.function']={fg='#569cd6'}, ['@keyword.return']={fg='#c586c0'}, ['@type']={fg='#4ec9b0'}, ['@type.builtin']={fg='#4ec9b0'}, ['@property']={fg='#9cdcfe'}, ['@field']={fg='#9cdcfe'}, ['@module']={fg='#4ec9b0'}, ['@operator']={fg='#d4d4d4'}, ['@punctuation.delimiter']={fg='#808080'}, ['@comment']={fg='#6a9955',italic=true}, ['@lsp.type.namespace']={fg='#4ec9b0'}, ['@lsp.type.type']={fg='#4ec9b0'}, ['@lsp.type.class']={fg='#4ec9b0'}, ['@lsp.type.enum']={fg='#4ec9b0'}, ['@lsp.type.interface']={fg='#4ec9b0'}, ['@lsp.type.struct']={fg='#4ec9b0'}, ['@lsp.type.parameter']={fg='#9cdcfe'}, ['@lsp.type.variable']={fg='#e8e8e8'}, ['@lsp.type.property']={fg='#9cdcfe'}, ['@lsp.type.enumMember']={fg='#b5cea8'}, ['@lsp.type.function']={fg='#dcdcaa'}, ['@lsp.type.method']={fg='#dcdcaa'}, ['@lsp.type.macro']={fg='#dcdcaa'}, ['@lsp.mod.deprecated']={strikethrough=true} }; for name, opts in pairs(groups) do pcall(set, 0, name, opts) end end; pcall(_G._rio_apply_syntax_theme); vim.api.nvim_create_autocmd('ColorScheme',{callback=function() pcall(_G._rio_apply_syntax_theme) end})"#,
    ));

    // Built-in LSP diagnostics + native popupmenu completion. The UI is
    // external (`ext_popupmenu=true`), so nvim owns completion behavior
    // and selection while Rio renders the menu surface in Rust.
    out.push(String::from(
        // Diagnostics: keep nvim collection/signs, but do not let nvim
        // paint inline virtual text/lines/underlines. Neoism renders
        // the visible same-row diagnostic lens in Rust chrome.
        r#"lua pcall(function() vim.opt.completeopt = { 'menuone', 'noinsert', 'noselect' }; vim.opt.pumheight = 12; for name, sign in pairs({ Error='E', Warn='W', Info='I', Hint='H' }) do pcall(vim.fn.sign_define, 'DiagnosticSign' .. name, { text = sign, texthl = 'Diagnostic' .. name, numhl = 'Diagnostic' .. name }) end; vim.diagnostic.config({ underline = false, signs = true, virtual_text = false, virtual_lines = false, update_in_insert = false, severity_sort = true, float = { border = 'rounded', source = 'if_many' } }); vim.keymap.set('i', '<C-Space>', '<C-x><C-o>', { silent = true }); vim.keymap.set('i', '<Tab>', function() if vim.fn.pumvisible() == 1 then return '<C-n>' end return '<Tab>' end, { expr = true, silent = true }); vim.keymap.set('i', '<S-Tab>', function() if vim.fn.pumvisible() == 1 then return '<C-p>' end return '<S-Tab>' end, { expr = true, silent = true }); vim.keymap.set('i', '<CR>', function() if vim.fn.pumvisible() == 1 then return '<C-y>' end return '<CR>' end, { expr = true, silent = true }) end)"#,
    ));

    out.push(String::from(
        r#"lua pcall(function() vim.api.nvim_create_autocmd('LspAttach', { callback = function(args) local client = vim.lsp.get_client_by_id(args.data.client_id); if not client then return end; local caps = client.server_capabilities or {}; if caps.completionProvider then vim.bo[args.buf].omnifunc = 'v:lua.vim.lsp.omnifunc'; if vim.lsp.completion and vim.lsp.completion.enable then pcall(vim.lsp.completion.enable, true, client.id, args.buf, { autotrigger = false }) end end end }) end)"#,
    ));

    out.push(String::from(
        r#"lua vim.api.nvim_create_autocmd('BufModifiedSet', { callback = function(args) local name = vim.api.nvim_buf_get_name(args.buf); if name == '' then return end; local mod = vim.bo[args.buf].modified; pcall(vim.rpcnotify, 1, 'rio_buf_modified', name, mod) end })"#,
    ));

    // BufEnter — fires whenever the active buffer changes (Tab cycling,
    // :bnext, :edit, our finder, etc). Lets the renderer keep the
    // chrome buffer-tabs strip and file-tree highlight in sync. Filter
    // unnamed buffers (`name == ''`) and non-file buftypes (terminals,
    // help, quickfix) so we don't litter the strip with scratch tabs.
    out.push(String::from(
        r#"lua vim.api.nvim_create_autocmd('BufEnter', { callback = function(args) local name = vim.api.nvim_buf_get_name(args.buf); if name == '' then return end; if vim.bo[args.buf].buftype ~= '' then return end; local total = vim.api.nvim_buf_line_count(args.buf); pcall(vim.rpcnotify, 1, 'rio_buf_enter', name, total) end })"#,
    ));

    // True-winbar tail for the breadcrumbs row — fires on cursor moves
    // with the current line/col plus the enclosing function/method name
    // when treesitter has a parser for the buffer's filetype. The
    // renderer appends this as a trailing segment after the file leaf so
    // breadcrumbs read like "src › renderer › buffer_tabs.rs › render ·
    // L498:C12". Wrapped in pcall so a missing parser or a lua error
    // never kills the cursor-moved path (which would feel like nvim
    // hung).
    //
    // Why debounced CursorHold + CursorMoved: line/col stays snappy
    // enough for chrome, while TS symbol lookup is kept out of the
    // per-key insert path. Large TypeScript buffers make synchronous
    // tree walks visible if we do them on every cursor notification.
    out.push(String::from(
        r#"lua _G._rio_winbar_emit = function() pcall(function() local pos = vim.api.nvim_win_get_cursor(0); local line, col = pos[1], pos[2]; local sym = ''; local ok, node = pcall(vim.treesitter.get_node); if ok and node then local cur = node; while cur do local t = cur:type(); if t == 'function_declaration' or t == 'function_definition' or t == 'method_declaration' or t == 'method_definition' or t == 'function_item' or t == 'class_declaration' or t == 'class_definition' or t == 'impl_item' or t == 'trait_item' or t == 'struct_item' or t == 'enum_item' then local nn = cur:field('name')[1] or cur:field('identifier')[1]; if nn then local txt = vim.treesitter.get_node_text(nn, 0); if type(txt) == 'string' and #txt > 0 and #txt < 80 then sym = txt; break end end end; cur = cur:parent() end end; local total = vim.api.nvim_buf_line_count(0); pcall(vim.rpcnotify, 1, 'rio_winbar', line, col + 1, sym, total) end) end; local rio_winbar_timer = nil; _G._rio_winbar_schedule = function() if rio_winbar_timer then rio_winbar_timer:stop(); rio_winbar_timer:close(); rio_winbar_timer = nil end; rio_winbar_timer = vim.defer_fn(function() rio_winbar_timer = nil; _G._rio_winbar_emit() end, 120) end; vim.api.nvim_create_autocmd({'CursorMoved','CursorHold','BufEnter'}, { callback = function() _G._rio_winbar_schedule() end }); vim.api.nvim_create_autocmd('BufWipeout', { callback = function() if rio_winbar_timer then rio_winbar_timer:stop(); rio_winbar_timer:close(); rio_winbar_timer = nil end end })"#,
    ));

    // Replace `vim.notify` so anything nvim or a plugin would normally
    // print to its message line gets routed to our chrome toasts.
    // `vim.log.levels` → 0 trace, 1 debug, 2 info, 3 warn, 4 error.
    // We collapse trace/debug/info to "info" since the chrome surface
    // only carries three severity colors.
    out.push(String::from(
        r#"lua vim.notify = function(msg, level) if not msg or msg == '' then return end; local s = 'info'; if level == 3 then s = 'warn' elseif level == 4 then s = 'error' end; pcall(vim.rpcnotify, 1, 'rio_notify', tostring(msg), s) end"#,
    ));

    // No nvim-side LSP clients. The Rust LSP engine owns every language-server
    // feature (hover/definition/references/diagnostics/completion/…), so nvim
    // runs purely as the text-editing substrate — no `vim.lsp.start`, no
    // duplicated servers, no orphaned rust-analyzers.

    out
}
