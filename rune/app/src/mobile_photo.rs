//! Native photo attachment for the mobile layout (including the desktop preview).

use crate::{app::App, tabs::TabKind, ui::modal::Modal};
use rune::Tx;

struct Target {
    workspace: String,
    tab: u64,
}

impl Target {
    fn capture(app: &App) -> Option<Self> {
        let tab = app.tabs.get(app.active)?;
        matches!(tab.kind, TabKind::Agent { .. }).then(|| Self {
            workspace: app.workspace_id.clone(),
            tab: tab.id,
        })
    }

    fn apply(self, app: &mut App, attach: impl FnOnce(&mut App)) -> bool {
        let mut applied = false;
        app.with_workspace(&self.workspace, |app| {
            let Some(index) = app.tabs.iter().position(|tab| {
                tab.id == self.tab && matches!(tab.kind, TabKind::Agent { .. })
            }) else { return };
            // The existing attachment APIs operate on active. No selection/focus hooks run.
            let active = std::mem::replace(&mut app.active, index);
            attach(app);
            app.active = active;
            applied = true;
        });
        applied
    }
}

/// Choose one photo for the currently selected agent tab. A sidebar action that opens or
/// selects a chat should do so before calling this function. Cancellation changes nothing.
pub(crate) fn choose(app: &mut App, tx: &mut Tx) {
    let Some(target) = Target::capture(app) else { return };

    #[cfg(not(target_os = "ios"))]
    {
        let options = rune::FileDialogOptions { title: "Choose a photo".into(), ..Default::default() }
            .filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "bmp"]);
        tx.open_file(options, move |app: &mut App, result, _| finish_file(app, target, result));
    }

    #[cfg(target_os = "ios")]
    {
        let _ = tx;
        open_photos(app, ios::Destination::Attachment(target));
    }
}

#[cfg(target_os = "ios")]
pub(crate) fn choose_background(app: &mut App) {
    open_photos(app, ios::Destination::Background);
}

#[cfg(target_os = "ios")]
fn open_photos(app: &mut App, destination: ios::Destination) {
    if app.mobile_photo.is_some() { return; }
    match ios::present(destination, app.waker.clone()) {
        Ok(pending) => app.mobile_photo = Some(pending),
        Err((ios::Destination::Background, error)) => app.mobile_backgrounds.error = Some(error.into()),
        Err((ios::Destination::Attachment(_), error)) => notice(app, error),
    }
}

#[cfg(not(target_os = "ios"))]
fn finish_file(app: &mut App, target: Target, result: rune::FileDialogResult) {
    match result {
        Ok(None) => {},
        Ok(Some(path)) => {
            target.apply(app, |app| {
                use crate::agent::attach::mime_for;
                if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| mime_for(name).starts_with("image/")) {
                    app.attach_images(&[path]);
                } else {
                    notice(app, "Choose a PNG, JPEG, WebP, GIF, or BMP image.");
                }
            });
        }
        Err(error) => { target.apply(app, |app| notice(app, &error.to_string())); }
    }
}

fn notice(app: &mut App, error: &str) {
    app.modal = Some(Modal::notice("Could not attach photo", error));
}

#[cfg(target_os = "ios")]
pub(crate) use ios::{Pending, poll};

#[cfg(target_os = "ios")]
mod ios {
    use super::{App, Target, notice};
    use crate::agent::attach::{MAX_BYTES, mime_for};
    use block2::RcBlock;
    use objc2::rc::{Allocated, Retained, autoreleasepool};
    use objc2::runtime::{AnyClass, NSObject};
    use objc2::{ClassType, DeclaredClass, ProtocolType, declare_class, extern_protocol, msg_send, msg_send_id, mutability};
    use objc2_foundation::{NSArray, NSData, NSError, MainThreadMarker, NSObjectProtocol, ns_string};
    use objc2_photos_ui::{PHPickerConfiguration, PHPickerConfigurationAssetRepresentationMode,
        PHPickerFilter, PHPickerResult};
    use objc2_ui_kit::{UIApplication, UIImage, UIImageJPEGRepresentation, UISceneActivationState,
        UIViewController, UIWindowScene};
    use std::cell::RefCell;
    use std::sync::mpsc;

    struct Photo {
        name: String,
        bytes: Vec<u8>,
    }

    type Answer = Result<Option<Photo>, &'static str>;

    pub(super) enum Destination { Attachment(Target), Background }

    struct Reply {
        sender: mpsc::Sender<Answer>,
        wake: Option<rune::Waker>,
    }

    impl Reply {
        fn send(&self, answer: Answer) {
            if self.sender.send(answer).is_ok() {
                if let Some(wake) = &self.wake { wake(); }
            }
        }
    }

    pub(crate) struct Pending {
        destination: Destination,
        receiver: mpsc::Receiver<Answer>,
        // PHPicker's delegate property is weak. Keep the delegate alive until the app receives
        // the answer (or drops this window). The delegate never stores App or Tx.
        _delegate: Retained<PickerDelegate>,
    }

    // The 0.2 bindings only declare the delegate method on macOS. On iOS the first
    // argument is a PHPickerViewController, which is a subclass of UIViewController.
    extern_protocol!(
        unsafe trait PHPickerViewControllerDelegate: NSObjectProtocol {
            #[method(picker:didFinishPicking:)]
            unsafe fn did_finish(&self, picker: &UIViewController, results: &NSArray<PHPickerResult>);
        }

        unsafe impl ProtocolType for dyn PHPickerViewControllerDelegate {}
    );

    declare_class!(
        struct PickerDelegate;

        unsafe impl ClassType for PickerDelegate {
            type Super = NSObject;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "NeoismPhotoPickerDelegate";
        }

        impl DeclaredClass for PickerDelegate {
            type Ivars = RefCell<Option<Reply>>;
        }

        unsafe impl NSObjectProtocol for PickerDelegate {}

        unsafe impl PHPickerViewControllerDelegate for PickerDelegate {
            #[method(picker:didFinishPicking:)]
            unsafe fn did_finish(&self, picker: &UIViewController, results: &NSArray<PHPickerResult>) {
                let Some(reply) = self.ivars().borrow_mut().take() else { return };
                // PHPicker does not automatically dismiss, including on Cancel.
                unsafe { picker.dismissViewControllerAnimated_completion(true, None); }
                let Some(result) = (unsafe { results.firstObject() }) else {
                    reply.send(Ok(None));
                    return;
                };
                let provider = unsafe { result.itemProvider() };
                let image_type = ns_string!("public.image");
                if !unsafe { provider.hasItemConformingToTypeIdentifier(image_type) } {
                    reply.send(Err("Photos did not provide an image for this selection."));
                    return;
                }
                let name = unsafe { provider.suggestedName() }.map(|name| name.to_string());
                // The provider copies the block and may call it on a background queue. It
                // captures only Rust-owned, thread-safe data; UIKit presentation stays above.
                let completion = RcBlock::new(move |data: *mut NSData, _: *mut NSError| {
                    let answer = autoreleasepool(|_| {
                        let data = unsafe { data.as_ref() }.ok_or("Could not load this photo from Photos.")?;
                        photo(data, name.as_deref()).map(Some)
                    });
                    reply.send(answer);
                });
                unsafe { provider.loadDataRepresentationForTypeIdentifier_completionHandler(image_type, &completion); }
            }
        }
    );

    fn photo(data: &NSData, suggested_name: Option<&str>) -> Result<Photo, &'static str> {
        const TOO_LARGE: &str = "Choose a photo smaller than 20 MiB.";
        if data.len() > MAX_BYTES { return Err(TOO_LARGE); }
        if data.is_empty() { return Err("Photos returned an empty image."); }
        // Preserve supported formats (including alpha and animation). Convert other formats
        // that UIKit understands, such as HEIC, to an attachment-compatible JPEG.
        let format = match image::guess_format(data.bytes()) {
            Ok(image::ImageFormat::Png) => Some("png"),
            Ok(image::ImageFormat::Jpeg) => Some("jpg"),
            Ok(image::ImageFormat::Gif) => Some("gif"),
            Ok(image::ImageFormat::WebP) => Some("webp"),
            Ok(image::ImageFormat::Bmp) => Some("bmp"),
            _ => None,
        };
        let (ext, bytes) = if let Some(ext) = format {
            (ext, data.bytes().to_vec())
        } else {
            let image = unsafe { UIImage::imageWithData(data) }.ok_or("Could not decode this photo.")?;
            let jpeg = unsafe { UIImageJPEGRepresentation(&image, 0.9).as_ref() }
                .ok_or("Could not convert this photo to JPEG.")?;
            if jpeg.len() > MAX_BYTES { return Err(TOO_LARGE); }
            ("jpg", jpeg.bytes().to_vec())
        };
        let name = suggested_name.and_then(|name| std::path::Path::new(name).file_stem())
            .and_then(|name| name.to_str()).filter(|name| !name.is_empty()).unwrap_or("Photo");
        Ok(Photo { name: format!("{name}.{ext}"), bytes })
    }

    pub(super) fn present(destination: Destination, wake: Option<rune::Waker>) -> Result<Pending, (Destination, &'static str)> {
        match present_picker(wake) {
            Ok((receiver, delegate)) => Ok(Pending { destination, receiver, _delegate: delegate }),
            Err(error) => Err((destination, error)),
        }
    }

    fn present_picker(wake: Option<rune::Waker>) -> Result<(mpsc::Receiver<Answer>, Retained<PickerDelegate>), &'static str> {
        let mtm = MainThreadMarker::new().ok_or("Photos must be opened on the UI thread.")?;
        let picker_class = AnyClass::get("PHPickerViewController").ok_or("Photos requires iOS 14 or later.")?;
        let root = presenter(mtm).ok_or("There is no active window for Photos.")?;
        let (sender, receiver) = mpsc::channel();
        // UIKit calls and selector signatures are for PHPicker and UIViewController; the
        // iOS subclass itself is missing from these generated bindings.
        unsafe {
            if root.presentedViewController().is_some() {
                return Err("Close the current system dialog before opening Photos.");
            }
            let configuration = PHPickerConfiguration::new();
            configuration.setFilter(Some(&PHPickerFilter::imagesFilter()));
            configuration.setSelectionLimit(1);
            configuration.setPreferredAssetRepresentationMode(PHPickerConfigurationAssetRepresentationMode::Compatible);
            let picker: Allocated<UIViewController> = msg_send_id![picker_class, alloc];
            let picker: Retained<UIViewController> = msg_send_id![picker, initWithConfiguration: &*configuration];
            let delegate = mtm.alloc::<PickerDelegate>().set_ivars(RefCell::new(Some(Reply { sender, wake })));
            let delegate: Retained<PickerDelegate> = msg_send_id![super(delegate), init];
            let _: () = msg_send![&*picker, setDelegate: &*delegate];
            // Use the picker's own Cancel button so every dismissal reports a result.
            picker.setModalInPresentation(true);
            root.presentViewController_animated_completion(&picker, true, None);
            Ok((receiver, delegate))
        }
    }

    fn presenter(mtm: MainThreadMarker) -> Option<Retained<UIViewController>> {
        let application = UIApplication::sharedApplication(mtm);
        for scene in unsafe { application.connectedScenes() }.iter() {
            if !scene.is_kind_of::<UIWindowScene>() || unsafe { scene.activationState() } != UISceneActivationState::ForegroundActive {
                continue;
            }
            // The class check above establishes that this is a window scene.
            let scene = unsafe { Retained::cast::<UIWindowScene>(scene.retain()) };
            for window in unsafe { scene.windows() }.iter() {
                if window.isKeyWindow() { return window.rootViewController(); }
            }
        }
        None
    }

    pub(crate) fn poll(app: &mut App) -> bool {
        let Some(pending) = &app.mobile_photo else { return false };
        let answer = match pending.receiver.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("Photos closed without returning an image."),
        };
        let Some(pending) = app.mobile_photo.take() else { return false };
        match (pending.destination, answer) {
            (_, Ok(None)) => false,
            (Destination::Attachment(target), Ok(Some(photo))) => target.apply(app, |app| {
                app.attach_bytes(&photo.name, mime_for(&photo.name), photo.bytes);
            }),
            (Destination::Attachment(target), Err(error)) => target.apply(app, |app| notice(app, error)),
            (Destination::Background, Ok(Some(photo))) => {
                app.import_background_photo(photo.name, photo.bytes);
                true
            }
            (Destination::Background, Err(error)) => {
                app.mobile_backgrounds.error = Some(error.into());
                true
            }
        }
    }
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn chat(app: &mut App) -> Target {
        app.open_tab(TabKind::Agent { session: None });
        Target::capture(app).unwrap()
    }

    #[test]
    fn file_dialog_attaches_to_the_originating_tab_in_a_parked_workspace() {
        let mut app = App::new();
        let target = chat(&mut app);
        let origin = target.tab;
        let next_tab = chat(&mut app).tab;
        app.new_workspace();
        let visible = chat(&mut app).tab;

        let mut image = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        image.write_all(b"photo bytes").unwrap();
        finish_file(&mut app, target, Ok(Some(image.path().to_owned())));

        assert_eq!(app.workspace_active, 1);
        assert_eq!(app.active_tab().id, visible);
        assert!(app.active_tab().draft.text.is_empty());
        assert_eq!(app.attach.pending[&origin][0].mime, "image/png");
        app.select_workspace(0);
        assert_eq!(app.active_tab().id, next_tab);
        assert!(app.active_tab().draft.text.is_empty());
        assert!(app.tabs.iter().find(|tab| tab.id == origin).unwrap().draft.text.contains("[image1]"));
    }

    #[test]
    fn cancellation_and_closed_tabs_do_not_affect_another_draft() {
        let mut app = App::new();
        let target = chat(&mut app);
        finish_file(&mut app, target, Ok(None));
        assert!(app.modal.is_none());
        assert!(app.active_tab().draft.text.is_empty());

        let target = Target::capture(&app).unwrap();
        app.close_tab_now(app.active);
        chat(&mut app);
        finish_file(&mut app, target, Ok(Some("missing.jpg".into())));
        assert!(app.modal.is_none());
        assert!(app.attach.pending.is_empty());
        assert!(app.active_tab().draft.text.is_empty());
    }
}