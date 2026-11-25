//! Platform QR acquisition; pairing validation and UI state belong to `mobile`.

use crate::{app::App, ui::modal::Modal};
use rune::Tx;

pub(crate) fn scan(app: &mut App, tx: &mut Tx) {
    #[cfg(target_os = "ios")]
    {
        let _ = tx;
        if let Some(reply) = native::begin(app) {
            // Only the UI thread presents UIKit. The callback never borrows app state.
            unsafe { native::neoism_qr_camera(reply, native::complete) };
        }
    }
    #[cfg(target_os = "macos")]
    {
        if app.mobile_qr.is_some() { return; }
        let options = rune::FileDialogOptions {
            title: "Choose an image containing the pairing QR code".into(),
            ..Default::default()
        }.filter("Images", &["png", "jpg", "jpeg", "heic", "heif", "tiff", "tif", "gif", "bmp", "webp"]);
        tx.open_file(options, |app: &mut App, result, _| match result {
            Ok(Some(path)) => match std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) {
                Ok(path) => {
                    if let Some(reply) = native::begin(app) {
                        // Native code copies the filesystem path before returning.
                        unsafe { native::neoism_qr_image(path.as_ptr(), reply, native::complete) };
                    }
                }
                Err(_) => notice(app, "Could not open the selected image."),
            },
            Ok(None) => {},
            Err(_) => notice(app, "Could not open the image picker. Copy and paste the pairing link instead."),
        });
    }
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        let _ = tx;
        notice(app, "QR scanning is not available on this platform. Copy and paste the pairing link instead.");
    }
}

fn notice(app: &mut App, message: &str) {
    app.modal = Some(Modal::notice("Could not scan QR code", message));
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub(crate) use native::{Pending, poll};

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod native {
    use super::{App, notice};
    use std::{ffi::{c_int, c_void}, sync::mpsc};

    type Answer = Result<Option<String>, String>;
    type Callback = unsafe extern "C" fn(*mut c_void, c_int, *const u8, usize);

    unsafe extern "C" {
        #[cfg(target_os = "ios")]
        pub(super) fn neoism_qr_camera(reply: *mut c_void, callback: Callback);
        #[cfg(target_os = "macos")]
        pub(super) fn neoism_qr_image(path: *const std::ffi::c_char, reply: *mut c_void, callback: Callback);
    }

    pub(crate) struct Pending(mpsc::Receiver<Answer>);

    struct Reply {
        sender: mpsc::Sender<Answer>,
        wake: Option<rune::Waker>,
    }

    pub(super) fn begin(app: &mut App) -> Option<*mut c_void> {
        if app.mobile_qr.is_some() { return None; }
        let (sender, receiver) = mpsc::channel();
        app.mobile_qr = Some(Pending(receiver));
        Some(Box::into_raw(Box::new(Reply { sender, wake: app.waker.clone() })).cast())
    }

    // Each native scan calls back exactly once. The UTF-8 bytes are valid only for this call.
    pub(super) unsafe extern "C" fn complete(reply: *mut c_void, status: c_int, bytes: *const u8, len: usize) {
        let reply = unsafe { Box::from_raw(reply.cast::<Reply>()) };
        let message = if bytes.is_null() { None } else {
            std::str::from_utf8(unsafe { std::slice::from_raw_parts(bytes, len) }).ok()
        };
        let answer = match (status, message) {
            (0, _) => Ok(None),
            (1, Some(text)) if !text.is_empty() => Ok(Some(text.to_owned())),
            (2, Some(error)) => Err(error.to_owned()),
            _ => Err("The QR code did not contain readable text.".into()),
        };
        if reply.sender.send(answer).is_ok() {
            if let Some(wake) = &reply.wake { wake(); }
        }
    }

    pub(crate) fn poll(app: &mut App) -> bool {
        let Some(pending) = &app.mobile_qr else { return false };
        let answer = match pending.0.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("The scanner closed without returning a result.".into()),
        };
        app.mobile_qr = None;
        match answer {
            Ok(None) => false,
            Ok(Some(text)) => { crate::mobile::accept_scanned_invite(app, &text); true }
            Err(error) => { notice(app, &error); true }
        }
    }

    #[cfg(all(test, target_os = "macos"))]
    mod tests {
        use super::*;

        #[test]
        fn vision_decodes_qr_images_but_rejects_blank_images() {
            let invite = "neoism://join?invite=example";
            let qr = qrcode::QrCode::new(invite).unwrap();
            let modules = qr.width();
            let colors = qr.to_colors();
            let size = (modules + 8) * 8; // Four-module quiet zone and 8 pixels per module.
            let mut pixels = vec![255; size * size];
            for y in 0..modules * 8 {
                for x in 0..modules * 8 {
                    if colors[y / 8 * modules + x / 8] == qrcode::Color::Dark {
                        pixels[(y + 32) * size + x + 32] = 0;
                    }
                }
            }

            let decode = |pixels: &[u8]| {
                let file = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
                let mut encoder = png::Encoder::new(file.as_file(), size as u32, size as u32);
                encoder.set_color(png::ColorType::Grayscale);
                let mut writer = encoder.write_header().unwrap();
                writer.write_image_data(pixels).unwrap();
                writer.finish().unwrap();

                let path = std::ffi::CString::new(file.path().as_os_str().as_encoded_bytes()).unwrap();
                let (sender, receiver) = mpsc::channel();
                let reply = Box::into_raw(Box::new(Reply { sender, wake: None })).cast();
                unsafe { neoism_qr_image(path.as_ptr(), reply, complete) };
                receiver.recv_timeout(std::time::Duration::from_secs(10)).unwrap()
            };

            assert_eq!(decode(&pixels).unwrap().as_deref(), Some(invite));
            pixels.fill(255);
            assert_eq!(decode(&pixels).unwrap_err(), "No readable QR code was found in the selected image.");
        }
    }
}