use std::path::{Path, PathBuf};

pub(crate) fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    #[cfg(windows)]
    {
        dunce::canonicalize(path)
    }
    #[cfg(not(windows))]
    {
        path.canonicalize()
    }
}

pub(crate) fn canonicalize_lossy(path: &Path) -> PathBuf {
    canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
