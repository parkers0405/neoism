//! Native Fast invite envelope parsing. Pure: no network, HTML, or persistence.
const MAX_INVITE_BYTES: usize = 8 * 1024;
const INVALID: &str =
    "Invalid Fast invite. Expected neoism://join?invite=<encoded HTTPS invite>";

/// Recognize the scheme even for malformed routes, so invalid invitations do
/// not fall through to file opening or command execution.
pub fn is_invite_uri(uri: &str) -> bool {
    uri.get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("neoism:"))
}

pub fn parse_invite_uri(uri: &str) -> Result<crate::fast_servers::FastJoin, String> {
    crate::fast_servers::normalize_fast_join(parse_invite_link(uri)?, None)
}

/// Keep the original decoded invite, including its capability fragment, for
/// form prefill and explicit retries. The normalized dial URL strips secrets.
pub(crate) fn parse_invite_link(uri: &str) -> Result<String, String> {
    if uri.len() > MAX_INVITE_BYTES || uri.bytes().any(|b| b.is_ascii_control()) {
        return Err(INVALID.into());
    }
    let encoded = uri.strip_prefix("neoism://join?invite=").ok_or(INVALID)?;
    if encoded.is_empty()
        || !encoded.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'-' | b'_' | b'.' | b'~' | b'*' | b'%')
        })
    {
        return Err(INVALID.into());
    }
    let link = decode(encoded)?;
    validate_invite_link(&link)?;
    Ok(link)
}

pub(crate) fn validate_invite_link(
    link: &str,
) -> Result<crate::fast_servers::FastJoin, String> {
    if link.len() > MAX_INVITE_BYTES || link.bytes().any(|b| b.is_ascii_control()) {
        return Err(INVALID.into());
    }
    // Inspect the literal path before URL parsing (which resolves dot segments
    // and backslashes). Only the documented routes are accepted.
    let rest = link.strip_prefix("https://").ok_or(INVALID)?;
    if rest.contains('\\') || rest.contains(char::is_whitespace) {
        return Err(INVALID.into());
    }
    let tail = rest.find(['/', '?', '#']).map(|i| &rest[i..]).unwrap_or("");
    let path = tail.split(['?', '#']).next().unwrap_or("");
    if !matches!(path, "" | "/" | "/session") {
        return Err(INVALID.into());
    }
    // Validate percent escapes and reject encoded controls, not only literal
    // controls. Never accept master tokens in query parameters or userinfo.
    let decoded = decode(link)?;
    if decoded.chars().any(char::is_control) {
        return Err(INVALID.into());
    }
    let url = url::Url::parse(link).map_err(|_| INVALID)?;
    if url.query_pairs().count() > 1 {
        return Err(INVALID.into());
    }
    crate::fast_servers::normalize_fast_join(link.to_owned(), None)
}

fn decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = bytes.get(i + 1).and_then(|b| hex(*b)).ok_or(INVALID)?;
            let lo = bytes.get(i + 2).and_then(|b| hex(*b)).ok_or(INVALID)?;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            // URI query decoding is not HTML form decoding: '+' stays '+'.
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| INVALID.into())
}
fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn envelope(link: &str) -> String {
        format!(
            "neoism://join?invite={}",
            url::form_urlencoded::byte_serialize(link.as_bytes()).collect::<String>()
        )
    }
    #[test]
    fn secretless_and_capability_preserved() {
        let link = "https://team.example/?name=Team%20One";
        let join = parse_invite_uri(&envelope(link)).unwrap();
        assert_eq!(join.endpoint, "wss://team.example/session");
        assert_eq!(join.name, "Team One");
        assert!(join.token.is_none());
        let link = format!("{link}#neoism-fast={}", "ab".repeat(32));
        assert_eq!(parse_invite_link(&envelope(&link)).unwrap(), link);
        assert_eq!(
            parse_invite_uri(&envelope(&link)).unwrap().token.unwrap(),
            "ab".repeat(32)
        );
    }
    #[test]
    fn rejects_wrong_routes_secrets_and_normalization_tricks() {
        for link in [
            "http://team.example/",
            "wss://team.example/session",
            "https://u:p@team.example/",
            "https://team.example/admin",
            "https://team.example/a/../session",
            "https://team.example/%2e/session",
            "https://team.example/?token=master",
            "https://team.example/?name=a&name=b",
            "https://team.example/#token=master",
            "https://team.example/?name=%00",
            "https://team.example/?name=%GG",
            "https://team.example:444/session",
        ] {
            assert!(parse_invite_uri(&envelope(link)).is_err(), "{link}");
        }
        for uri in [
            "neoism://join/other?invite=x",
            "neoism://join/?invite=x",
            "neoism://user@join?invite=x",
            "neoism://join?invite=x&invite=y",
            "neoism://join?invite=%FF",
            "neoism://join?invite=%",
            "neoism://join?invite=x#secret",
        ] {
            assert!(parse_invite_uri(uri).is_err(), "{uri}");
        }
    }
    #[test]
    fn bounded_and_plain_text_names() {
        assert!(parse_invite_uri(&format!(
            "neoism://join?invite={}",
            "a".repeat(MAX_INVITE_BYTES)
        ))
        .is_err());
        let join = parse_invite_uri(&envelope(
            "https://team.example/?name=%3Cb%3ETeam%3C%2Fb%3E",
        ))
        .unwrap();
        assert_eq!(join.name, "<b>Team</b>"); // Plain text; never rendered as HTML.
        assert!(is_invite_uri("NEOISM://bad"));
        assert!(!is_invite_uri("https://team.example/"));
    }
}
