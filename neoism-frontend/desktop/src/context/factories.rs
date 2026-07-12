use crate::ansi::CursorShape;
use crate::app::ime::Ime;
use crate::app::messenger::Messenger;
#[cfg(test)]
use crate::context::manager::{ContextManager, ContextManagerConfig};
use crate::context::renderable::{Cursor, RenderableContent};
use crate::context::tab::Context;
use crate::editor::markdown::MarkdownPane;
use crate::editor::neodraw::DrawPane;
use crate::editor::notebook::NotebookPane;
use crate::event::sync::FairMutex;
use crate::layout::ContextDimension;
use crate::neoism::agent::NeoismAgentPane;
use crate::workspace::tags_view::NeoismTagsPane;
use neoism_backend::config::Shell;
use neoism_backend::event::WindowId;
use neoism_backend::performer::nvim_events::{
    Colors as NvimColors, EditorMode, HighlightTable,
};
use neoism_terminal_core::crosswords::Crosswords;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[cfg(not(target_os = "windows"))]
use std::fs;

// Global atomic counter for generating unique route IDs
pub(super) static ROUTE_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

// Global atomic counter for generating unique rich text IDs
pub(super) static RICH_TEXT_ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Generate a unique rich text ID for terminal contexts
pub fn next_rich_text_id() -> usize {
    RICH_TEXT_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

pub(super) fn ide_init_commands(theme: &str) -> Vec<String> {
    let mut commands = neoism_backend::performer::nvim::ide_init_commands();
    if !theme.trim().is_empty() {
        // Resolves custom (Mash Up Pack) themes to a full-palette
        // apply — a fresh nvim only knows the builtin palettes.
        commands.push(crate::mashup::vim_theme_command(theme));
    }
    commands
}

pub fn create_dead_context<T: neoism_backend::event::EventListener>(
    _event_proxy: T,
    _window_id: WindowId,
    route_id: usize,
    rich_text_id: usize,
    dimension: ContextDimension,
) -> Context<T> {
    let terminal = Crosswords::new(
        dimension,
        CursorShape::Block,
        neoism_backend::TerminalId::from(route_id),
        // Dead context never sees new input — no scrollback needed.
        0,
    );
    let terminal: Arc<FairMutex<Crosswords>> = Arc::new(FairMutex::new(terminal));
    let (sender, _receiver) = corcovado::channel::channel();

    Context {
        route_id,
        #[cfg(not(target_os = "windows"))]
        main_fd: Arc::new(-1),
        #[cfg(not(target_os = "windows"))]
        shell_pid: 1,
        messenger: Messenger::new(sender),
        renderable_content: RenderableContent::new(Cursor::default()),
        terminal,
        terminal_input: crate::terminal::blocks::TerminalInputBuffer::default(),
        terminal_shell_kind: crate::terminal::blocks::TerminalShellKind::Unknown,
        rich_text_id,
        dimension,
        pending_terminal_resize: false,
        pending_splash: false,
        splash_dim_stable_frames: 0,
        splash_last_dim: (0, 0),
        splash_last_cursor_row: 0,
        splash_injection: None,
        // (gap_cells_h / menu_cells_h live inside the optional
        // SplashInjection — nothing to seed here.)
        ime: Ime::new(),
        remote_pty: None,
        _io_thread: None,
        editor: None,
        editor_redraw_rx: None,
        editor_daemon_messages: Default::default(),
        editor_hl_table: HighlightTable::new(),
        editor_default_colors: NvimColors::default(),
        editor_mode: EditorMode::default(),
        editor_pending_scroll_lines: 0,
        editor_predicted_cells: Vec::new(),
        editor_pending_grid_scroll_lines: 0,
        editor_scroll_reset_pending: false,
        editor_viewport_topline: 0,
        editor_presence_line: 0,
        editor_presence_col: 0,
        editor_textoff: 0,
        editor_viewport_botline: 0,
        editor_viewport_line_count: 0,
        editor_grid_id: None,
        editor_cursor_line: 0,
        editor_total_lines: 0,
        editor_pending_keys: String::new(),
        editor_pending_elastic_lines: 0,
        editor_popup_menu: None,
        editor_lsp_status: None,
        editor_lsp_action_result: None,
        editor_lsp_action_result_modal_seen: true,
        editor_lsp_completion: None,
        editor_lsp_completion_seq: 0,
        editor_lsp_hover: None,
        editor_lsp_hover_seq: 0,
        editor_lsp_hover_cell: None,
        editor_buf_modified: Default::default(),
        editor_buf_enter: Default::default(),
        editor_notifications: Default::default(),
        editor_yank_flashes: Default::default(),
        editor_diagnostics: None,
        attached_lsps: Vec::new(),
        lsp_snapshot: None,
        lsp_messages: std::collections::BTreeMap::new(),
        editor_path: None,
        markdown: None,
        draw: None,
        notebook: None,
        neoism_agent: None,
        neoism_tags: None,
        neoism_extensions: None,
    }
}

pub fn create_markdown_context<T: neoism_backend::event::EventListener>(
    event_proxy: T,
    window_id: WindowId,
    rich_text_id: usize,
    dimension: ContextDimension,
    path: PathBuf,
) -> Context<T> {
    let route_id = ROUTE_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut context =
        create_dead_context(event_proxy, window_id, route_id, rich_text_id, dimension);
    context.markdown = Some(MarkdownPane::load(path));
    context
}

pub fn create_draw_context<T: neoism_backend::event::EventListener>(
    event_proxy: T,
    window_id: WindowId,
    rich_text_id: usize,
    dimension: ContextDimension,
    path: PathBuf,
) -> Context<T> {
    let route_id = ROUTE_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut context =
        create_dead_context(event_proxy, window_id, route_id, rich_text_id, dimension);
    context.draw = Some(DrawPane::load(path));
    context
}

pub fn create_notebook_context<T: neoism_backend::event::EventListener>(
    event_proxy: T,
    window_id: WindowId,
    rich_text_id: usize,
    dimension: ContextDimension,
    path: PathBuf,
) -> Context<T> {
    let route_id = ROUTE_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut context =
        create_dead_context(event_proxy, window_id, route_id, rich_text_id, dimension);
    context.notebook = Some(NotebookPane::load(path));
    context
}

pub fn create_neoism_agent_context<T: neoism_backend::event::EventListener>(
    event_proxy: T,
    window_id: WindowId,
    rich_text_id: usize,
    dimension: ContextDimension,
    directory: Option<String>,
) -> Context<T> {
    let route_id = ROUTE_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut context =
        create_dead_context(event_proxy, window_id, route_id, rich_text_id, dimension);
    context.neoism_agent = Some(NeoismAgentPane::with_directory(directory));
    context
}

pub fn create_neoism_tags_context<T: neoism_backend::event::EventListener>(
    event_proxy: T,
    window_id: WindowId,
    rich_text_id: usize,
    dimension: ContextDimension,
    path: PathBuf,
    workspace_root: PathBuf,
) -> Context<T> {
    let route_id = ROUTE_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut context =
        create_dead_context(event_proxy, window_id, route_id, rich_text_id, dimension);
    context.neoism_tags = Some(NeoismTagsPane::new(path, workspace_root));
    context
}

pub fn create_neoism_extensions_context<T: neoism_backend::event::EventListener>(
    event_proxy: T,
    window_id: WindowId,
    rich_text_id: usize,
    dimension: ContextDimension,
) -> Context<T> {
    let route_id = ROUTE_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut context =
        create_dead_context(event_proxy, window_id, route_id, rich_text_id, dimension);
    context.neoism_extensions =
        Some(crate::workspace::extensions::NeoismExtensionsPane::new());
    context
}

#[cfg(not(target_os = "windows"))]
pub(super) fn neoism_block_shell_for_spawn(
    shell: &Shell,
    route_id: usize,
) -> Option<Shell> {
    let name = std::path::Path::new(&shell.program)
        .file_name()
        .and_then(|name| name.to_str())?;
    let dir = std::env::temp_dir()
        .join(format!("neoism-shell-{}-{route_id}", std::process::id()));
    fs::create_dir_all(&dir).ok()?;
    let zsh_rc_dir = dir.join("zsh");
    fs::create_dir_all(&zsh_rc_dir).ok()?;
    let zsh_rc = zsh_rc_dir.join(".zshrc");
    let bash_rc = dir.join("bashrc");
    let fish_rc = dir.join("neoism.fish");
    let zsh_dir = zsh_rc_dir.display();
    let bash_rc_path = bash_rc.display();
    let fish_rc_path = fish_rc.display();
    let sh_subshell_functions = format!(
        r#"
__neoism_bashrc="{bash_rc_path}"
__neoism_zdotdir="{zsh_dir}"
__neoism_fish_init="{fish_rc_path}"
bash() {{
  if [ "$#" -eq 0 ]; then
    command bash --rcfile "$__neoism_bashrc" -i
  else
    command bash "$@"
  fi
}}
zsh() {{
  if [ "$#" -eq 0 ]; then
    ZDOTDIR="$__neoism_zdotdir" command zsh -i
  else
    command zsh "$@"
  fi
}}
fish() {{
  if [ "$#" -eq 0 ]; then
    command fish --init-command "source $__neoism_fish_init" -i
  else
    command fish "$@"
  fi
}}
nix-shell() {{
  local __neoism_has_command=0
  local __neoism_arg
  for __neoism_arg in "$@"; do
    case "$__neoism_arg" in
      --command|--run) __neoism_has_command=1 ;;
    esac
  done
  if [ "$__neoism_has_command" = 0 ]; then
    command nix-shell "$@" --command "bash --rcfile '$__neoism_bashrc' -i"
  else
    command nix-shell "$@"
  fi
}}
"#
    );
    let fish_subshell_functions = format!(
        r#"
function bash
  if test (count $argv) -eq 0
    command bash --rcfile "{bash_rc_path}" -i
  else
    command bash $argv
  end
end
function zsh
  if test (count $argv) -eq 0
    env ZDOTDIR="{zsh_dir}" zsh -i
  else
    command zsh $argv
  end
end
function fish
  if test (count $argv) -eq 0
    command fish --init-command "source {fish_rc_path}" -i
  else
    command fish $argv
  end
end
function nix-shell
  set -l __neoism_has_command 0
  for __neoism_arg in $argv
    switch $__neoism_arg
      case --command --run
        set __neoism_has_command 1
    end
  end
  if test $__neoism_has_command -eq 0
    command nix-shell $argv --command "bash --rcfile '{bash_rc_path}' -i"
  else
    command nix-shell $argv
  end
end
"#
    );

    let zsh_script = format!(
        r#"if [ -r "$HOME/.zshrc" ]; then
  source "$HOME/.zshrc"
fi
__neoism_precmd() {{
  local __neoism_status=$?
  printf '\033]7;file://%s%s\007' "$HOST" "$PWD"
  printf '\033]133;D;%d\007' "$__neoism_status"
}}
__neoism_preexec() {{
  printf '\033]133;C\007'
}}
typeset -ga precmd_functions
typeset -ga preexec_functions
precmd_functions=(${{precmd_functions:#__neoism_precmd}} __neoism_precmd)
preexec_functions=(${{preexec_functions:#__neoism_preexec}} __neoism_preexec)
bindkey '^P' kill-buffer
PROMPT=$'%{{\033]133;A\007%}}%{{\033]133;B\007%}}'
RPROMPT=''
{sh_subshell_functions}
"#
    );
    let bash_script = format!(
        r#"if [ -r "$HOME/.bashrc" ]; then
  . "$HOME/.bashrc"
fi
__neoism_hidden_ps1=$'\001\033]133;A\007\002\001\033]133;B\007\002'
__neoism_preexec() {{
  [ "${{__neoism_in_prompt:-0}}" = 1 ] && return
  case "$BASH_COMMAND" in
    __neoism_prompt_command*|PS1=*) return ;;
  esac
  printf '\033]133;C\007'
}}
__neoism_prompt_command() {{
  local __neoism_status=$?
  __neoism_in_prompt=1
  if [ -n "${{__neoism_saved_prompt_command:-}}" ]; then
    eval "$__neoism_saved_prompt_command"
  fi
  PS1="$__neoism_hidden_ps1"
  __neoism_in_prompt=0
  printf '\033]7;file://%s%s\007' "${{HOSTNAME:-localhost}}" "$PWD"
  printf '\033]133;D;%d\007' "$__neoism_status"
}}
__neoism_saved_prompt_command=${{PROMPT_COMMAND:-}}
bind '"\C-p": kill-whole-line'
PROMPT_COMMAND=__neoism_prompt_command
PS1="$__neoism_hidden_ps1"
{sh_subshell_functions}
trap '__neoism_preexec' DEBUG
"#
    );
    let fish_script = format!(
        r#"function __neoism_preexec --on-event fish_preexec
  printf '\e]133;C\a'
end
function __neoism_postexec --on-event fish_postexec
  set -l __neoism_status $status
  printf '\e]133;D;%d\a' $__neoism_status
end
function fish_prompt
  printf '\e]7;file://%s%s\a' (hostname) "$PWD"
  printf '\e]133;A\a\e]133;B\a'
end
bind \cp 'commandline ""'
{fish_subshell_functions}
"#
    );
    fs::write(&zsh_rc, zsh_script).ok()?;
    fs::write(&bash_rc, bash_script).ok()?;
    fs::write(&fish_rc, fish_script).ok()?;

    match name {
        "zsh" => {
            let mut args = vec![
                format!("ZDOTDIR={}", zsh_rc_dir.display()),
                shell.program.clone(),
            ];
            if shell.args.iter().any(|arg| arg == "--login" || arg == "-l") {
                args.push("-l".to_string());
            }
            args.push("-i".to_string());
            Some(Shell {
                program: "env".to_string(),
                args,
            })
        }
        "bash" => Some(Shell {
            program: shell.program.clone(),
            args: vec![
                "--rcfile".to_string(),
                bash_rc.display().to_string(),
                "-i".to_string(),
            ],
        }),
        "fish" => Some(Shell {
            program: shell.program.clone(),
            args: vec![
                "--init-command".to_string(),
                format!("source {}", fish_rc.display()),
                "-i".to_string(),
            ],
        }),
        _ => None,
    }
}

#[cfg(test)]
pub fn create_mock_context<
    T: neoism_backend::event::EventListener + Clone + std::marker::Send + Sync + 'static,
>(
    event_proxy: T,
    window_id: WindowId,
    rich_text_id: usize,
    dimension: ContextDimension,
) -> Context<T> {
    let config = ContextManagerConfig {
        #[cfg(not(target_os = "windows"))]
        use_fork: true,
        working_dir: None,
        shell: Shell {
            program: std::env::var("SHELL").unwrap_or("bash".to_string()),
            args: vec![],
        },
        spawn_performer: false,
        is_native: false,
        should_update_title_extra: false,
        cwd: false,
        ..ContextManagerConfig::default()
    };
    ContextManager::create_context(
        (&Cursor::default(), false),
        event_proxy.clone(),
        window_id,
        rich_text_id,
        dimension,
        &config,
        None,
    )
    .unwrap()
}

pub(super) fn resolve_editor_file_and_cwd(
    file: PathBuf,
    cwd: Option<PathBuf>,
) -> (PathBuf, Option<PathBuf>) {
    let cwd = cwd.map(|p| p.canonicalize().unwrap_or(p));
    let file = if file.is_absolute() {
        file.canonicalize().unwrap_or(file)
    } else if let Some(root) = cwd.as_ref() {
        let joined = root.join(&file);
        joined.canonicalize().unwrap_or(joined)
    } else {
        file.canonicalize()
            .or_else(|_| std::env::current_dir().map(|cwd| cwd.join(&file)))
            .unwrap_or(file)
    };
    let cwd = cwd
        .or_else(|| std::env::current_dir().ok())
        .or_else(|| file.parent().map(|p| p.to_path_buf()));
    (file, cwd)
}

pub fn process_open_url(
    mut shell: Shell,
    mut working_dir: Option<String>,
    editor: Shell,
    open_url: Option<&str>,
) -> (Shell, Option<String>) {
    if open_url.is_none() {
        return (shell, working_dir);
    }

    if let Ok(url) = url::Url::parse(open_url.unwrap_or_default()) {
        if let Ok(path_buf) = url.to_file_path() {
            if path_buf.exists() {
                if path_buf.is_file() {
                    let mut args = editor.args;
                    args.push(path_buf.display().to_string());
                    shell = Shell {
                        program: editor.program,
                        args,
                    }
                } else if path_buf.is_dir() {
                    working_dir = Some(path_buf.display().to_string());
                }
            }
        }
    }

    (shell, working_dir)
}
