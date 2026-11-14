//! Open a link in the system's default handler: [`Tx::open_url`](crate::Tx::open_url).
//!
//! Only `http:`, `https:` and `mailto:` links open, the same set the web build allows. Anything
//! else (`file:`, custom app schemes, `javascript:`) is refused with
//! [`OpenUrlError::UnsupportedScheme`] before anything runs, as are empty links and links with
//! spaces or control characters. The check runs when the handler calls `open_url`; the open
//! itself happens once the handler's frame is done.
//!
//! macOS asks `NSWorkspace`, Windows `ShellExecuteW`, Linux runs `xdg-open`. Headless runs and
//! tests never open anything: they keep the requests for [`Runtime::take_url_requests`].
use crate::Runtime;

pub(crate) mod incoming;
pub(crate) mod instance;

const MAX_LEN: usize = 8192;

/// Why a link was refused or didn't open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenUrlError {
    /// Nothing to open.
    Empty,
    /// Not a link: too long, has whitespace or control characters, or lacks a scheme or host.
    Invalid,
    /// A scheme other than `http`, `https` or `mailto` (lowercased).
    UnsupportedScheme(String),
    /// The system couldn't open it, or this platform has no way to.
    Failed(String),
}

impl std::fmt::Display for OpenUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("empty link"),
            Self::Invalid => f.write_str("not a valid link"),
            Self::UnsupportedScheme(s) => write!(f, "links with the {s}: scheme can't be opened"),
            Self::Failed(why) => write!(f, "couldn't open the link: {why}"),
        }
    }
}
impl std::error::Error for OpenUrlError {}

/// A link a handler asked to open, already checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Link {
    pub url: String,
    /// `Tx::open_url_same_tab`: replace the page on web rather than open a tab.
    pub same_tab: bool,
}

/// The link to open, trimmed and with a lowercase scheme, or why it can't be.
pub(crate) fn check(url: &str) -> Result<String, OpenUrlError> { check_link(url, false) }

/// `check` for a same-tab link: on web a link without a scheme (`/notes/42`, `?q=a`, `#top`)
/// is relative to the page, and the browser resolves it.
pub(crate) fn check_same_tab(url: &str) -> Result<String, OpenUrlError> { check_link(url, cfg!(target_arch = "wasm32")) }

fn check_link(url: &str, relative: bool) -> Result<String, OpenUrlError> {
    let url = url.trim();
    if url.is_empty() { return Err(OpenUrlError::Empty); }
    if url.len() > MAX_LEN
        || url.chars().any(|c| c.is_whitespace() || c.is_control()
            || matches!(c, '\\' | '"' | '<' | '>' | '`' | '^' | '{' | '|' | '}'))
        || bad_percent(url.as_bytes())
    {
        return Err(OpenUrlError::Invalid);
    }
    if relative && url.find([':', '/', '?', '#']).is_none_or(|i| url.as_bytes()[i] != b':') {
        return Ok(url.to_owned());
    }
    let (scheme, rest) = url.split_once(':').ok_or(OpenUrlError::Invalid)?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme.is_empty() || !scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return Err(OpenUrlError::Invalid);
    }
    match scheme.as_str() {
        "http" | "https" => {
            let after = rest.strip_prefix("//").ok_or(OpenUrlError::Invalid)?;
            let tail_start = after.find(['/', '?', '#']).unwrap_or(after.len());
            let (authority, tail) = after.split_at(tail_start);
            if tail.contains(['[', ']']) || !valid_authority(authority) {
                return Err(OpenUrlError::Invalid);
            }
        }
        "mailto" => {
            if rest.is_empty() || rest.starts_with("//") || rest.contains(['[', ']']) {
                return Err(OpenUrlError::Invalid);
            }
        }
        _ => return Err(OpenUrlError::UnsupportedScheme(scheme)),
    }
    Ok(format!("{scheme}:{rest}"))
}

fn bad_percent(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let (Some(&a), Some(&b)) = (bytes.get(i + 1), bytes.get(i + 2)) else { return true };
            if !a.is_ascii_hexdigit() || !b.is_ascii_hexdigit() || (a == b'0' && b == b'0') {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

fn valid_authority(authority: &str) -> bool {
    let (userinfo, host_port) = match authority.rsplit_once('@') {
        Some((u, hp)) => (Some(u), hp),
        None => (None, authority),
    };
    if userinfo.is_some_and(|u| u.is_empty() || u.contains(['[', ']', '@'])) { return false; }
    let port = if let Some(rest) = host_port.strip_prefix('[') {
        let Some((ipv6, port)) = rest.split_once(']') else { return false };
        if ipv6.len() < 2 || !ipv6.contains(':') || ipv6.starts_with('.') || ipv6.contains("..")
            || !ipv6.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.'))
        {
            return false;
        }
        if !port.is_empty() && !port.starts_with(':') { return false; }
        port.strip_prefix(':')
    } else if host_port.contains(['[', ']']) {
        return false;
    } else {
        let (host, port) = match host_port.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (host_port, None),
        };
        if host.is_empty() || host.starts_with('.') || host.contains("..") || host.contains(['%', ':']) {
            return false;
        }
        port
    };
    port.is_none_or(|p| !p.is_empty() && p.parse::<u16>().is_ok())
}

impl<S: 'static> Runtime<S> {
    /// Takes the links handlers asked to open that this runtime didn't open: all of them in a
    /// headless run or test, none in a window. Each was already checked.
    pub fn take_url_requests(&mut self) -> Vec<String> { std::mem::take(&mut self.url_requests) }

    pub(crate) fn open_urls(&mut self, links: Vec<Link>) {
        for link in links {
            if self.open_urls_natively {
                #[cfg(target_arch = "wasm32")]
                let opened = if link.same_tab { crate::platform::web::open_url_same_tab(&link.url).map_err(OpenUrlError::Failed) } else { open(&link.url) };
                #[cfg(not(target_arch = "wasm32"))]
                let opened = open(&link.url);
                if let Err(e) = opened { eprintln!("rune: open_url({}): {e}", link.url); }
            } else {
                self.url_requests.push(link.url);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn open(url: &str) -> Result<(), OpenUrlError> { crate::platform::web::open_url(url).map_err(OpenUrlError::Failed) }

#[cfg(target_os = "macos")]
fn open(url: &str) -> Result<(), OpenUrlError> {
    use objc2::{class, msg_send, rc::{Retained, autoreleasepool}};
    use objc2_foundation::{NSString, NSURL};
    let ascii = percent_encode_non_ascii(url);
    // SAFETY: NSWorkspace's shared instance lives for the process; `openURL:` takes an NSURL
    // and returns BOOL. Handlers run on the UI thread.
    autoreleasepool(|_| unsafe {
        let ns = NSString::from_str(&ascii);
        let url: Option<Retained<NSURL>> = NSURL::URLWithString(&ns);
        let url = url.ok_or(OpenUrlError::Invalid)?;
        let workspace: *mut objc2::runtime::AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace.is_null() { return Err(OpenUrlError::Failed("NSWorkspace unavailable".into())); }
        let opened: objc2::runtime::Bool = msg_send![workspace, openURL: &*url];
        if opened.as_bool() { Ok(()) } else { Err(OpenUrlError::Failed("no app opened it".into())) }
    })
}

#[cfg(target_os = "macos")]
fn percent_encode_non_ascii(url: &str) -> std::borrow::Cow<'_, str> {
    if url.is_ascii() { return std::borrow::Cow::Borrowed(url); }
    let mut out = String::with_capacity(url.len() + 16);
    for &b in url.as_bytes() {
        if b < 0x80 {
            out.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{b:02X}");
        }
    }
    std::borrow::Cow::Owned(out)
}

#[cfg(target_os = "windows")]
fn open(url: &str) -> Result<(), OpenUrlError> {
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, target) = (wide("open"), wide(url));
    // SAFETY: both strings are NUL-terminated and outlive the call; the other pointers are null.
    let result = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), target.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    // Values above 32 are success.
    if result as usize > 32 { Ok(()) } else { Err(OpenUrlError::Failed(format!("ShellExecute returned {}", result as usize))) }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
fn open(url: &str) -> Result<(), OpenUrlError> {
    debug_assert!(!url.starts_with('-'));
    // xdg-open's argument loop rejects `--` as an unknown option; `check` guarantees a scheme prefix.
    let mut child = std::process::Command::new("xdg-open").arg(url)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().map_err(|e| OpenUrlError::Failed(e.to_string()))?;
    // Reap it so it doesn't linger as a zombie.
    std::thread::spawn(move || { let _ = child.wait(); });
    Ok(())
}

#[cfg(not(any(target_arch = "wasm32", target_os = "macos", target_os = "windows", all(unix, not(any(target_os = "ios", target_os = "android"))))))]
fn open(_: &str) -> Result<(), OpenUrlError> { Err(OpenUrlError::Failed("this platform can't open links".into())) }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cx, Env, Runtime, el::{El, el}};

    #[test]
    fn allows_web_and_mail_links_only() {
        assert_eq!(check("  https://example.com/a?b=1#c ").as_deref(), Ok("https://example.com/a?b=1#c"));
        assert_eq!(check("HTTP://example.com").as_deref(), Ok("http://example.com"));
        assert_eq!(check("https://[::1]:8080/path").as_deref(), Ok("https://[::1]:8080/path"));
        assert_eq!(check("mailto:someone@example.com?subject=Hi").as_deref(), Ok("mailto:someone@example.com?subject=Hi"));
        assert_eq!(check("file:///etc/passwd"), Err(OpenUrlError::UnsupportedScheme("file".into())));
        assert_eq!(check("JavaScript:alert(1)"), Err(OpenUrlError::UnsupportedScheme("javascript".into())));
        assert_eq!(check("myapp://do/thing"), Err(OpenUrlError::UnsupportedScheme("myapp".into())));
        assert_eq!(check(""), Err(OpenUrlError::Empty));
        assert_eq!(check("  "), Err(OpenUrlError::Empty));
        for bad in [
            "example.com", "https:", "https://", "https:///x", "https://a b", "https://a\n.com",
            "mailto:", "mailto://example.com", "1http://x", "://x",
            "https://\\evil.com", "https://example.com/\"--flag", "https://example.com/a\\b",
            "https://example.com/%ZZ", "https://example.com/%00", "https://example.com/%2",
            "https://@/", "https://user@/", "https://:80/", "https://example.com:abc/",
            "https://evil%2f.com/", "https://.example.com", "https://a..b.com",
            "https://example.com/a[0]", "https://[not-ipv6]/", "https://[::1]abc/",
        ] {
            assert_eq!(check(bad), Err(OpenUrlError::Invalid), "{bad:?}");
        }
    }

    #[test]
    fn same_tab_links_may_be_relative_only_on_web() {
        for relative in ["/notes/42", "notes/42?x=1", "?q=a", "#top", "../c", "a/b:c"] {
            assert_eq!(check_link(relative, true).as_deref(), Ok(relative), "{relative:?}");
            assert_eq!(check_link(relative, false), Err(OpenUrlError::Invalid), "{relative:?}");
        }
        assert_eq!(check_link("javascript:alert(1)", true), Err(OpenUrlError::UnsupportedScheme("javascript".into())));
        assert_eq!(check_link("/a b", true), Err(OpenUrlError::Invalid));
        assert_eq!(check_link("HTTPS://example.com/x", true).as_deref(), Ok("https://example.com/x"));
        assert_eq!(check_same_tab("/notes/42"), Err(OpenUrlError::Invalid), "native has no page to resolve against");
    }

    #[test]
    fn headless_runs_record_the_request_and_open_nothing() {
        fn view(seen: &Vec<&'static str>, _: &mut Cx<Vec<&'static str>>) -> El {
            let _ = seen;
            el().key("btn").size(10.0, 10.0).on_press_tx(|seen: &mut Vec<&'static str>, tx| {
                seen.push("handler");
                tx.open_url("https://example.com/clicked").unwrap();
            })
        }
        let mut rt = Runtime::new(Vec::new(), view, (50.0, 50.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let mut results = Vec::new();
        rt.update(|_, tx| {
            results.push(tx.open_url("https://example.com/docs"));
            results.push(tx.open_url("file:///etc/hosts"));
            results.push(tx.open_url("mailto:a@example.com"));
            results.push(tx.open_url_same_tab("https://example.com/here"));
        });
        rt.pointer_down(5.0, 5.0);
        rt.pointer_up(5.0, 5.0);
        rt.frame(0.0);
        assert_eq!(results, [Ok(()), Err(OpenUrlError::UnsupportedScheme("file".into())), Ok(()), Ok(())]);
        assert_eq!(rt.take_url_requests(), [
            "https://example.com/docs",
            "mailto:a@example.com",
            "https://example.com/here",
            "https://example.com/clicked",
        ]);
        assert!(rt.take_url_requests().is_empty());
    }
}
