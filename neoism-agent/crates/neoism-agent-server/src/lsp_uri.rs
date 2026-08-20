use std::path::{Path, PathBuf};

pub(super) fn path_to_file_uri(path: &Path) -> String {
    let path = crate::windows_process::strip_verbatim_prefix(path);
    let rendered = path.display().to_string();
    let mut uri = String::from("file://");
    if !rendered.starts_with('/') {
        uri.push('/');
    }
    uri.push_str(&percent_encode_path(&rendered.replace('\\', "/")));
    uri
}

pub(super) fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    let decoded = percent_decode(path)?;
    #[cfg(windows)]
    let decoded = decoded
        .strip_prefix('/')
        .filter(|path| path.as_bytes().get(1) == Some(&b':'))
        .unwrap_or(&decoded)
        .to_string();
    Some(PathBuf::from(decoded))
}

fn percent_encode_path(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'/'
            | b':'
            | b'-'
            | b'_'
            | b'.'
            | b'~' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn percent_decode(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = path.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_drive_path_has_a_standard_file_uri() {
        assert_eq!(
            path_to_file_uri(Path::new(r"\\?\C:\src\main.rs")),
            "file:///C:/src/main.rs"
        );
    }
}
