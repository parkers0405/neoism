//! Device-local pictures and settings. The paired workspace never receives either.
use crate::{app::App, palette::backgrounds::{self, Choice}, ui::wallpaper::Wallpaper};
use rune::Tx;
use serde_json::{Value, json};
use std::{io::Read, path::{Path, PathBuf}, sync::{Arc, Mutex}};

const WINDOW: &str = "ui.window.";
const IMAGE: &str = "background-image";
const DISABLED: &str = "background-image-disabled";
const MAX_BYTES: usize = 32 * 1024 * 1024;

type ImportResult = Result<PathBuf, String>;

#[derive(Default)]
pub(crate) struct State {
    pub config: Value,
    pub error: Option<String>,
    pub busy: bool,
    result: Arc<Mutex<Option<ImportResult>>>,
}

fn directory() -> PathBuf { crate::config::dir().join("mobile-backgrounds") }

pub(crate) fn catalog() -> Vec<backgrounds::Background> { backgrounds::catalog(&directory(), None) }

impl App {
    pub(crate) fn load_mobile_background(&mut self) {
        self.mobile_backgrounds.config = std::fs::read(directory().join("settings.json")).ok()
            .and_then(|data| serde_json::from_slice(&data).ok()).unwrap_or_else(|| json!({"ui": {"window": {}}}));
        // Resolve against the current sandbox; iOS can change its absolute container path.
        if let Some(name) = self.mobile_backgrounds.config["ui"]["window"][IMAGE]["path"].as_str()
            .and_then(|path| Path::new(path).file_name()).map(|name| directory().join("bg").join(name)) {
            self.mobile_backgrounds.config["ui"]["window"][IMAGE]["path"] = json!(name);
        }
        self.wallpaper = Wallpaper::from_config(&self.mobile_backgrounds.config);
    }

    fn update_mobile_background(&mut self, commit: bool) {
        let config = &self.mobile_backgrounds.config;
        self.wallpaper = Wallpaper::from_config(config);
        if let Some(settings) = self.settings.as_mut() {
            for key in [IMAGE, DISABLED] { settings.set_local(&format!("{WINDOW}{key}"), config["ui"]["window"][key].clone()); }
            settings.background_choice = None;
        }
        if commit {
            let dir = directory();
            let result = (|| -> std::io::Result<()> {
                std::fs::create_dir_all(&dir)?;
                let mut file = tempfile::NamedTempFile::new_in(&dir)?;
                serde_json::to_writer(file.as_file_mut(), config)?;
                file.persist(dir.join("settings.json"))?;
                Ok(())
            })();
            self.mobile_backgrounds.error = result.err().map(|err| format!("Couldn't save background settings: {err}"));
        }
    }

    pub(crate) fn toggle_mobile_background(&mut self) {
        let window = &mut self.mobile_backgrounds.config["ui"]["window"];
        if !window[IMAGE]["path"].is_string() { return; }
        window[DISABLED] = json!(window[DISABLED].as_bool() != Some(true));
        self.update_mobile_background(true);
    }

    pub(crate) fn choose_mobile_background(&mut self, choice: Choice) {
        let window = &mut self.mobile_backgrounds.config["ui"]["window"];
        match choice {
            Choice::Image(path) => {
                window[IMAGE]["path"] = json!(path);
                window[DISABLED] = json!(false);
            }
            _ => window[DISABLED] = json!(true),
        }
        self.update_mobile_background(true);
    }

    pub(crate) fn set_mobile_background_field(&mut self, field: &str, value: Value, commit: bool) {
        let image = &mut self.mobile_backgrounds.config["ui"]["window"][IMAGE];
        image[field] = value;
        self.update_mobile_background(commit);
    }

    pub(crate) fn poll_background_import(&mut self) -> bool {
        let Some(result) = self.mobile_backgrounds.result.lock().unwrap().take() else { return false };
        self.mobile_backgrounds.busy = false;
        match result {
            Ok(path) => {
                if let Some(settings) = self.settings.as_mut() {
                    settings.backgrounds = catalog();
                    settings.background_hover = None;
                }
                self.choose_mobile_background(Choice::Image(path));
            }
            Err(error) => self.mobile_backgrounds.error = Some(error),
        }
        true
    }

    fn import_background(&mut self, source: Source) {
        if self.mobile_backgrounds.busy { return; }
        self.mobile_backgrounds.busy = true;
        self.mobile_backgrounds.error = None;
        let result = self.mobile_backgrounds.result.clone();
        let wake = self.waker.clone();
        std::thread::spawn(move || {
            *result.lock().unwrap() = Some(import_into(source, &directory().join("bg")).map_err(|e| format!("Couldn't import picture: {e}")));
            if let Some(wake) = wake { wake(); }
        });
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn import_background_photo(&mut self, name: String, bytes: Vec<u8>) {
        self.import_background(Source::Bytes(bytes, name));
    }
}

enum Source { Path(PathBuf), Bytes(Vec<u8>, String) }

fn import_into(source: Source, dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let (bytes, name) = match source {
        Source::Path(path) => {
            let mut bytes = Vec::new();
            std::fs::File::open(&path)?.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
            (bytes, path.file_stem().unwrap_or_default().to_string_lossy().chars().take(80).collect::<String>())
        }
        Source::Bytes(bytes, name) => {
            let stem = Path::new(&name).file_stem().and_then(|stem| stem.to_str()).unwrap_or("Picture");
            (bytes, stem.chars().take(80).collect())
        }
    };
    if bytes.len() > MAX_BYTES { return Err("the file exceeds 32 MB".into()); }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let picture = reader.decode()?.thumbnail(1920, 1920).into_rgba8();
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::Builder::new().prefix(&format!("{name}-")).suffix(".png").tempfile_in(dir)?;
    picture.write_to(file.as_file_mut(), image::ImageFormat::Png)?;
    let saved = file.keep()?.1;
    Ok(std::fs::canonicalize(&saved).unwrap_or(saved))
}

#[cfg(not(target_os = "ios"))]
pub(super) fn choose_picture(_: &mut App, tx: &mut Tx) {
    let options = rune::FileDialogOptions { title: "Choose a background picture".into(), ..Default::default() }
        .filter("Pictures", &["png", "jpg", "jpeg", "webp", "gif", "bmp"]);
    tx.open_file(options, |app: &mut App, result, _| match result {
        Ok(Some(path)) => app.import_background(Source::Path(path)),
        Ok(None) => {},
        Err(error) => app.mobile_backgrounds.error = Some(error.to_string()),
    });
}

#[cfg(target_os = "ios")]
pub(super) fn choose_picture(app: &mut App, _: &mut Tx) {
    crate::mobile_photo::choose_background(app);
}

pub(super) fn paste_picture(app: &mut App, tx: &mut Tx) {
    #[cfg(target_os = "ios")]
    {
        let _ = tx;
        if let Some(bytes) = ios_picture() { app.import_background(Source::Bytes(bytes, "Pasted picture".into())); }
        else { app.mobile_backgrounds.error = Some("Copy a picture in Photos, then paste it here (32 MB maximum).".into()); }
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = app;
        tx.read_clipboard(|app: &mut App, content, _| {
            if let Some(image) = content.image() { app.import_background(Source::Bytes(image.png().to_vec(), "Pasted picture".into())); }
            else if let Some(path) = content.files().first() { app.import_background(Source::Path(path.clone())); }
            else { app.mobile_backgrounds.error = Some("No picture found on the clipboard.".into()); }
        });
    }
}

#[cfg(target_os = "ios")]
fn ios_picture() -> Option<Vec<u8>> {
    use std::ffi::{c_char, c_void};
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *const c_void;
        fn objc_msgSend();
    }
    #[link(name = "UIKit", kind = "framework")]
    unsafe extern "C" { fn UIImagePNGRepresentation(image: *mut c_void) -> *mut c_void; }
    // Called from the UI thread after an explicit Paste; copy the autoreleased NSData now.
    unsafe {
        let send: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void = std::mem::transmute(objc_msgSend as *const ());
        let length: unsafe extern "C" fn(*mut c_void, *const c_void) -> usize = std::mem::transmute(objc_msgSend as *const ());
        let board = send(objc_getClass(c"UIPasteboard".as_ptr()), sel_registerName(c"generalPasteboard".as_ptr()));
        let image = send(board, sel_registerName(c"image".as_ptr()));
        if image.is_null() { return None; }
        let data = UIImagePNGRepresentation(image);
        let size = length(data, sel_registerName(c"length".as_ptr()));
        if !(1..=MAX_BYTES).contains(&size) { return None; }
        let bytes = send(data, sel_registerName(c"bytes".as_ptr()));
        (!bytes.is_null()).then(|| std::slice::from_raw_parts(bytes.cast::<u8>(), size).to_vec())
    }
}