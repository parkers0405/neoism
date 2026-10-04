use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::{io, AcquisitionError, AcquisitionStage};

/// Hash a tree independently of directory enumeration order and host metadata.
/// Paths use `/`; each regular file contributes its path, byte length, and
/// bytes. Empty directories and `.git` administration data are intentionally
/// ignored, matching the content model of a Git checkout.
pub fn tree_sha256(root: &Path) -> Result<String, AcquisitionError> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|e| io(AcquisitionStage::Checksum, root, e))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(AcquisitionError::UnsafePath(root.to_path_buf()));
    }
    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hash = Sha256::new();
    hash.update(b"neoism-lua-tree-v1\0");
    for (relative, path) in files {
        let mut file = fs::File::open(&path)
            .map_err(|e| io(AcquisitionStage::Checksum, &path, e))?;
        let length = file
            .metadata()
            .map_err(|e| io(AcquisitionStage::Checksum, &path, e))?
            .len();
        hash.update((relative.len() as u64).to_be_bytes());
        hash.update(relative.as_bytes());
        hash.update(length.to_be_bytes());
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|e| io(AcquisitionStage::Checksum, &path, e))?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn file_sha256(path: &Path) -> Result<String, AcquisitionError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| io(AcquisitionStage::Checksum, path, e))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(AcquisitionError::UnsafePath(path.to_path_buf()));
    }
    let mut file =
        fs::File::open(path).map_err(|e| io(AcquisitionStage::Checksum, path, e))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| io(AcquisitionStage::Checksum, path, e))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn collect(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), AcquisitionError> {
    let entries =
        fs::read_dir(dir).map_err(|e| io(AcquisitionStage::Checksum, dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io(AcquisitionStage::Checksum, dir, e))?;
        if dir == root && entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|e| io(AcquisitionStage::Checksum, &path, e))?;
        if metadata.file_type().is_symlink() {
            return Err(AcquisitionError::UnsafePath(path));
        }
        if metadata.is_dir() {
            collect(root, &path, files)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| AcquisitionError::UnsafePath(path.clone()))?;
            let relative = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            files.push((relative, path));
        } else {
            return Err(AcquisitionError::UnsafePath(path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_is_order_independent_and_content_sensitive() {
        let root = temp("checksum");
        fs::create_dir_all(root.join("z")).unwrap();
        fs::write(root.join("z/b.lua"), b"return 2").unwrap();
        fs::write(root.join("a.lua"), b"return 1").unwrap();
        let first = tree_sha256(&root).unwrap();
        fs::remove_file(root.join("a.lua")).unwrap();
        fs::write(root.join("a.lua"), b"return 1").unwrap();
        assert_eq!(first, tree_sha256(&root).unwrap());
        fs::write(root.join("a.lua"), b"return 3").unwrap();
        assert_ne!(first, tree_sha256(&root).unwrap());
        let _ = fs::remove_dir_all(root);
    }

    fn temp(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "neoism-lua-{label}-{}-{}",
            std::process::id(),
            unique()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn unique() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
