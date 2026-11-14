//! Drags from other apps onto a window, on macOS. winit's window takes only file paths, so a
//! drag carrying none was refused: the screenshot thumbnail, Photos and browsers' images send a
//! file promise, Safari and canvases bare image data. The content view becomes the window's
//! drag destination instead (a per-instance subclass, as `own_window_dragging` makes): a file
//! promise is received into a temporary folder and image data is written there as a file, so a
//! drop still reaches `El::on_drop_files` as paths. AppKit says where the drag is at every step,
//! so the host needn't sample the pointer.
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, msg_send_id, sel};
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSView};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSPoint, NSString, NSURL};

/// One step of a drag over a content view, in points from its top-left.
#[derive(Debug, PartialEq)]
pub(crate) enum Step {
    Enter(Vec<PathBuf>, (f32, f32)),
    Move((f32, f32)),
    Drop(Vec<PathBuf>, (f32, f32)),
    Leave,
}

#[derive(Default)]
struct Dest {
    steps: Vec<Step>,
    /// A drag this view takes is over it.
    over: bool,
}

thread_local! {
    /// Each content view's steps since its host last took them, by the view's address.
    static DESTS: RefCell<HashMap<usize, Dest>> = RefCell::default();
}

const FILE_URL: &str = "public.file-url";
const PROMISED_TYPE: &str = "com.apple.pasteboard.promised-file-content-type";
/// Image data a drop writes to a file, by pasteboard type, with the file's extension. TIFF, what
/// most Mac apps drag, is written as PNG.
const IMAGES: [(&str, &str); 6] = [
    ("public.png", "png"), ("public.jpeg", "jpg"), ("com.compuserve.gif", "gif"),
    ("org.webmproject.webp", "webp"), ("public.heic", "heic"), ("public.tiff", "png"),
];

/// What one pasteboard item gives a drop.
#[derive(Debug, PartialEq)]
enum Item {
    File(PathBuf),
    /// A file the source writes when asked, with its type's extension when known.
    Promise(Option<&'static str>),
    /// Image data: its pasteboard type and file extension.
    Image(&'static str, &'static str),
}

impl Item {
    /// The path a drag over the window reports: the file, else a stand-in named for its type,
    /// which is all an `El::accepts_files` filter reads.
    fn hover_path(&self) -> PathBuf {
        match self {
            Item::File(path) => path.clone(),
            Item::Promise(ext) => PathBuf::from(ext.map_or("File".into(), |e| format!("File.{e}"))),
            Item::Image(_, ext) => PathBuf::from(format!("Image.{ext}")),
        }
    }
}

/// The content view of `window` takes drags of files, file promises and image data.
pub(crate) fn install(window: &winit::window::Window) {
    let Some(view) = crate::platform::macos::content_view(window) else { return };
    // SAFETY: main thread (windows are created there), on winit's live content view.
    unsafe {
        let base = view.class();
        if !base.name().contains("RuneDrops_") {
            let name = format!("RuneDrops_{}", base.name());
            let class = match ClassBuilder::new(&name, base) {
                Some(mut b) => {
                    b.add_method(sel!(draggingEntered:), entered as unsafe extern "C" fn(_, _, _) -> _);
                    b.add_method(sel!(draggingUpdated:), updated as unsafe extern "C" fn(_, _, _) -> _);
                    b.add_method(sel!(draggingExited:), exited as unsafe extern "C" fn(_, _, _));
                    b.add_method(sel!(prepareForDragOperation:), prepare as unsafe extern "C" fn(_, _, _) -> _);
                    b.add_method(sel!(performDragOperation:), perform as unsafe extern "C" fn(_, _, _) -> _);
                    b.register()
                }
                None => match AnyClass::get(&name) { Some(c) => c, None => return },
            };
            // Only this instance's class changes, to a subclass without ivars.
            AnyObject::set_class(&view, class);
        }
        view.registerForDraggedTypes(&NSArray::from_vec(registered_types()));
    }
}

/// The steps of drags over `window` since the last call.
pub(crate) fn take(window: &winit::window::Window) -> Vec<Step> {
    let Some(view) = crate::platform::macos::content_view(window) else { return Vec::new() };
    let key = Retained::as_ptr(&view) as usize;
    DESTS.with(|d| d.borrow_mut().get_mut(&key).map(|d| std::mem::take(&mut d.steps)).unwrap_or_default())
}

fn push(key: usize, step: Step) {
    DESTS.with(|d| {
        let mut d = d.borrow_mut();
        let dest = d.entry(key).or_default();
        // A move replaces the move before it: the host wants only the latest position.
        if matches!(step, Step::Move(_)) && matches!(dest.steps.last(), Some(Step::Move(_))) { dest.steps.pop(); }
        match step {
            Step::Enter(..) | Step::Move(_) => dest.over = true,
            Step::Drop(..) | Step::Leave => dest.over = false,
        }
        dest.steps.push(step);
    });
}

fn over(key: usize) -> bool { DESTS.with(|d| d.borrow().get(&key).is_some_and(|d| d.over)) }

fn registered_types() -> Vec<Retained<NSString>> {
    let mut types: Vec<Retained<NSString>> = [FILE_URL, "NSFilenamesPboardType"].into_iter()
        .chain(IMAGES.iter().map(|(t, _)| *t)).map(NSString::from_str).collect();
    types.extend(promise_types());
    types
}

/// The types a file promise's item carries, new (`NSFilePromiseProvider`) and old.
fn promise_types() -> Vec<Retained<NSString>> {
    // SAFETY: a class method returning an array of strings.
    unsafe {
        let types: Retained<NSArray<NSString>> = msg_send_id![class!(NSFilePromiseReceiver), readableDraggedTypes];
        types.to_vec_retained()
    }
}

/// The operation a drag gets: copy, else what the source calls generic, else none (a Rune
/// window's own drag-out offers nothing to the app).
fn operation(mask: usize) -> usize {
    const COPY: usize = 1;
    const GENERIC: usize = 4;
    if mask & COPY != 0 { COPY } else if mask & GENERIC != 0 { GENERIC } else { 0 }
}

/// What each item of `pb` gives a drop, in order. An item that gives nothing is left out.
unsafe fn items(pb: &NSPasteboard) -> Vec<(Retained<NSPasteboardItem>, Item)> {
    let promises: Vec<String> = promise_types().iter().map(|t| t.to_string()).collect();
    let Some(list) = (unsafe { pb.pasteboardItems() }) else { return Vec::new() };
    list.to_vec_retained().into_iter().filter_map(|item| {
        let types: Vec<String> = unsafe { item.types() }.iter().map(|t| t.to_string()).collect();
        let has = |t: &str| types.iter().any(|x| x == t);
        let found = if has(FILE_URL) {
            let url = unsafe { item.stringForType(&NSString::from_str(FILE_URL)) }?;
            file_path(&url).map(Item::File)
        } else if types.iter().any(|t| promises.contains(t)) {
            let uti = unsafe { item.stringForType(&NSString::from_str(PROMISED_TYPE)) }.map(|s| s.to_string());
            Some(Item::Promise(uti.as_deref().and_then(extension)))
        } else {
            IMAGES.iter().find(|(t, _)| has(t)).map(|&(t, ext)| Item::Image(t, ext))
        };
        found.map(|f| (item, f))
    }).collect()
}

/// The path of a `file://` URL, resolving the file-reference form (`file:///.file/id=…`).
pub(crate) fn file_path(url: &NSString) -> Option<PathBuf> {
    // SAFETY: Foundation URL calls with live strings.
    unsafe {
        let url = NSURL::URLWithString(url)?;
        let url = url.filePathURL()?;
        Some(PathBuf::from(url.path()?.to_string()))
    }
}

/// A file extension for a few common types a promise names.
fn extension(uti: &str) -> Option<&'static str> {
    Some(match uti {
        "public.png" => "png",
        "public.jpeg" => "jpg",
        "public.tiff" => "tiff",
        "public.heic" => "heic",
        "com.compuserve.gif" => "gif",
        "org.webmproject.webp" => "webp",
        "com.adobe.pdf" => "pdf",
        "public.plain-text" | "public.utf8-plain-text" => "txt",
        "com.apple.quicktime-movie" => "mov",
        "public.mpeg-4" => "mp4",
        _ => return None,
    })
}

/// A new folder for one drop's files under the system's temporary folder.
fn drop_dir() -> Option<PathBuf> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let dir = std::env::temp_dir().join("rune-drops").join(format!("{}-{stamp}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).ok().map(|_| dir)
}

/// `stem.ext` in `dir`, numbered when the name is taken.
fn free_name(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    (1..).map(|i| dir.join(if i == 1 { format!("{stem}.{ext}") } else { format!("{stem} {i}.{ext}") }))
        .find(|p| !p.exists()).unwrap_or_else(|| dir.join(format!("{stem}.{ext}")))
}

/// The name an image takes from the URL it came from, if the item has one (a browser's image).
fn image_stem(item: &NSPasteboardItem) -> String {
    // SAFETY: reading a string from a live pasteboard item.
    let url = unsafe { item.stringForType(&NSString::from_str("public.url")) }.map(|u| u.to_string());
    url.as_deref().and_then(url_stem).unwrap_or_else(|| "Image".into())
}

fn url_stem(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let (_, path) = rest.split(['?', '#']).next()?.split_once('/')?;
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    let stem = last.rsplit_once('.').map_or(last, |(s, _)| s);
    let stem: String = stem.chars().filter(|c| !matches!(c, ':' | '\\' | '\0')).take(80).collect();
    (!stem.is_empty() && !stem.starts_with('.')).then_some(stem)
}

/// Writes an item's image to `dir`, as PNG when it's TIFF.
unsafe fn write_image(item: &NSPasteboardItem, kind: &str, ext: &str, dir: &Path) -> Option<PathBuf> {
    let data = unsafe { item.dataForType(&NSString::from_str(kind)) }?;
    let bytes = data.bytes();
    let png;
    let bytes = if kind == "public.tiff" {
        png = crate::clipboard::tiff_to_png(bytes)?;
        png.png()
    } else { bytes };
    let path = free_name(dir, &image_stem(item), ext);
    std::fs::write(&path, bytes).ok().map(|_| path)
}

/// The drag's position in `view`, in points from its top-left.
unsafe fn location(view: &NSView, info: &AnyObject) -> (f32, f32) {
    let p: NSPoint = unsafe { msg_send![info, draggingLocation] };
    let p = view.convertPoint_fromView(p, None);
    let y = if view.isFlipped() { p.y } else { view.bounds().size.height - p.y };
    (p.x as f32, y as f32)
}

fn key(view: &AnyObject) -> usize { view as *const AnyObject as usize }

unsafe fn view(this: &AnyObject) -> &NSView {
    // SAFETY: these methods are only on the content view's subclass.
    unsafe { &*(this as *const AnyObject).cast::<NSView>() }
}

unsafe fn pasteboard(info: &AnyObject) -> Retained<NSPasteboard> { unsafe { msg_send_id![info, draggingPasteboard] } }

/// A drag entered or moved: Enter when it starts to be taken, then Move.
unsafe fn update(this: &AnyObject, info: &AnyObject) -> usize {
    autoreleasepool(|_| unsafe {
        let key = key(this);
        let mask: usize = msg_send![info, draggingSourceOperationMask];
        let op = operation(mask);
        let at = location(view(this), info);
        if op == 0 {
            if over(key) { push(key, Step::Leave); }
            return 0;
        }
        if over(key) {
            push(key, Step::Move(at));
            return op;
        }
        let paths: Vec<PathBuf> = items(&pasteboard(info)).iter().map(|(_, item)| item.hover_path()).collect();
        if paths.is_empty() { return 0; }
        push(key, Step::Enter(paths, at));
        op
    })
}

unsafe extern "C" fn entered(this: &AnyObject, _: Sel, info: &AnyObject) -> usize { unsafe { update(this, info) } }

unsafe extern "C" fn updated(this: &AnyObject, _: Sel, info: &AnyObject) -> usize { unsafe { update(this, info) } }

unsafe extern "C" fn exited(this: &AnyObject, _: Sel, _info: *mut AnyObject) {
    if over(key(this)) { push(key(this), Step::Leave); }
}

unsafe extern "C" fn prepare(this: &AnyObject, _: Sel, _info: &AnyObject) -> Bool { Bool::new(over(key(this))) }

/// The drop: files at once, image data once written, and file promises once their sources
/// have written them, all in one `Step::Drop`.
unsafe extern "C" fn perform(this: &AnyObject, _: Sel, info: &AnyObject) -> Bool {
    let key = key(this);
    autoreleasepool(|_| unsafe {
        let at = location(view(this), info);
        let pb = pasteboard(info);
        let items = items(&pb);
        let mut dir = None;
        let mut paths = Vec::new();
        let mut promised = false;
        for (item, found) in &items {
            match found {
                Item::File(path) => paths.push(path.clone()),
                Item::Image(kind, ext) => {
                    if dir.is_none() { dir = drop_dir(); }
                    if let Some(path) = dir.as_deref().and_then(|d| write_image(item, kind, ext, d)) { paths.push(path); }
                }
                Item::Promise(_) => promised = true,
            }
        }
        if promised && let Some(dir) = dir.or_else(drop_dir) && receive(key, &pb, &dir, paths.clone(), at) {
            return Bool::YES;
        }
        if paths.is_empty() {
            push(key, Step::Leave);
            return Bool::NO;
        }
        push(key, Step::Drop(paths, at));
        Bool::YES
    })
}

/// Asks each promise's source to write its files into `dir`. Once all have answered, `paths`
/// and the promised files drop at `at`.
unsafe fn receive(key: usize, pb: &NSPasteboard, dir: &Path, paths: Vec<PathBuf>, at: (f32, f32)) -> bool {
    unsafe {
        let classes = NSArray::from_vec(vec![Retained::<AnyObject>::retain(class!(NSFilePromiseReceiver) as *const AnyClass as *mut AnyObject).unwrap()]);
        let Some(receivers): Option<Retained<NSArray<AnyObject>>> = msg_send_id![pb, readObjectsForClasses: &*classes, options: &*NSDictionary::<NSString, AnyObject>::new()]
        else { return false };
        let receivers = receivers.to_vec_retained();
        let files: usize = receivers.iter().map(|r| {
            let types: Retained<NSArray<NSString>> = msg_send_id![r, fileTypes];
            types.count().max(1)
        }).sum();
        if files == 0 { return false; }
        // The sources write off the main thread; the drop is queued back on it.
        let queue: Retained<AnyObject> = msg_send_id![class!(NSOperationQueue), new];
        let pending = Arc::new(Mutex::new((files, paths)));
        let destination = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&dir.to_string_lossy()), true);
        for receiver in &receivers {
            let pending = pending.clone();
            // The receiver lives until its files are in.
            let keep = receiver.clone();
            let reader = block2::RcBlock::new(move |url: *mut NSURL, error: *mut NSError| {
                let _ = &keep;
                let path = if error.is_null() { url.as_ref().and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string())) } else { None };
                let mut p = pending.lock().unwrap_or_else(|e| e.into_inner());
                p.0 = p.0.saturating_sub(1);
                p.1.extend(path);
                if p.0 > 0 { return; }
                let paths = std::mem::take(&mut p.1);
                drop(p);
                let deliver = block2::RcBlock::new(move || {
                    if paths.is_empty() { push(key, Step::Leave) } else { push(key, Step::Drop(paths.clone(), at)) }
                });
                let main: Retained<AnyObject> = msg_send_id![class!(NSOperationQueue), mainQueue];
                let _: () = msg_send![&main, addOperationWithBlock: &*deliver];
            });
            let _: () = msg_send![receiver, receivePromisedFilesAtDestination: &*destination, options: &*NSDictionary::<NSString, AnyObject>::new(),
                operationQueue: &*queue, reader: &*reader];
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::NSData;

    /// Whether this session has a pasteboard server. A headless or SSH-only session (no window
    /// server) has none, and `pasteboardWithUniqueName` returns nil there, so the round-trip
    /// tests skip with a note instead of panicking.
    fn pasteboard_available() -> bool {
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *AVAILABLE.get_or_init(|| {
            let probe = std::panic::catch_unwind(|| unsafe {
                let pb: Option<Retained<NSPasteboard>> = msg_send_id![class!(NSPasteboard), pasteboardWithUniqueName];
                let Some(pb) = pb else { return false };
                pb.clearContents();
                let item = NSPasteboardItem::new();
                let kind = NSString::from_str("public.utf8-plain-text");
                let ok = item.setString_forType(&NSString::from_str("probe"), &kind)
                    && pb.writeObjects(&NSArray::from_vec(vec![objc2::runtime::ProtocolObject::from_retained(item)]))
                    && pb.stringForType(&kind).is_some_and(|s| s.to_string() == "probe");
                let () = msg_send![&*pb, releaseGlobally];
                ok
            });
            let ok = probe.unwrap_or(false);
            if !ok { eprintln!("macos_drop: no pasteboard in this session (headless?); skipping pasteboard round-trip tests"); }
            ok
        })
    }

    /// A private pasteboard, released when dropped.
    struct Board(Retained<NSPasteboard>);

    impl Drop for Board {
        fn drop(&mut self) { unsafe { msg_send![&*self.0, releaseGlobally] } }
    }

    impl std::ops::Deref for Board {
        type Target = NSPasteboard;
        fn deref(&self) -> &NSPasteboard { &self.0 }
    }

    /// A private pasteboard holding `items`, or None without a pasteboard.
    fn board(items: &[&[(&str, &[u8])]]) -> Option<Board> {
        if !pasteboard_available() { return None; }
        unsafe {
            let pb = Board(NSPasteboard::pasteboardWithUniqueName());
            pb.clearContents();
            let items: Vec<Retained<NSPasteboardItem>> = items.iter().map(|types| {
                let item = NSPasteboardItem::new();
                for (t, data) in *types { item.setData_forType(&NSData::with_bytes(data), &NSString::from_str(t)); }
                item
            }).collect();
            let objects: Vec<Retained<objc2::runtime::ProtocolObject<dyn objc2_app_kit::NSPasteboardWriting>>> =
                items.into_iter().map(|i| objc2::runtime::ProtocolObject::from_retained(i)).collect();
            assert!(pb.writeObjects(&NSArray::from_vec(objects)));
            Some(pb)
        }
    }

    fn png() -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, 2, 1);
        e.set_color(png::ColorType::Rgba);
        e.write_header().unwrap().write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        out
    }

    fn found(pb: &NSPasteboard) -> Vec<Item> { unsafe { items(pb) }.into_iter().map(|(_, i)| i).collect() }

    #[test]
    fn a_file_url_is_its_path() {
        let Some(pb) = board(&[&[(FILE_URL, b"file:///tmp/notes%20one.txt")]]) else { return };
        assert_eq!(found(&pb), [Item::File("/tmp/notes one.txt".into())]);
    }

    #[test]
    fn image_data_without_a_file_is_an_image() {
        let Some(pb) = board(&[&[("public.png", &png())], &[("public.url", b"https://e.com/a.png"), ("public.tiff", b"II*\0")]]) else { return };
        assert_eq!(found(&pb), [Item::Image("public.png", "png"), Item::Image("public.tiff", "png")]);
    }

    #[test]
    fn a_file_url_wins_over_the_image_beside_it() {
        let Some(pb) = board(&[&[(FILE_URL, b"file:///tmp/a.png"), ("public.png", &png())]]) else { return };
        assert_eq!(found(&pb), [Item::File("/tmp/a.png".into())]);
    }

    #[test]
    fn a_promise_is_named_for_its_type() {
        let Some(pb) = board(&[&[("com.apple.NSFilePromiseItemMetaData", b"x"), (PROMISED_TYPE, b"public.png")]]) else { return };
        assert_eq!(found(&pb), [Item::Promise(Some("png"))]);
    }

    #[test]
    fn hover_paths_stand_in_for_files_to_come() {
        assert_eq!(Item::File("/tmp/a.txt".into()).hover_path(), PathBuf::from("/tmp/a.txt"));
        assert_eq!(Item::Promise(Some("png")).hover_path(), PathBuf::from("File.png"));
        assert_eq!(Item::Promise(None).hover_path(), PathBuf::from("File"));
        assert_eq!(Item::Image("public.tiff", "png").hover_path(), PathBuf::from("Image.png"));
    }

    #[test]
    fn promised_types_map_to_extensions() {
        assert_eq!(extension("public.jpeg"), Some("jpg"));
        assert_eq!(extension("com.adobe.pdf"), Some("pdf"));
        assert_eq!(extension("public.data"), None);
    }

    #[test]
    fn file_urls_resolve_to_paths() {
        assert_eq!(file_path(&NSString::from_str("file:///tmp/notes%20one.txt")), Some(PathBuf::from("/tmp/notes one.txt")));
        assert_eq!(file_path(&NSString::from_str("https://e.com/a.png")), None);
    }

    #[test]
    fn taken_names_are_numbered() {
        let dir = drop_dir().unwrap();
        assert_eq!(free_name(&dir, "Image", "png"), dir.join("Image.png"));
        std::fs::write(dir.join("Image.png"), b"").unwrap();
        std::fs::write(dir.join("Image 2.png"), b"").unwrap();
        assert_eq!(free_name(&dir, "Image", "png"), dir.join("Image 3.png"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn text_alone_gives_nothing() {
        let Some(pb) = board(&[&[("public.utf8-plain-text", b"hello")], &[("public.url", b"https://e.com/")]]) else { return };
        assert!(found(&pb).is_empty());
    }

    #[test]
    fn images_are_written_named_for_their_url() {
        let Some(pb) = board(&[&[("public.png", &png())], &[("public.png", &png())], &[("public.url", b"https://e.com/img/kitten.jpg?w=2"), ("public.png", &png())]]) else { return };
        let dir = drop_dir().unwrap();
        let paths: Vec<PathBuf> = unsafe { items(&pb) }.iter()
            .filter_map(|(item, f)| match f { Item::Image(k, e) => unsafe { write_image(item, k, e, &dir) }, _ => None }).collect();
        let names: Vec<_> = paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["Image.png", "Image 2.png", "kitten.png"]);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), png());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn url_stems() {
        assert_eq!(url_stem("https://e.com/a/cat.photo.png?x=1").as_deref(), Some("cat.photo"));
        assert_eq!(url_stem("https://e.com/"), None);
        assert_eq!(url_stem("https://e.com"), None);
        assert_eq!(url_stem("data:image/png;base64,AAAA"), None);
    }

    #[test]
    fn the_operation_follows_the_source() {
        assert_eq!(operation(1 | 2 | 16), 1);
        assert_eq!(operation(4), 4);
        assert_eq!(operation(0), 0);
        assert_eq!(operation(2), 0);
    }

    #[test]
    fn moves_coalesce_and_steps_track_the_drag() {
        let key = 0xd0d0;
        push(key, Step::Enter(vec!["a".into()], (1.0, 1.0)));
        push(key, Step::Move((2.0, 2.0)));
        push(key, Step::Move((3.0, 3.0)));
        assert!(over(key));
        push(key, Step::Drop(vec!["a".into()], (3.0, 3.0)));
        assert!(!over(key));
        let steps = DESTS.with(|d| std::mem::take(&mut d.borrow_mut().get_mut(&key).unwrap().steps));
        assert_eq!(steps, [Step::Enter(vec!["a".into()], (1.0, 1.0)), Step::Move((3.0, 3.0)), Step::Drop(vec!["a".into()], (3.0, 3.0))]);
    }
}
