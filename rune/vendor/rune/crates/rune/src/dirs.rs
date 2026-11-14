//! Where an app keeps its files: the per-user config, data, cache, state and log folders that
//! each platform expects, plus the user's home and Downloads folders.
//!
//! ```no_run
//! let dirs = rune::dirs::AppDirs::new("com.example.notes").expect("a home folder");
//! let drafts = dirs.data().join("drafts.json");
//! let crash_log = dirs.logs().join("crash.log");
//! ```
//!
//! | Folder | macOS, iOS | Windows | elsewhere (XDG) |
//! | --- | --- | --- | --- |
//! | config | `~/Library/Application Support/<id>` | `%APPDATA%\<id>` | `$XDG_CONFIG_HOME/<id>` (`~/.config`) |
//! | data | `~/Library/Application Support/<id>` | `%APPDATA%\<id>` | `$XDG_DATA_HOME/<id>` (`~/.local/share`) |
//! | cache | `~/Library/Caches/<id>` | `%LOCALAPPDATA%\<id>\Cache` | `$XDG_CACHE_HOME/<id>` (`~/.cache`) |
//! | state | `~/Library/Application Support/<id>` | `%LOCALAPPDATA%\<id>` | `$XDG_STATE_HOME/<id>` (`~/.local/state`) |
//! | logs | `~/Library/Logs/<id>` | `%LOCALAPPDATA%\<id>\Logs` | `$XDG_STATE_HOME/<id>/logs` |
//!
//! Home is `HOME`, or `USERPROFILE` on Windows. Windows falls back to `<home>\AppData\Roaming`
//! and `<home>\AppData\Local` when `APPDATA` or `LOCALAPPDATA` is unset, and a relative XDG
//! variable is ignored, as the XDG spec says.
//!
//! [`ROOT_ENV`] (`RUNE_DIRS_ROOT`) moves every folder an app writes under one root,
//! `<root>/{config,data,cache,state,logs}/<id>` and `<root>/downloads`, so a test or an isolated
//! run leaves the real ones alone. [`home`](AppDirs::home) stays the real one: apps read it (to
//! show `~`, to start a folder picker) rather than write to it. [`crate::prefs::Prefs`] honours
//! the root as well.
//!
//! Nothing here touches the disk: the folders are made by whoever writes to them first.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// Puts every folder of every app under this one, for tests and isolated runs.
pub const ROOT_ENV: &str = "RUNE_DIRS_ROOT";

/// An app's folders. Resolve them once at startup with [`AppDirs::new`] (or
/// [`AppDirs::from_env`] with an injected lookup) and pass the value to the code that writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDirs {
    bases: Bases,
    id: String,
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    state: PathBuf,
    logs: PathBuf,
}

impl AppDirs {
    /// The folders of the app with this id (a reverse-DNS name is conventional), from the
    /// process environment. `None` on the web, or when no home folder can be found.
    pub fn new(app_id: &str) -> Option<AppDirs> {
        Self::from_env(app_id, |k| std::env::var_os(k))
    }

    /// [`new`](Self::new) with the environment looked up through `var`, so a caller can resolve
    /// the folders from its own config instead of the process environment.
    pub fn from_env(app_id: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<AppDirs> {
        Some(Bases::resolve(&var)?.app(app_id))
    }

    /// Every folder under `root` (the [`ROOT_ENV`] layout), with the real home from the process
    /// environment.
    pub fn rooted(root: impl Into<PathBuf>, app_id: &str) -> AppDirs {
        let home = home_from(|k| std::env::var_os(k));
        Bases::rooted(root.into(), home).app(app_id)
    }

    /// The folders of another app id resolved from the same environment: a second product
    /// folder of this app, or another app's data this one imports.
    pub fn sibling(&self, app_id: &str) -> AppDirs {
        self.bases.app(app_id)
    }

    /// The folder name these paths end in: the app id with characters a folder can't hold as `_`.
    pub fn id(&self) -> &str { &self.id }

    /// Settings the user chose, roaming with the profile on Windows.
    pub fn config(&self) -> &Path { &self.config }

    /// Documents and records the app keeps, roaming with the profile on Windows.
    pub fn data(&self) -> &Path { &self.data }

    /// Files the app can rebuild, kept on this machine and safe to delete.
    pub fn cache(&self) -> &Path { &self.cache }

    /// Machine-local state that should survive a restart (window places, history, locks).
    pub fn state(&self) -> &Path { &self.state }

    /// Log files.
    pub fn logs(&self) -> &Path { &self.logs }

    /// The user's home folder, if one was found (never moved by [`ROOT_ENV`]).
    pub fn home(&self) -> Option<&Path> { self.bases.home.as_deref() }

    /// Where the user's downloads go (`<root>/downloads` under [`ROOT_ENV`]).
    pub fn downloads(&self) -> Option<&Path> { self.bases.downloads.as_deref() }

    /// Whether [`ROOT_ENV`] (or [`rooted`](Self::rooted)) placed these folders.
    pub fn is_rooted(&self) -> bool { self.bases.rooted }
}

/// The user's home folder from the process environment: `HOME`, or `USERPROFILE` (then
/// `HOMEDRIVE` + `HOMEPATH`) on Windows.
pub fn home() -> Option<PathBuf> {
    home_from(|k| std::env::var_os(k))
}

/// [`home`] with the environment looked up through `var`.
pub fn home_from(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    #[cfg(target_os = "windows")]
    {
        set("USERPROFILE").or_else(|| {
            let mut drive = var("HOMEDRIVE").filter(|v| !v.is_empty())?;
            drive.push(var("HOMEPATH").filter(|v| !v.is_empty())?);
            Some(PathBuf::from(drive))
        })
    }
    #[cfg(not(target_os = "windows"))]
    { set("HOME") }
}

/// The base a [`crate::prefs::Prefs`] app folder goes in, without the id.
pub(crate) fn config_base(var: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    Bases::resolve(var).map(|b| b.config.base)
}

/// One kind of folder: `<base>/<id>`, then `tail` when the platform nests it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Place {
    base: PathBuf,
    tail: Option<&'static str>,
}

impl Place {
    fn at(base: PathBuf) -> Place { Place { base, tail: None } }
    #[cfg_attr(any(target_arch = "wasm32", target_os = "macos", target_os = "ios"), allow(dead_code))]
    fn nested(base: PathBuf, tail: &'static str) -> Place { Place { base, tail: Some(tail) } }
    fn path(&self, id: &str) -> PathBuf {
        let dir = self.base.join(id);
        match self.tail {
            Some(tail) => dir.join(tail),
            None => dir,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Bases {
    config: Place,
    data: Place,
    cache: Place,
    state: Place,
    logs: Place,
    home: Option<PathBuf>,
    downloads: Option<PathBuf>,
    rooted: bool,
}

impl Bases {
    fn app(&self, app_id: &str) -> AppDirs {
        let id = crate::prefs::sanitize(app_id);
        AppDirs {
            config: self.config.path(&id),
            data: self.data.path(&id),
            cache: self.cache.path(&id),
            state: self.state.path(&id),
            logs: self.logs.path(&id),
            bases: self.clone(),
            id,
        }
    }

    fn rooted(root: PathBuf, home: Option<PathBuf>) -> Bases {
        Bases {
            config: Place::at(root.join("config")),
            data: Place::at(root.join("data")),
            cache: Place::at(root.join("cache")),
            state: Place::at(root.join("state")),
            logs: Place::at(root.join("logs")),
            downloads: Some(root.join("downloads")),
            home,
            rooted: true,
        }
    }

    fn resolve(var: &impl Fn(&str) -> Option<OsString>) -> Option<Bases> {
        let set = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = home_from(var);
        if let Some(root) = set(ROOT_ENV) {
            return Some(Bases::rooted(root, home));
        }
        Bases::platform(&set, home)
    }

    #[cfg(target_arch = "wasm32")]
    fn platform(_set: &impl Fn(&str) -> Option<PathBuf>, _home: Option<PathBuf>) -> Option<Bases> { None }

    #[cfg(all(not(target_arch = "wasm32"), any(target_os = "macos", target_os = "ios")))]
    fn platform(_set: &impl Fn(&str) -> Option<PathBuf>, home: Option<PathBuf>) -> Option<Bases> {
        let library = home.as_ref()?.join("Library");
        let support = library.join("Application Support");
        Some(Bases {
            config: Place::at(support.clone()),
            data: Place::at(support.clone()),
            cache: Place::at(library.join("Caches")),
            state: Place::at(support),
            logs: Place::at(library.join("Logs")),
            downloads: home.as_ref().map(|h| h.join("Downloads")),
            home,
            rooted: false,
        })
    }

    #[cfg(all(not(target_arch = "wasm32"), target_os = "windows"))]
    fn platform(set: &impl Fn(&str) -> Option<PathBuf>, home: Option<PathBuf>) -> Option<Bases> {
        let app_data = |k: &str, leaf: &str| set(k).or_else(|| home.as_ref().map(|h| h.join("AppData").join(leaf)));
        let roaming = app_data("APPDATA", "Roaming")?;
        let local = app_data("LOCALAPPDATA", "Local")?;
        Some(Bases {
            config: Place::at(roaming.clone()),
            data: Place::at(roaming),
            cache: Place::nested(local.clone(), "Cache"),
            state: Place::at(local.clone()),
            logs: Place::nested(local, "Logs"),
            downloads: home.as_ref().map(|h| h.join("Downloads")),
            home,
            rooted: false,
        })
    }

    #[cfg(all(not(target_arch = "wasm32"), not(any(target_os = "macos", target_os = "ios", target_os = "windows"))))]
    fn platform(set: &impl Fn(&str) -> Option<PathBuf>, home: Option<PathBuf>) -> Option<Bases> {
        let xdg = |k: &str, fallback: &[&str]| {
            set(k).filter(|p| p.is_absolute()).or_else(|| home.as_ref().map(|h| fallback.iter().fold(h.clone(), |p, part| p.join(part))))
        };
        let state = xdg("XDG_STATE_HOME", &[".local", "state"])?;
        Some(Bases {
            config: Place::at(xdg("XDG_CONFIG_HOME", &[".config"])?),
            data: Place::at(xdg("XDG_DATA_HOME", &[".local", "share"])?),
            cache: Place::at(xdg("XDG_CACHE_HOME", &[".cache"])?),
            logs: Place::nested(state.clone(), "logs"),
            state: Place::at(state),
            downloads: xdg("XDG_DOWNLOAD_DIR", &["Downloads"]),
            home,
            rooted: false,
        })
    }
}
