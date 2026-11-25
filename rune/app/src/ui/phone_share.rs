//! Desktop device pairing and native QR artwork.

use super::{feather, modal::Modal, theme::rgb};
use crate::{app::App, servers};
use rune::{El, Pixels, el, row};

pub fn button(app: &App, height: f32) -> El {
    let p = app.pal;
    row().key("workspace-phone-share").label("Pair a phone")
        .help("Pair a phone").w(34.0).h(height).center().radius(6.0).hover(rgb(p.hover))
        .child(feather::view(app, feather::Icon::QrCode, 15.0, p.accent))
        .on_press(open)
}

fn open(app: &mut App) {
    open_with(app, crate::daemon::quick_invite_target);
}

fn open_with(app: &mut App, local: impl FnOnce() -> Result<(url::Url, String), String>) {
    if app.mobile { return; }
    if app.server.connecting().is_some() {
        app.modal = Some(Modal::notice("Open on phone", "Wait for the current server connection to finish, then try again."));
        return;
    }
    if let Some(server) = &app.server.link.current {
        // Only an owned, running Fast Server can issue a new display of its invite.
        app.modal = Some(if server.temporary && server.hosted {
            match servers::invite(&server.id) {
                Ok(invite) => {
                    let modal = Modal::fast_server_ready(&server.name, &invite);
                    app.server.ready_invite = Some(invite);
                    modal
                }
                Err(error) => Modal::notice("Invite unavailable", &error),
            }
        } else {
            Modal::notice("Open on phone", "This workspace is on another server. Open a local project to host it, or ask the host for their Fast Server invite.")
        });
        return;
    }

    app.modal = Some(match local() {
        Ok((endpoint, token)) => {
            app.device_invite = Some(crate::device_pair::request(endpoint, serde_json::json!({}), Some(token), None, app.waker.clone()));
            Modal::notice("Pair a phone", "Generating a one-time code…")
        }
        Err(error) => Modal::notice("Pairing unavailable", &error),
    });
}

#[derive(serde::Deserialize)]
pub(crate) struct InviteReply { invite: String }

pub(super) const QR_TITLE: &str = "Pair this computer";

pub(crate) fn ready(app: &mut App, invite: String) -> Modal {
    app.device_qr = Some(invite);
    Modal::notice(QR_TITLE, "Connect your phone to Tailscale and scan in Neoism. Anyone with this code can pair until it expires or is used.")
}

pub(crate) fn poll(app: &mut App) -> bool {
    let Some(result) = app.device_invite.as_ref().and_then(|reply| reply.lock().unwrap().take()) else { return false };
    app.device_invite = None;
    if !app.modal.as_ref().is_some_and(|m| m.title == "Pair a phone") { return false; }
    app.modal = Some(match result {
        Ok(reply) if crate::device_pair::parse(&reply.invite).is_ok_and(|i| i.scan_only) => ready(app, reply.invite),
        Ok(_) => Modal::notice("Pairing unavailable", "The local daemon returned an invalid device invite."),
        Err(error) => Modal::notice("Pairing unavailable", &error),
    });
    true
}

const QUIET: usize = 4;
const PIXELS_PER_MODULE: usize = 8;

fn qr_bitmap(invite: &str) -> Option<(u32, Vec<u8>)> {
    let code = qrcode::QrCode::new(invite.as_bytes()).ok()?;
    let modules = code.width();
    let side = (modules + 2 * QUIET) * PIXELS_PER_MODULE;
    // Opaque black-and-white QR, including the standard four-module quiet zone. Render whole
    // modules into one texture so Rune needn't lay out or paint hundreds of individual cells.
    let mut rgba = vec![255; side * side * 4];
    for (index, color) in code.into_colors().into_iter().enumerate() {
        if color != qrcode::Color::Dark { continue; }
        let x = (index % modules + QUIET) * PIXELS_PER_MODULE;
        let y = (index / modules + QUIET) * PIXELS_PER_MODULE;
        for dy in 0..PIXELS_PER_MODULE {
            let start = ((y + dy) * side + x) * 4;
            for pixel in rgba[start..start + PIXELS_PER_MODULE * 4].chunks_exact_mut(4) {
                pixel[..3].fill(0);
            }
        }
    }
    Some((side as u32, rgba))
}

pub fn qr_view(app: &App, invite: &str) -> El {
    let s = app.ty.scale();
    // Leave room for the instructions and controls. A QR-pinned CA can need over 100 modules.
    let device = crate::device_pair::is_device_link(invite);
    let available = (if device { 448.0 } else { 224.0 } * s).min(app.window[0] - 16.0 - 40.0 * s)
        .min(app.window[1] * if device { 0.58 } else { 0.40 }).max(0.0);
    let code = rune::memo("workspace-invite-qr", (invite, available.to_bits()), || {
        if let Some((side, rgba)) = qr_bitmap(invite) {
            let module = side as f32 / PIXELS_PER_MODULE as f32;
            let size = (available / module).floor() * module;
            if size > 0.0 {
                let image = Pixels::new();
                image.set(side, side, rgba);
                return el().w(size).h(size).pixels(&image);
            }
        }
        rune::text("Cannot display this code. Enlarge the window or generate a new invite.").wrap().color(rgb(app.pal.dim))
    });
    el().w_full().center().mb(10.0 * s).child(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn quick_reply_displays_scan_only_qr_without_password_form() {
        use axum::{Json, Router, http::HeaderMap, routing::post};
        const INVITE: &str = "neoism://device?origin=https://desk.invalid&v=2&code=fixture";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mobile-device/quick-invite", listener.local_addr().unwrap()).parse().unwrap();
        let service = Router::new().route("/mobile-device/quick-invite", post(|headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
            assert_eq!(headers["authorization"], "Bearer local-test");
            assert_eq!(body, serde_json::json!({}));
            Json(serde_json::json!({"invite": INVITE}))
        }));
        let server = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
        let mut app = App::new();
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        app.waker = Some(std::sync::Arc::new(move || { let _ = send.send(()); }));
        open_with(&mut app, || Ok((url, "local-test".into())));
        tokio::time::timeout(std::time::Duration::from_secs(5), receive.recv()).await.unwrap().unwrap();
        assert!(poll(&mut app));
        assert!(app.server.connecting().is_none());
        assert!(app.server.ready_invite.is_none());
        assert!(app.form.is_none());
        assert_eq!(app.device_qr.as_deref(), Some(INVITE));
        server.abort();
    }

    #[test]
    fn workspace_qr_remote_guest_cannot_reuse_a_previous_invite() {
        let mut app = App::new();
        app.server.ready_invite = Some("https://previous.example.invalid/".into());
        app.server.link.current = Some(servers::Selection { id: "guest".into(), temporary: true, ..Default::default() });
        open(&mut app);
        assert!(app.form.is_none());
        let modal = app.modal.unwrap();
        assert!(!modal.body.contains("previous.example"));
        assert!(modal.buttons.iter().all(|button| button.action == super::super::modal::Action::Close));
    }

    #[test]
    fn workspace_qr_bitmap_keeps_a_full_opaque_quiet_zone() {
        let (side, pixels) = qr_bitmap("https://phone-share.example.invalid/?name=Team+project").unwrap();
        let side = side as usize;
        let border = QUIET * PIXELS_PER_MODULE;
        assert_eq!(pixels.len(), side * side * 4);
        for (i, pixel) in pixels.chunks_exact(4).enumerate() {
            let (x, y) = (i % side, i / side);
            if x < border || y < border || x >= side - border || y >= side - border {
                assert_eq!(pixel, [255; 4]);
            }
        }
        let top_left_finder = (border * side + border) * 4;
        assert_eq!(&pixels[top_left_finder..top_left_finder + 4], &[0, 0, 0, 255]);
    }
}