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
use neoism_terminal_core::crosswords::Crosswords;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use std::fs;

// Global atomic counter for generating unique route IDs
pub(super) static ROUTE_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

// Global atomic counter for generating unique rich text IDs
pub(super) static RICH_TEXT_ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Generate a unique rich text ID for terminal contexts
pub fn next_rich_text_id() -> usize {
    RICH_TEXT_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
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
        // 0 = "no shell": Drop's kill guard skips it on every platform.
        shell_pid: 0,
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
        markdown: None,
        code: None,
        draw: None,
        notebook: None,
        neoism_agent: None,
        neoism_tags: None,
        neoism_extensions: None,
    }
}

pub fn create_code_context<T: neoism_backend::event::EventListener>(
    event_proxy: T,
    window_id: WindowId,
    rich_text_id: usize,
    dimension: ContextDimension,
    path: PathBuf,
) -> Context<T> {
    let route_id = ROUTE_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut context =
        create_dead_context(event_proxy, window_id, route_id, rich_text_id, dimension);
    context.code = Some(neoism_ui::editor::code::CodePane::load(path));
    context
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

/// Windows twin of the unix rc wrappers above: a `neoism_profile.ps1`
/// dot-sourced into PowerShell so every prompt is framed with the same
/// OSC 133 marks (`D;<exit>` closing the previous command, `A` before
/// the prompt text, `B` after it) plus an OSC 7 `file:///C:/...` cwd
/// report the desktop's Windows tab-cwd/workspace re-rooting reads.
/// cmd (and anything else that isn't PowerShell) has no prompt hooks
/// worth shimming and spawns as configured (`None`).
#[cfg(target_os = "windows")]
pub(super) fn neoism_block_shell_for_spawn(
    shell: &Shell,
    route_id: usize,
) -> Option<Shell> {
    let name = std::path::Path::new(&shell.program)
        .file_name()
        .and_then(|name| name.to_str())?
        .to_ascii_lowercase();
    if !matches!(
        name.as_str(),
        "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe"
    ) {
        return None;
    }
    let dir = std::env::temp_dir()
        .join(format!("neoism-shell-{}-{route_id}", std::process::id()));
    fs::create_dir_all(&dir).ok()?;
    let profile = dir.join("neoism_profile.ps1");
    let profile_script = r##"# Neoism block-shell integration for PowerShell. Dot-sourced via
# `-NoExit -Command` AFTER the user's own $PROFILE ran, mirroring the
# unix zsh/bash/fish rc wrappers: every prompt is framed with OSC 133
# marks so the block UI can segment command output, and OSC 7 reports
# the cwd for tab-cwd / workspace re-rooting.
$Global:__NeoismOriginalPrompt = $function:prompt

function Global:prompt {
    # $? / $LASTEXITCODE describe the command that just finished; read
    # them before anything in this function can clobber them.
    $__neoism_exit = if ($Global:?) {
        0
    } elseif (($null -ne $Global:LASTEXITCODE) -and ($Global:LASTEXITCODE -ne 0)) {
        $Global:LASTEXITCODE
    } else {
        1
    }
    # OSC 7 cwd: file:///C:/... with forward slashes and
    # percent-encoded spaces. Skipped on non-filesystem providers
    # (a registry drive has no cwd a terminal could re-root to).
    $__neoism_out = ''
    $__neoism_loc = $ExecutionContext.SessionState.Path.CurrentLocation
    if ($__neoism_loc.Provider.Name -eq 'FileSystem') {
        $__neoism_cwd = $__neoism_loc.ProviderPath.Replace('\', '/').Replace(' ', '%20')
        $__neoism_out += "$([char]27)]7;file:///$__neoism_cwd$([char]7)"
    }
    # D;<exit> closes the PREVIOUS command's block, then A <prompt
    # text> B frame this prompt (B doubles as command start).
    $__neoism_out += "$([char]27)]133;D;$__neoism_exit$([char]7)"
    $__neoism_out += "$([char]27)]133;A$([char]7)"
    $__neoism_out += if ($null -ne $Global:__NeoismOriginalPrompt) {
        $Global:__NeoismOriginalPrompt.Invoke()
    } else {
        "PS $($ExecutionContext.SessionState.Path.CurrentLocation)$('>' * ($NestedPromptLevel + 1)) "
    }
    $__neoism_out += "$([char]27)]133;B$([char]7)"
    $__neoism_out
}

# C (command pre-exec) needs a command-accepted hook, which only
# PSReadLine's PSConsoleHostReadLine provides. Wrap it when present;
# without PSReadLine the C mark is skipped and D/A/B still frame the
# blocks.
if ($null -ne (Get-Command PSConsoleHostReadLine -ErrorAction Ignore)) {
    $Global:__NeoismOriginalReadLine = $function:PSConsoleHostReadLine
    function Global:PSConsoleHostReadLine {
        $__neoism_line = & $Global:__NeoismOriginalReadLine
        [Console]::Write("$([char]27)]133;C$([char]7)")
        $__neoism_line
    }
}
"##;
    fs::write(&profile, profile_script).ok()?;

    // Dot-source through `-Command` (no -NoProfile) so the user's own
    // $PROFILE still runs first; appended AFTER the configured args so
    // `-NoLogo` and friends survive. Doubling embedded quotes is the
    // escape for a single-quoted PowerShell string literal.
    let quoted_profile = profile.display().to_string().replace('\'', "''");
    let mut args = shell.args.clone();
    args.push("-NoExit".to_string());
    args.push("-Command".to_string());
    args.push(format!(". '{quoted_profile}'"));
    Some(Shell {
        program: shell.program.clone(),
        args,
    })
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
