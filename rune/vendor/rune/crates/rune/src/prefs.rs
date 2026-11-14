//! A small store for an app's settings (text size, last-used options): JSON in the platform's
//! config directory, read and written with serde.
//!
//! ```no_run
//! let mut prefs = rune::prefs::Prefs::open("com.example.notes");
//! let size: f32 = prefs.get("text_size").unwrap_or(14.0);
//! prefs.set("text_size", &(size + 1.0)).ok();
//! ```
//!
//! The file is `prefs.json` in `~/Library/Application Support/<app id>` on macOS and iOS,
//! `%APPDATA%\<app id>` on Windows and `$XDG_CONFIG_HOME/<app id>` (else `~/.config/<app id>`)
//! elsewhere: [`crate::dirs::AppDirs::config`]. `RUNE_PREFS_DIR` replaces the config directory,
//! and `RUNE_DIRS_ROOT` puts it at `<root>/config`, so a headless run or a test script keeps its
//! settings apart from the real ones. [`Prefs::memory`] keeps nothing on disk.
//!
//! Every `set` and `remove` rewrites the file whole through a temporary file and a rename, so a
//! crash leaves the old settings or the new, never half. Nothing runs in the background and
//! nothing watches the file: a second process's changes show up at the next `open`. A file that
//! doesn't parse is moved to `prefs.json.bad` and the store starts empty. On the web, and where
//! no config directory can be found, the store is in memory and `is_persistent` says so.
use std::{
    collections::BTreeMap,
    fmt, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

const FILE: &str = "prefs.json";
static NEXT_TMP: AtomicU64 = AtomicU64::new(0);

/// Why a change couldn't be saved. The value is still set in memory.
#[derive(Debug)]
pub enum PrefsError {
    /// The value can't be written as JSON.
    Serialize(String),
    /// The file couldn't be written.
    Io(io::Error),
}

impl fmt::Display for PrefsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(why) => write!(f, "can't store the value: {why}"),
            Self::Io(e) => write!(f, "can't save preferences: {e}"),
        }
    }
}
impl std::error::Error for PrefsError {}

/// An app's settings, keyed by name. See the [module docs](self).
#[derive(Debug)]
pub struct Prefs {
    values: BTreeMap<String, Value>,
    path: Option<PathBuf>,
}

impl Prefs {
    /// The settings of the app with this id (a reverse-DNS name is conventional), loaded from
    /// its config directory. Slashes and other characters a folder name can't hold become `_`.
    pub fn open(app_id: &str) -> Prefs {
        match config_dir(|k| std::env::var_os(k)) {
            Some(dir) => Prefs::at(dir.join(sanitize(app_id)).join(FILE)),
            None => Prefs::memory(),
        }
    }

    /// Settings kept in this file, which need not exist yet; its folder is made on the first save.
    pub fn at(path: impl Into<PathBuf>) -> Prefs {
        let path = path.into();
        let values = load(&path);
        Prefs { values, path: Some(path) }
    }

    /// Settings that live only as long as this value, for tests and headless runs.
    pub fn memory() -> Prefs { Prefs { values: BTreeMap::new(), path: None } }

    /// The file this store saves to, if it has one.
    pub fn path(&self) -> Option<&Path> { self.path.as_deref() }

    /// Whether changes reach a file.
    pub fn is_persistent(&self) -> bool { self.path.is_some() }

    /// The value under `key`, or `None` if it is unset or isn't a `T` (a setting written by
    /// another version of the app).
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        serde_json::from_value(self.values.get(key)?.clone()).ok()
    }

    /// Sets `key` and saves. On an error the value is still set for this run.
    pub fn set<T: Serialize + ?Sized>(&mut self, key: &str, value: &T) -> Result<(), PrefsError> {
        let value = serde_json::to_value(value).map_err(|e| PrefsError::Serialize(e.to_string()))?;
        self.values.insert(key.to_owned(), value);
        self.save()
    }

    /// Unsets `key` and saves.
    pub fn remove(&mut self, key: &str) -> Result<(), PrefsError> {
        if self.values.remove(key).is_none() { return Ok(()); }
        self.save()
    }

    /// Whether `key` has a value of any type.
    pub fn contains(&self, key: &str) -> bool { self.values.contains_key(key) }

    fn save(&self) -> Result<(), PrefsError> {
        let Some(path) = &self.path else { return Ok(()) };
        let json = serde_json::to_vec_pretty(&self.values).map_err(|e| PrefsError::Serialize(e.to_string()))?;
        write_atomic(path, &json).map_err(PrefsError::Io)
    }
}

fn load(path: &Path) -> BTreeMap<String, Value> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return BTreeMap::new(),
    };
    if let Ok(values) = serde_json::from_slice(&bytes) { return values; }
    let mut bad = path.as_os_str().to_owned();
    bad.push(".bad");
    if std::fs::rename(path, &bad).is_err() { let _ = std::fs::write(&bad, &bytes); }
    BTreeMap::new()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use io::Write;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    let seq = NEXT_TMP.fetch_add(1, Ordering::Relaxed);
    tmp.push(format!(".{}.{seq}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result
}

pub(crate) fn sanitize(app_id: &str) -> String {
    let id: String = app_id.trim().chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let trimmed = id.trim_end_matches(|c: char| c.is_whitespace() || c == '.');
    if trimmed.is_empty() { "app".into() } else { trimmed.to_owned() }
}

/// The directory app folders live in, from an environment lookup.
fn config_dir(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = set("RUNE_PREFS_DIR") { return Some(dir); }
    crate::dirs::config_base(&var)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("rune-prefs-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Dir(dir)
        }
    }
    impl Drop for Dir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[test]
    fn values_round_trip_through_the_file() {
        let dir = Dir::new("roundtrip");
        let path = dir.0.join("app").join(FILE);
        let mut prefs = Prefs::at(&path);
        assert_eq!(prefs.get::<f32>("text_size"), None);
        prefs.set("text_size", &15.5f32).unwrap();
        prefs.set("window", &(800u32, 600u32)).unwrap();
        prefs.set("recent", &["a", "b"]).unwrap();
        let again = Prefs::at(&path);
        assert_eq!(again.get::<f32>("text_size"), Some(15.5));
        assert_eq!(again.get("window"), Some((800u32, 600u32)));
        assert_eq!(again.get::<Vec<String>>("recent"), Some(vec!["a".into(), "b".into()]));
        assert_eq!(again.get::<String>("text_size"), None, "a value of another type reads as unset");
        let mut prefs = again;
        prefs.remove("text_size").unwrap();
        assert!(!Prefs::at(&path).contains("text_size"));
        assert!(!path.with_file_name("prefs.json.tmp").exists(), "no temporary file is left behind");
    }

    #[test]
    fn a_corrupt_file_starts_empty_and_is_kept_as_bad() {
        let dir = Dir::new("corrupt");
        std::fs::create_dir_all(&dir.0).unwrap();
        let path = dir.0.join(FILE);
        std::fs::write(&path, b"{ not json").unwrap();
        let mut prefs = Prefs::at(&path);
        assert!(!prefs.contains("anything"));
        assert_eq!(std::fs::read(dir.0.join("prefs.json.bad")).unwrap(), b"{ not json");
        prefs.set("k", &1u8).unwrap();
        assert_eq!(Prefs::at(&path).get::<u8>("k"), Some(1));

        std::fs::write(&path, b"[1, 2]").unwrap();
        assert!(!Prefs::at(&path).contains("k"), "a file that isn't an object is corrupt too");
        assert_eq!(std::fs::read(dir.0.join("prefs.json.bad")).unwrap(), b"[1, 2]");
    }

    #[test]
    fn memory_stores_touch_no_file() {
        let mut prefs = Prefs::memory();
        prefs.set("k", "v").unwrap();
        assert_eq!(prefs.get::<String>("k").as_deref(), Some("v"));
        assert!(!prefs.is_persistent() && prefs.path().is_none());
    }

    #[test]
    fn app_ids_stay_inside_the_config_directory() {
        assert_eq!(sanitize("com.example.notes"), "com.example.notes");
        assert_eq!(sanitize("../evil/id"), ".._evil_id");
        assert_eq!(sanitize(".."), "app");
        assert_eq!(sanitize("..."), "app");
        assert_eq!(sanitize("com.example."), "com.example");
        assert_eq!(sanitize(""), "app");
    }

    #[test]
    fn concurrent_writers_do_not_clobber_temp_files() {
        let dir = Dir::new("concurrent");
        let path = dir.0.join(FILE);
        std::thread::scope(|s| {
            for t in 0..4u32 {
                let path = &path;
                s.spawn(move || {
                    let mut prefs = Prefs::at(path);
                    for i in 0..20u32 {
                        prefs.set(&format!("k_{t}"), &(t * 100 + i)).unwrap();
                    }
                });
            }
        });
        let final_prefs = Prefs::at(&path);
        assert!(final_prefs.is_persistent());
        assert!(!dir.0.join("prefs.json.bad").exists());
    }

    #[test]
    fn the_config_directory_follows_the_platform_and_the_override() {
        let env = |pairs: &'static [(&str, &str)]| move |k: &str| pairs.iter().find(|p| p.0 == k).map(|p| p.1.into());
        assert_eq!(config_dir(env(&[("RUNE_PREFS_DIR", "/scratch"), ("HOME", "/home/x")])), Some(PathBuf::from("/scratch")));
        assert_eq!(config_dir(env(&[])), None);
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        assert_eq!(config_dir(env(&[("HOME", "/h")])), Some(PathBuf::from("/h/Library/Application Support")));
        #[cfg(target_os = "windows")]
        assert_eq!(config_dir(env(&[("APPDATA", "C:\\Roaming")])), Some(PathBuf::from("C:\\Roaming")));
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
        {
            assert_eq!(config_dir(env(&[("HOME", "/h")])), Some(PathBuf::from("/h/.config")));
            assert_eq!(config_dir(env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "/xdg")])), Some(PathBuf::from("/xdg")));
            assert_eq!(config_dir(env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "rel")])), Some(PathBuf::from("/h/.config")));
        }
    }
}
