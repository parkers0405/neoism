//! Lexical host paths carried over the wire. Never interpret these with the
//! guest's `Path::components`, `join`, `strip_prefix`, canonicalize or stat.
//!
//! An absolute daemon-produced root determines syntax, NOT the guest OS or the
//! host shell. Unix roots retain literal backslashes. Windows drive/UNC roots
//! retain their spelling/case (including verbatim prefixes); we do not guess
//! filesystem case sensitivity or fold aliases. The
//! workspace daemon canonicalizes declared roots on the HOST before publishing
//! them (`workspace::declare_workspace_dir`), so live Windows roots use native
//! drive/UNC syntax, not ambiguous Unix-looking `//` roots. New listings carry
//! host paths directly. This helper also supports older name-only listings.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPath(String);

impl HostPath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Windows roots are unambiguous drive-absolute or backslash UNC paths.
    /// A leading `/` is Unix, even when a filename contains `\` or `C:`.
    pub fn is_windows(&self) -> bool {
        let bytes = self.0.as_bytes();
        self.0.starts_with("\\\\")
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\'))
    }

    /// Join a host-relative path. The relative wire representation may use `/`
    /// for segment separators; backslashes are separators ONLY on Windows.
    /// No case folding, dot-segment collapse, or guest filesystem I/O occurs.
    pub fn join(&self, relative: &str) -> Self {
        if relative.is_empty() {
            return self.clone();
        }
        let windows = self.is_windows();
        let separator = if windows { '\\' } else { '/' };
        let relative = if windows {
            relative.replace(['/', '\\'], &separator.to_string())
        } else {
            relative.to_owned()
        };
        let mut result = self.0.clone();
        if !result.ends_with('/') && !(windows && result.ends_with('\\')) {
            result.push(separator);
        }
        result.push_str(&relative);
        Self(result)
    }

    /// Return a slash-separated relative wire path, respecting HOST syntax.
    /// Exact spelling is intentional: a daemon-produced identity is opaque;
    /// silently folding Windows case could merge case-sensitive directories.
    pub fn relative(&self, path: &str) -> Option<String> {
        let windows = self.is_windows();
        let normalize = |s: &str| {
            if windows {
                s.replace('\\', "/")
            } else {
                s.to_owned()
            }
        };
        let root = normalize(&self.0);
        let path = normalize(path);
        let root = root.trim_end_matches('/');
        if path == root || path == format!("{root}/") {
            return Some(String::new());
        }
        path.strip_prefix(&format!("{root}/")).map(str::to_owned)
    }

    /// Neoism document IDs are opaque strings, not percent-encoded file URIs.
    pub fn buffer_id(&self) -> String {
        format!("file://{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joined_tree_read_crdt_presence_identity_is_guest_independent() {
        let root = HostPath::new("/home/host/项目 space");
        let dir = root.join("src 子目录");
        let file = dir.join("main file.rs");
        // Exercise actual guest PathBuf storage too (this test runs on Windows).
        let stored = std::path::PathBuf::from(file.as_str());
        let read_path = root.relative(stored.to_str().unwrap()).unwrap();
        assert_eq!(read_path, "src 子目录/main file.rs");
        let host_identity = root.join(&read_path).buffer_id();
        assert_eq!(file.buffer_id(), host_identity);
        assert_eq!(
            host_identity,
            "file:///home/host/项目 space/src 子目录/main file.rs"
        );
        assert_eq!(
            root.relative(root.join("README.md").as_str()).unwrap(),
            "README.md"
        );
    }

    #[test]
    fn unix_backslashes_are_filename_bytes_not_separators() {
        let root = HostPath::new("/work");
        let file = root.join(r"src\literal").join("文 件.rs");
        assert_eq!(file.as_str(), "/work/src\\literal/文 件.rs");
        assert_eq!(
            root.relative(file.as_str()).unwrap(),
            "src\\literal/文 件.rs"
        );
        assert_ne!(
            file.buffer_id(),
            root.join("src/literal/文 件.rs").buffer_id()
        );
        assert!(root.relative("/workspace/main.rs").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_host_wire_segments_match_recursive_native_tree_joins() {
        for root in [r"C:\Work", "C:/Work", "C:/", r"\\Server\Share\Work", r"\\?\C:\Work"] {
            let native = std::path::PathBuf::from(root).join("src").join("子 dir").join("main.rs");
            let wire = HostPath::new(root).join("src/子 dir/main.rs");
            assert_eq!(native.to_str().unwrap(), wire.as_str());
        }
    }

    #[test]
    fn windows_drive_unc_and_verbatim_preserve_host_spelling() {
        for root in [
            r"C:\Users\Host\项目",
            r"\\Server\Share\Project",
            r"\\?\C:\Project",
            r"\\?\UNC\Server\Share\Project",
            "C:/Users/Host/项目",
        ] {
            let root = HostPath::new(root);
            assert!(root.is_windows());
            let file = root.join("src/子 dir/main.rs");
            assert_eq!(root.relative(file.as_str()).unwrap(), "src/子 dir/main.rs");
            assert_eq!(root.join(&root.relative(file.as_str()).unwrap()), file);
            assert!(file.as_str().starts_with(root.as_str()));
        }
        assert_eq!(
            HostPath::new(r"C:\Work").join("src/main.rs").as_str(),
            r"C:\Work\src\main.rs"
        );
        // No blind case folding; server listings own the canonical spelling.
        assert!(HostPath::new(r"C:\Work").relative(r"c:\work\x").is_none());
        assert!(!HostPath::new("/work/C:\\literal").is_windows());
    }
}
