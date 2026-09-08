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
    let decoded = windows_decoded_file_path(decoded);
    Some(PathBuf::from(decoded))
}

/// Production uses this only on the Windows host. Keeping the text transform
/// testable on Unix builders avoids leaving UNC coverage compile-only.
#[cfg(any(windows, test))]
fn windows_decoded_file_path(decoded: String) -> String {
    if let Some(drive) = decoded
        .strip_prefix('/')
        .filter(|path| path.as_bytes().get(1) == Some(&b':'))
    {
        // Keep the existing file:///C:/... drive representation.
        drive.to_string()
    } else if decoded.as_bytes().get(1) == Some(&b':') {
        decoded // preserve already-unprefixed drive paths as well
    } else if decoded.starts_with("//") {
        // Legacy native emitter: file://///server/share/path.
        format!(r"\\{}", decoded.trim_start_matches('/').replace('/', r"\"))
    } else if !decoded.starts_with('/') && decoded.contains('/') {
        // Standard authority UNC: file://server/share/path. Without this
        // prefix the engine mistakes it for a workspace-relative path,
        // irreversibly losing the share before an editor bridge sees it.
        format!(r"\\{}", decoded.replace('/', r"\"))
    } else {
        decoded
    }
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

    #[test]
    fn native_uri_windows_unc_authority_is_absolute_not_workspace_relative() {
        for (uri, expected) in [
            (
                "file://Server/Share/space%20%E9%A1%B9%E7%9B%AE/%2520.rs",
                r"\\Server\Share\space 项目\%20.rs",
            ),
            ("file://///Server/Share/file.rs", r"\\Server\Share\file.rs"),
        ] {
            let decoded = PathBuf::from(windows_decoded_file_path(
                percent_decode(uri.strip_prefix("file://").unwrap()).unwrap(),
            ));
            #[cfg(windows)]
            {
                assert!(decoded.is_absolute());
                assert_eq!(file_uri_to_path(uri).unwrap(), decoded);
            }
            assert_eq!(decoded.as_os_str(), std::ffi::OsStr::new(expected));
        }
    }

    #[test]
    fn native_uri_windows_drive_decoding_is_unchanged() {
        let uri = "file:///C:/Work/space%20%E9%A1%B9%E7%9B%AE/%2520.rs";
        let decoded = windows_decoded_file_path(
            percent_decode(uri.strip_prefix("file://").unwrap()).unwrap(),
        );
        assert_eq!(decoded, "C:/Work/space 项目/%20.rs");
        #[cfg(windows)]
        assert_eq!(
            file_uri_to_path(uri).unwrap().as_os_str(),
            std::ffi::OsStr::new(&decoded)
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn native_uri_unix_decoding_is_unchanged() {
        assert_eq!(
            file_uri_to_path("file:///home/host/space%20%E9%A1%B9%E7%9B%AE/%2520.rs")
                .unwrap(),
            PathBuf::from("/home/host/space 项目/%20.rs")
        );
        assert_eq!(
            file_uri_to_path("file:///home/host/literal%5Cname.rs").unwrap(),
            PathBuf::from(r"/home/host/literal\name.rs")
        );
        // This is a Unix-native decoder, not a Windows-guest URI decoder.
        assert_eq!(
            file_uri_to_path("file:///C:/Work/file.rs").unwrap(),
            PathBuf::from("/C:/Work/file.rs")
        );
    }
}
