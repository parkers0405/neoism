//! Native shell startup hooks shared by local desktop and daemon PTYs.
use base64::Engine;

/// Frame cmd's expanded prompt without changing its visible text. cmd has no
/// dependable pre-exec/status hook: each newly printed prompt supplies D/A/B,
/// and D intentionally omits an exit code. ST is expressible using PROMPT's
/// `$E` escape; BEL is not. Explicit /C, /K and batch invocations are untouched.
/// The lifecycle prefix lives on its own prompt row (`$_`): cmd's line editor
/// can replay the editable prompt row while echoing/wrapping a submission. An
/// inline D would falsely finish that command, as verified under Wine. The
/// user's original prompt text follows unchanged on the editable row. As in
/// normal cmd, /Q, echo-off or a later PROMPT replacement can suppress prompts.
pub fn cmd_prompt(
    program: &str,
    args: &[String],
    original: Option<&str>,
) -> Option<String> {
    let name = program.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    if !matches!(name.as_str(), "cmd" | "cmd.exe")
        || args.iter().any(|arg| {
            let arg = arg.to_ascii_lowercase();
            !arg.starts_with('/') || arg.starts_with("/c") || arg.starts_with("/k")
        })
    {
        return None;
    }
    const PREFIX: &str = r"$e]133;D$e\$e]133;A$e\$_";
    const SUFFIX: &str = r"$e]133;B$e\";
    let prompt = original.filter(|value| !value.is_empty()).unwrap_or("$P$G");
    if prompt.starts_with(PREFIX) && prompt.ends_with(SUFFIX) {
        return Some(prompt.to_owned());
    }
    Some(format!("{PREFIX}{prompt}{SUFFIX}"))
}

/// Apply only to the child environment, treating Windows variable names as
/// case-insensitive. Called at the native spawn boundary so neither daemon
/// prepared sessions nor desktop factories can accidentally bypass the hook.
pub fn apply_cmd_prompt_env(
    program: &str,
    args: &[String],
    env: &mut Vec<(String, String)>,
) {
    let original = env
        .iter()
        .rev()
        .find(|(key, _)| key.eq_ignore_ascii_case("PROMPT"))
        .map(|(_, value)| value.clone())
        .or_else(|| {
            std::env::vars()
                .find(|(key, _)| key.eq_ignore_ascii_case("PROMPT"))
                .map(|(_, value)| value)
        });
    if let Some(prompt) = cmd_prompt(program, args, original.as_deref()) {
        env.retain(|(key, _)| !key.eq_ignore_ascii_case("PROMPT"));
        env.push(("PROMPT".into(), prompt));
    }
}

/// Append an interactive PowerShell hook after normal profile loading. Other
/// shells retain their configured arguments unchanged (the caller gets None).
pub fn powershell_args(program: &str, args: &[String]) -> Option<Vec<String>> {
    let name = program.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    if !matches!(
        name.as_str(),
        "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe"
    ) {
        return None;
    }
    // A command/file invocation is not an interactive profile startup. Do not
    // append a second command or change the meaning of the user's argv.
    if args.iter().any(|arg| {
        let arg = arg.trim_start_matches('-').to_ascii_lowercase();
        matches!(
            arg.as_str(),
            "c" | "command"
                | "commandwithargs"
                | "f"
                | "file"
                | "e"
                | "ec"
                | "enc"
                | "encodedcommand"
                | "noninteractive"
                | "noni"
        )
    }) {
        return None;
    }
    let profile_script = r##"# Neoism block-shell integration for PowerShell. Run via
# `-NoExit -EncodedCommand` AFTER the user's own $PROFILE ran, mirroring the
# unix zsh/bash/fish rc wrappers: every prompt is framed with OSC 133
# marks so the block UI can segment command output, and OSC 7 reports
# the cwd for tab-cwd / workspace re-rooting.
$Global:__NeoismOriginalPrompt = $function:prompt
$Global:__NeoismExecutable = (Get-Command neoism -CommandType Application -ErrorAction Ignore).Source
function Global:neoism {
    if (($args.Count -gt 0) -and ($args[0] -eq 'cd')) {
        if ($args.Count -gt 2) { Write-Error 'usage: neoism cd [directory]'; return }
        try {
            if ($args.Count -eq 1) { Set-Location $HOME -ErrorAction Stop }
            elseif ($args[1] -eq '-') { Set-Location -Path '-' -ErrorAction Stop }
            else { Set-Location -LiteralPath $args[1] -ErrorAction Stop }
        } catch { Write-Error $_; return }
        $__neoism_cwd = $ExecutionContext.SessionState.Path.CurrentLocation.ProviderPath.Replace('\', '/').Replace(' ', '%20')
        [Console]::Write("$([char]27)]7;file:///$__neoism_cwd$([char]7)")
    } elseif ($Global:__NeoismExecutable) {
        & $Global:__NeoismExecutable @args
    }
}

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
    # text> B frame the new prompt; C marks command acceptance.
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

    // PowerShell's encoded commands are UTF-16LE. Keeping the integration
    // inline avoids execution-policy checks on a generated .ps1 file.
    let encoded_profile = base64::engine::general_purpose::STANDARD.encode(
        profile_script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let mut args = args.to_vec();
    if !args.iter().any(|arg| arg.eq_ignore_ascii_case("-NoExit")) {
        args.push("-NoExit".to_string());
    }
    args.push("-EncodedCommand".to_string());
    args.push(encoded_profile);
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_prompt_preserves_text_and_does_not_invent_exit_codes() {
        let original = "custom $P$G$S";
        let prompt = cmd_prompt(
            r"C:\Windows\System32\CMD.EXE",
            &["/D".into(), "/Q".into()],
            Some(original),
        )
        .unwrap();
        assert_eq!(prompt, r"$e]133;D$e\$e]133;A$e\$_custom $P$G$S$e]133;B$e\");
        assert!(!prompt.contains("133;D;"));
        assert_eq!(cmd_prompt("cmd", &[], Some(&prompt)), Some(prompt));
        assert!(cmd_prompt("cmd", &[], None).unwrap().contains("$P$G"));
        for args in [
            vec!["/C", "set /a 1+1"],
            vec!["/cdir"],
            vec!["/K", "startup.cmd"],
            vec!["/kfoo"],
            vec!["file.cmd"],
        ] {
            let args: Vec<_> = args.into_iter().map(str::to_owned).collect();
            assert!(cmd_prompt("cmd.exe", &args, None).is_none());
        }
        assert!(cmd_prompt("pwsh.exe", &[], None).is_none());
    }

    #[test]
    fn cmd_environment_is_child_scoped_and_case_insensitive() {
        let mut env = vec![
            ("Prompt".into(), "custom $G".into()),
            ("OTHER".into(), "keep".into()),
        ];
        apply_cmd_prompt_env("cmd.exe", &[], &mut env);
        assert_eq!(
            env.iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case("prompt"))
                .count(),
            1
        );
        assert_eq!(env[0], ("OTHER".into(), "keep".into()));
        assert!(env[1].1.contains("custom $G"));
        let original = env.clone();
        apply_cmd_prompt_env("cmd.exe", &[], &mut env);
        assert_eq!(env, original);
        apply_cmd_prompt_env("cmd.exe", &["/C".into(), "script.cmd".into()], &mut env);
        assert_eq!(env, original);
    }

    #[test]
    fn powershell_hook_is_utf16le_and_preserves_profile_arguments() {
        for program in [
            "pwsh",
            "PowerShell.EXE",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
        ] {
            let original = vec!["-NoLogo".to_string()];
            let args = powershell_args(program, &original).unwrap();
            assert_eq!(&args[..3], &["-NoLogo", "-NoExit", "-EncodedCommand"]);
            assert!(!args.iter().any(|arg| arg == "-NoProfile"));
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&args[3])
                .unwrap();
            let units: Vec<_> = bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            let script = String::from_utf16(&units).unwrap();
            for marker in ["]133;A", "]133;B", "]133;C", "]133;D;", "]7;file:///"] {
                assert!(script.contains(marker), "missing {marker}");
            }
            assert!(script.contains("$function:prompt"));
            assert!(script.contains("$function:PSConsoleHostReadLine"));
        }
    }

    #[test]
    fn leaves_other_shells_and_explicit_commands_alone() {
        for shell in ["cmd.exe", "/bin/bash", "fish", "not-pwsh.exe"] {
            assert!(powershell_args(shell, &[]).is_none());
        }
        for option in [
            "-Command",
            "-c",
            "-File",
            "-EncodedCommand",
            "-NonInteractive",
        ] {
            assert!(
                powershell_args("pwsh.exe", &[option.into(), "user payload".into()])
                    .is_none()
            );
        }
        let args = powershell_args("pwsh", &["-NoProfile".into()]).unwrap();
        assert_eq!(args[0], "-NoProfile");
        let args = powershell_args("pwsh", &["-NoExit".into()]).unwrap();
        assert_eq!(args.iter().filter(|arg| *arg == "-NoExit").count(), 1);
    }
}
