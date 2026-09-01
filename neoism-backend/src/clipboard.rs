// clipboard.rs was retired originally from https://github.com/alacritty/alacritty/blob/e35e5ad14fce8456afdd89f2b392b9924bb27471/alacritty/src/clipboard.rs
// which is licensed under Apache 2.0 license.

use raw_window_handle::RawDisplayHandle;
#[cfg(not(any(target_os = "macos", windows)))]
use std::io::Write;
#[cfg(not(any(target_os = "macos", windows)))]
use std::process::{Command, Stdio};
use tracing::warn;

/// Phase 3b consolidated `ClipboardType` into `neoism-terminal-core`.
/// Backend re-exports it from its `lib.rs` so call sites that say
/// `neoism_backend::ClipboardType` continue to compile.
pub use neoism_terminal_core::ClipboardType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub bytes: Vec<u8>,
    pub mime: String,
    pub filename: String,
}

use copypasta::nop_clipboard::NopClipboardContext;
#[cfg(all(feature = "wayland", not(any(target_os = "macos", windows))))]
use copypasta::wayland_clipboard;
#[cfg(all(feature = "x11", not(any(target_os = "macos", windows))))]
use copypasta::x11_clipboard::{Primary as X11SelectionClipboard, X11ClipboardContext};
#[cfg(any(feature = "x11", target_os = "macos", windows))]
use copypasta::ClipboardContext;
use copypasta::ClipboardProvider;

pub struct Clipboard {
    clipboard: Box<dyn ClipboardProvider>,
    selection: Option<Box<dyn ClipboardProvider>>,
}

impl Clipboard {
    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn new(display: RawDisplayHandle) -> Self {
        match display {
            #[cfg(all(feature = "wayland", not(any(target_os = "macos", windows))))]
            RawDisplayHandle::Wayland(display) => {
                let (selection, clipboard) =
                    wayland_clipboard::create_clipboards_from_external(
                        display.display.as_ptr(),
                    );
                Self {
                    clipboard: Box::new(clipboard),
                    selection: Some(Box::new(selection)),
                }
            }
            _ => Self::default(),
        }
    }

    /// Used for tests and to handle missing clipboard provider when built without the `x11`
    /// feature.
    pub fn new_nop() -> Self {
        Self {
            clipboard: Box::new(NopClipboardContext::new().unwrap()),
            selection: None,
        }
    }
}

impl Default for Clipboard {
    fn default() -> Self {
        #[cfg(any(target_os = "macos", windows))]
        return Self {
            clipboard: Box::new(ClipboardContext::new().unwrap()),
            selection: None,
        };

        #[cfg(all(feature = "x11", not(any(target_os = "macos", windows))))]
        return Self {
            clipboard: Box::new(ClipboardContext::new().unwrap()),
            selection: Some(Box::new(
                X11ClipboardContext::<X11SelectionClipboard>::new().unwrap(),
            )),
        };

        #[cfg(not(any(feature = "x11", target_os = "macos", windows)))]
        return Self::new_nop();
    }
}

impl Clipboard {
    pub fn set(&mut self, ty: ClipboardType, text: impl Into<String>) {
        let text = text.into();
        let clipboard = match (ty, &mut self.selection) {
            (ClipboardType::Selection, Some(provider)) => provider,
            (ClipboardType::Selection, None) => return,
            _ => &mut self.clipboard,
        };

        clipboard.set_contents(text.clone()).unwrap_or_else(|err| {
            warn!("Unable to store text in clipboard: {}", err);
        });

        set_external_clipboard(ty, &text);
    }

    pub fn get(&mut self, ty: ClipboardType) -> String {
        if let Some(text) = get_external_clipboard(ty) {
            return text;
        }

        let clipboard = match (ty, &mut self.selection) {
            (ClipboardType::Selection, Some(provider)) => provider,
            _ => &mut self.clipboard,
        };

        match clipboard.get_contents() {
            Err(err) => {
                warn!("Unable to load text from clipboard: {}", err);
                String::new()
            }
            Ok(text) => text,
        }
    }

    pub fn get_image(&mut self) -> Option<ClipboardImage> {
        get_external_clipboard_image()
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn set_external_clipboard(ty: ClipboardType, text: &str) {
    let commands: &[(&str, &[&str])] = match ty {
        ClipboardType::Clipboard => &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ],
        ClipboardType::Selection => &[
            ("wl-copy", &["--primary"]),
            ("xclip", &["-selection", "primary"]),
            ("xsel", &["--primary", "--input"]),
        ],
    };

    for (program, args) in commands {
        let Ok(mut child) = Command::new(program)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };

        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(text.as_bytes()).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                continue;
            }
        }

        if child.wait().is_ok_and(|status| status.success()) {
            return;
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
fn set_external_clipboard(_ty: ClipboardType, _text: &str) {}

#[cfg(not(any(target_os = "macos", windows)))]
fn get_external_clipboard(ty: ClipboardType) -> Option<String> {
    let commands: &[(&str, &[&str])] = match ty {
        ClipboardType::Clipboard => &[
            ("wl-paste", &["--no-newline", "--type", "text/plain"]),
            (
                "wl-paste",
                &[
                    "--no-newline",
                    "--type",
                    "text/plain;charset=utf-8",
                ],
            ),
            ("wl-paste", &["--no-newline"]),
            ("xclip", &["-selection", "clipboard", "-out"]),
            ("xsel", &["--clipboard", "--output"]),
        ],
        ClipboardType::Selection => &[
            (
                "wl-paste",
                &["--primary", "--no-newline", "--type", "text/plain"],
            ),
            (
                "wl-paste",
                &[
                    "--primary",
                    "--no-newline",
                    "--type",
                    "text/plain;charset=utf-8",
                ],
            ),
            ("wl-paste", &["--primary", "--no-newline"]),
            ("xclip", &["-selection", "primary", "-out"]),
            ("xsel", &["--primary", "--output"]),
        ],
    };

    for (program, args) in commands {
        let Ok(output) = Command::new(program)
            .args(*args)
            .stderr(Stdio::null())
            .output()
        else {
            continue;
        };

        if output.status.success() {
            return Some(String::from_utf8_lossy(&output.stdout).into_owned());
        }
    }

    None
}

#[cfg(any(target_os = "macos", windows))]
fn get_external_clipboard(_ty: ClipboardType) -> Option<String> {
    None
}

#[cfg(not(any(target_os = "macos", windows)))]
fn get_external_clipboard_image() -> Option<ClipboardImage> {
    const IMAGE_TYPES: &[(&str, &str)] = &[
        ("image/png", "png"),
        ("image/jpeg", "jpg"),
        ("image/webp", "webp"),
        ("image/gif", "gif"),
    ];

    if let Some(types) = command_stdout("wl-paste", &["--list-types"]) {
        let types = String::from_utf8_lossy(&types);
        for (mime, extension) in IMAGE_TYPES {
            if !types.lines().any(|line| line.trim() == *mime) {
                continue;
            }
            if let Some(bytes) = command_stdout("wl-paste", &["--type", mime]) {
                return Some(ClipboardImage {
                    bytes,
                    mime: (*mime).to_string(),
                    filename: format!("clipboard.{extension}"),
                });
            }
        }
    }

    for (mime, extension) in IMAGE_TYPES {
        if let Some(bytes) =
            command_stdout("xclip", &["-selection", "clipboard", "-t", mime, "-out"])
        {
            return Some(ClipboardImage {
                bytes,
                mime: (*mime).to_string(),
                filename: format!("clipboard.{extension}"),
            });
        }
    }

    None
}

#[cfg(not(any(target_os = "macos", windows)))]
fn command_stdout(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(program)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    Some(output.stdout)
}

#[cfg(any(target_os = "macos", windows))]
fn get_external_clipboard_image() -> Option<ClipboardImage> {
    None
}
