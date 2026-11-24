//! Browser handoff for the provider's own login process. Rune never reads its credentials.
use url::Url;

const MAX_LINE: usize = 16_512;
const GOOGLE_PROMPT: &str = "Open the following link to authenticate the ACP server: ";
const CODEX_DEVICE_URL: &str = "https://auth.openai.com/codex/device";

#[derive(Default)]
pub(super) struct BrowserHandoff {
    line: Vec<u8>,
    overflow: bool,
    pub(super) url: Option<Url>,
}

impl BrowserHandoff {
    /// Only called for output from an explicit sign-in tab, never from ordinary shell output.
    pub(super) fn receive(&mut self, command: (&str, &[&str]), data: &[u8]) -> Option<&Url> {
        let (program, args) = command;
        let google = program == "agy" && args == ["acp", "--login"];
        let device = program == "codex" && args == ["login", "--device-auth"];
        if self.url.is_some() || !(google || device) { return None; }
        for &byte in data {
            if byte == b'\n' {
                let url = (!self.overflow).then(|| std::str::from_utf8(&self.line).ok()).flatten().and_then(|line| {
                    let line = line.trim_end_matches('\r');
                    if google { google_url(line.strip_prefix(GOOGLE_PROMPT)?) }
                    // The device URL is fixed, including when the CLI wraps it in ANSI styling.
                    else if line.contains(CODEX_DEVICE_URL) { Url::parse(CODEX_DEVICE_URL).ok() }
                    else { None }
                });
                self.line.clear();
                self.overflow = false;
                if let Some(url) = url {
                    self.url = Some(url);
                    return self.url.as_ref();
                }
            } else if self.line.len() < MAX_LINE && !self.overflow {
                self.line.push(byte);
            } else {
                self.line.clear();
                self.overflow = true;
            }
        }
        None
    }
}

fn google_url(raw: &str) -> Option<Url> {
    if raw.len() > 16_384 || raw.chars().any(|c| c.is_whitespace() || c.is_control()) { return None; }
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("accounts.google.com") || url.port().is_some()
        || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some()
        || url.path() != "/o/oauth2/auth" { return None; }
    let params: Vec<_> = url.query_pairs().collect();
    let one = |key: &str| {
        let mut values = params.iter().filter(|(name, _)| name == key).map(|(_, value)| value.as_ref());
        values.next().filter(|_| values.next().is_none())
    };
    let state = one("state")?;
    if state.is_empty() || state.len() > 512 || state.chars().any(|c| c.is_whitespace() || c.is_control())
        || one("response_type")? != "code" || one("redirect_uri")? != "https://antigravity.google/oauth-callback" {
        return None;
    }
    Some(url)
}

pub(super) fn open(url: &Url) -> std::io::Result<()> {
    #[cfg(not(windows))]
    { crate::cli::open_external(url.as_str()) }
    #[cfg(windows)]
    {
        // Do not send OAuth query strings through cmd.exe.
        std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", url.as_str()]).spawn().map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUERY: &str = "?state=opaque&response_type=code&redirect_uri=https%3A%2F%2Fantigravity.google%2Foauth-callback";

    #[test]
    fn harness_login_only_opens_a_valid_google_code_url_once() {
        let mut browser = BrowserHandoff::default();
        let cmd = ("agy", &["acp", "--login"][..]);
        for url in [
            format!("https://accounts.google.com.evil.invalid/o/oauth2/auth{QUERY}"),
            format!("https://accounts.google.com/o/oauth2/auth{QUERY}&state=duplicate"),
            format!("https://accounts.google.com/o/oauth2/auth{QUERY}&redirect_uri=https://evil.invalid"),
            format!("https://accounts.google.com/o/oauth2/auth{}", QUERY.replace("antigravity.google", "evil.invalid")),
        ] {
            assert!(browser.receive(cmd, format!("{GOOGLE_PROMPT}{url}\r\n").as_bytes()).is_none());
        }
        let valid = format!("{GOOGLE_PROMPT}https://accounts.google.com/o/oauth2/auth{QUERY}\r\n");
        assert!(browser.receive(("sh", &[]), valid.as_bytes()).is_none());
        assert!(browser.receive(cmd, &valid.as_bytes()[..90]).is_none());
        assert_eq!(browser.receive(cmd, &valid.as_bytes()[90..]).unwrap().host_str(), Some("accounts.google.com"));
        assert!(browser.receive(cmd, valid.as_bytes()).is_none());
    }

    #[test]
    fn harness_login_codex_device_handoff_never_uses_the_emitted_query() {
        let mut browser = BrowserHandoff::default();
        let output = format!("  \x1b[4m{CODEX_DEVICE_URL}?ignored=true\x1b[0m\r\n");
        assert!(browser.receive(("codex", &["login"]), output.as_bytes()).is_none());
        assert_eq!(browser.receive(("codex", &["login", "--device-auth"]), output.as_bytes()).unwrap().as_str(), CODEX_DEVICE_URL);
    }
}