use crate::context::Context;
use rustc_hash::FxHashMap;
use std::path::{Path, MAIN_SEPARATOR, MAIN_SEPARATOR_STR};
use std::time::Instant;

pub struct ContextTitleExtra {
    pub program: String,
}

pub struct ContextTitle {
    pub content: String,
    pub extra: Option<ContextTitleExtra>,
}

pub struct ContextManagerTitles {
    pub last_title_update: Option<Instant>,
    pub titles: FxHashMap<usize, ContextTitle>,
    pub key: String,
}

impl ContextManagerTitles {
    pub fn new(
        idx: usize,
        content: String,
        extra: Option<ContextTitleExtra>,
    ) -> ContextManagerTitles {
        let key = format!("{idx}{content};");
        let mut map = FxHashMap::default();
        map.insert(idx, ContextTitle { content, extra });
        ContextManagerTitles {
            key,
            titles: map,
            last_title_update: None,
        }
    }

    #[inline]
    pub fn set_key_val(
        &mut self,
        idx: usize,
        content: String,
        extra: Option<ContextTitleExtra>,
    ) {
        self.titles.insert(idx, ContextTitle { content, extra });
    }

    #[inline]
    pub fn set_key(&mut self, key: String) {
        self.key = key;
    }
}

pub fn create_title_extra_from_context<T: neoism_backend::event::EventListener>(
    context: &Context<T>,
) -> Option<ContextTitleExtra> {
    #[cfg(unix)]
    let program =
        teletypewriter::foreground_process_name(*context.main_fd, context.shell_pid);

    // Windows has no controlling-tty fd; the ConPTY backend resolves the
    // foreground process by walking the shell pid's descendants.
    #[cfg(windows)]
    let program = teletypewriter::foreground_process_name(context.shell_pid);

    #[cfg(all(not(unix), not(windows)))]
    let program = String::default();

    Some(ContextTitleExtra { program })
}

// Possible options:

// - `TITLE`: terminal title via OSC sequences for setting terminal title
// - `PROGRAM`: (e.g `fish`, `zsh`, `bash`, `vim`, etc...)
// - `ABSOLUTE_PATH`: (e.g `/Users/rapha/Documents/a/rio`)
// - `RELATIVE_PATH`: (e.g `~/Documents/a/rio` or `…/a/psone/starpsx`)
// - `COLUMNS`: current columns
// - `LINES`: current lines

/// Shorten an absolute path for display:
/// - Replace home directory prefix with `~`
/// - If 4+ components deep, show `…/last/three/components`
fn shorten_path(absolute: &str) -> String {
    let path = Path::new(absolute);

    // Replace home prefix with ~ (`/home/name` and `C:\Users\name` alike)
    let display_path = {
        if let Some(home) = dirs::home_dir() {
            if let Ok(stripped) = path.strip_prefix(&home) {
                let s = stripped.to_string_lossy();
                if s.is_empty() {
                    "~".to_string()
                } else {
                    format!("~{MAIN_SEPARATOR}{s}")
                }
            } else {
                absolute.to_string()
            }
        } else {
            absolute.to_string()
        }
    };

    // If 4+ components, show …/last3; split on both separators so
    // Windows paths shorten too
    let components: Vec<&str> = display_path
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .collect();
    if components.len() >= 4 {
        format!(
            "…{MAIN_SEPARATOR}{}",
            components[components.len() - 3..].join(MAIN_SEPARATOR_STR)
        )
    } else {
        display_path
    }
}

#[inline]
fn try_terminal_title<T: neoism_backend::event::EventListener>(
    context: &Context<T>,
) -> Option<String> {
    // Title refresh runs on the UI thread; never wait behind a PTY fair-lock lease.
    context
        .terminal
        .try_lock_unfair()
        .map(|terminal| terminal.title.to_string())
}

#[inline]
fn try_terminal_current_directory<T: neoism_backend::event::EventListener>(
    context: &Context<T>,
) -> Option<String> {
    // Title refresh runs on the UI thread; never wait behind a PTY fair-lock lease.
    context.terminal.try_lock_unfair().and_then(|terminal| {
        terminal
            .current_directory
            .as_ref()
            .and_then(|path| path.clone().into_os_string().into_string().ok())
    })
}

#[inline]
pub fn update_title<T: neoism_backend::event::EventListener>(
    template: &str,
    context: &Context<T>,
) -> String {
    if template.is_empty() {
        return template.to_string();
    }

    let mut new_template = template.to_owned();

    let re = regex::Regex::new(r"\{\{(.*?)\}\}").unwrap();
    for (to_replace_str, [variable]) in re.captures_iter(template).map(|c| c.extract()) {
        let variables = if to_replace_str.contains("||") {
            variable.split("||").collect()
        } else {
            vec![variable]
        };

        let mut matched = false;
        for (i, scoped_variable) in variables.iter().enumerate() {
            if matched {
                break;
            }

            let var = scoped_variable.to_owned().trim().to_lowercase();
            match var.as_str() {
                "columns" => {
                    new_template = new_template
                        .replace(to_replace_str, &context.dimension.columns.to_string());
                    matched = true;
                }
                "lines" => {
                    new_template = new_template
                        .replace(to_replace_str, &context.dimension.lines.to_string());
                    matched = true;
                }
                "title" => {
                    // In case it has a fallback and title is empty
                    // or
                    // In case is the last then we need to erase variables either way
                    let is_only_one = variables.len() == 1;
                    let is_last = i == variables.len() - 1;
                    let Some(terminal_title) = try_terminal_title(context) else {
                        if is_only_one || is_last {
                            new_template = new_template.replace(to_replace_str, "");
                        }
                        continue;
                    };

                    if is_only_one || is_last || !terminal_title.is_empty() {
                        new_template =
                            new_template.replace(to_replace_str, &terminal_title);
                        matched = !terminal_title.is_empty();
                    }
                }
                "program" => {
                    #[cfg(unix)]
                    let program = teletypewriter::foreground_process_name(
                        *context.main_fd,
                        context.shell_pid,
                    );
                    #[cfg(windows)]
                    let program =
                        teletypewriter::foreground_process_name(context.shell_pid);
                    #[cfg(all(not(unix), not(windows)))]
                    let program = String::new();

                    new_template = new_template.replace(to_replace_str, &program);
                    matched = true;
                }
                "absolute_path" => {
                    if let Some(dir_str) = try_terminal_current_directory(context) {
                        new_template = new_template.replace(to_replace_str, &dir_str);
                        matched = true;
                        continue;
                    }

                    #[cfg(unix)]
                    {
                        let path = teletypewriter::foreground_process_path(
                            *context.main_fd,
                            context.shell_pid,
                        )
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();

                        // In case it has a fallback and path is empty
                        // or
                        // In case is the last then we need to erase variables either way
                        let is_only_one = variables.len() == 1;
                        let is_last = i == variables.len() - 1;
                        if is_only_one || is_last {
                            new_template = new_template.replace(to_replace_str, &path);
                            continue;
                        }

                        if !path.is_empty() {
                            new_template = new_template.replace(to_replace_str, &path);
                            matched = true;
                        }
                    }

                    // No /proc-style cwd read off-unix: OSC 7 above is the
                    // only source. Erase the placeholder so the raw
                    // template never shows in the title.
                    #[cfg(not(unix))]
                    {
                        let is_only_one = variables.len() == 1;
                        let is_last = i == variables.len() - 1;
                        if is_only_one || is_last {
                            new_template = new_template.replace(to_replace_str, "");
                        }
                    }
                }
                "relative_path" => {
                    if let Some(dir_str) = try_terminal_current_directory(context) {
                        new_template =
                            new_template.replace(to_replace_str, &shorten_path(&dir_str));
                        matched = true;
                        continue;
                    }

                    #[cfg(unix)]
                    {
                        let path = teletypewriter::foreground_process_path(
                            *context.main_fd,
                            context.shell_pid,
                        )
                        .map(|p| shorten_path(&p.to_string_lossy()))
                        .unwrap_or_default();

                        let is_only_one = variables.len() == 1;
                        let is_last = i == variables.len() - 1;
                        if is_only_one || is_last {
                            new_template = new_template.replace(to_replace_str, &path);
                            continue;
                        }

                        if !path.is_empty() {
                            new_template = new_template.replace(to_replace_str, &path);
                            matched = true;
                        }
                    }

                    #[cfg(not(unix))]
                    {
                        let is_only_one = variables.len() == 1;
                        let is_last = i == variables.len() - 1;
                        if is_only_one || is_last {
                            new_template = new_template.replace(to_replace_str, "");
                        }
                    }
                }
                _ => {}
            }
        }
    }

    new_template
}

#[cfg(test)]
pub mod test {
    use super::*;
    use crate::context::create_mock_context;
    use crate::context::ContextDimension;
    use neoism_backend::config::layout::Margin;
    use neoism_backend::event::VoidListener;
    use neoism_backend::sugarloaf::layout::TextDimensions;
    use neoism_window::window::WindowId;

    #[test]
    fn test_update_title() {
        let context_dimension = ContextDimension::build(
            1200.0,
            800.0,
            TextDimensions {
                scale: 2.,
                width: 18.,
                height: 9.,
            },
            1.0,
            Margin::default(),
        );

        assert_eq!(context_dimension.columns, 64);
        assert_eq!(context_dimension.lines, 84);

        let rich_text_id = 0;
        let context = create_mock_context(
            VoidListener {},
            WindowId::from(0),
            rich_text_id,
            context_dimension,
        );
        assert_eq!(update_title("", &context), String::from(""));
        assert_eq!(update_title("{{columns}}", &context), String::from("64"));
        assert_eq!(update_title("{{COLUMNS}}", &context), String::from("64"));
        assert_eq!(update_title("{{ COLUMNS }}", &context), String::from("64"));
        assert_eq!(update_title("{{ columns }}", &context), String::from("64"));
        assert_eq!(
            update_title("hello {{ COLUMNS }} AbC", &context),
            String::from("hello 64 AbC")
        );
        assert_eq!(
            update_title("hello {{ Lines }} AbC", &context),
            String::from("hello 84 AbC")
        );
        assert_eq!(
            update_title("{{ columns }}x{{lines}}", &context),
            String::from("64x84")
        );

        assert_eq!(update_title("{{ title }}", &context), String::from(""));

        // #[cfg(unix)]
        // assert_eq!(
        //     update_title("{{path_absolute}}"), &context)
        //     String::from("")
        // );
    }

    #[test]
    fn test_update_title_does_not_block_on_busy_terminal() {
        let context_dimension = ContextDimension::build(
            1200.0,
            800.0,
            TextDimensions {
                scale: 2.,
                width: 18.,
                height: 9.,
            },
            1.0,
            Margin::default(),
        );

        let context =
            create_mock_context(VoidListener {}, WindowId::from(0), 0, context_dimension);
        let _terminal = context.terminal.lock_unfair();

        assert_eq!(
            update_title("{{ title || columns }}", &context),
            String::from("64")
        );
        assert_eq!(update_title("{{ title }}", &context), String::from(""));
    }

    #[test]
    fn test_update_title_with_logical_or() {
        let context_dimension = ContextDimension::build(
            1200.0,
            800.0,
            TextDimensions {
                scale: 2.,
                width: 18.,
                height: 9.,
            },
            1.0,
            Margin::default(),
        );

        assert_eq!(context_dimension.columns, 64);
        assert_eq!(context_dimension.lines, 84);

        let rich_text_id = 0;
        let context = create_mock_context(
            VoidListener {},
            WindowId::from(0),
            rich_text_id,
            context_dimension,
        );
        assert_eq!(update_title("", &context), String::from(""));
        // Title always starts empty
        assert_eq!(update_title("{{title}}", &context), String::from(""));

        assert_eq!(
            update_title("{{ title || columns }}", &context),
            String::from("64")
        );

        assert_eq!(
            update_title("{{ title || title }}", &context),
            String::from("")
        );

        // let's modify title to actually be something
        {
            let mut term = context.terminal.lock();
            term.title = "Something".to_string();
        };

        assert_eq!(
            update_title("{{ title || columns }}", &context),
            String::from("Something")
        );

        assert_eq!(
            update_title("{{ columns || title }}", &context),
            String::from("64")
        );

        // Use a path that can't plausibly be $HOME on any realistic system.
        // Sandboxed builds (e.g. Void's xbps-src) often set HOME=/tmp, so a
        // literal "/tmp" here would get collapsed to "~" and break the test.
        {
            let path = std::path::PathBuf::from("/rio-sandbox-test-dir");
            let mut term = context.terminal.lock();
            term.current_directory = Some(path);
        };

        assert_eq!(
            update_title("{{ absolute_path || title }}", &context),
            String::from("/rio-sandbox-test-dir"),
        );

        assert_eq!(
            update_title("{{ relative_path || title }}", &context),
            String::from("/rio-sandbox-test-dir"),
        );
    }

    #[test]
    fn test_shorten_path() {
        // Use a path prefix that can't plausibly be $HOME to keep the test
        // deterministic in build sandboxes that set HOME=/tmp or similar.
        assert_eq!(
            shorten_path("/rio-sandbox-test-dir"),
            "/rio-sandbox-test-dir",
        );
        assert_eq!(
            shorten_path("/rio-sandbox-test-dir/sub"),
            "/rio-sandbox-test-dir/sub",
        );

        // Deep paths get truncated to last 3 components
        assert_eq!(shorten_path("/a/b/c/d/e"), "…/c/d/e");
        assert_eq!(shorten_path("/a/b/c/d"), "…/b/c/d");

        // 3 components stays as-is
        assert_eq!(shorten_path("/a/b/c"), "/a/b/c");
    }
}
