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

    app.form = Some(servers::form::Form::pair_phone());
}

#[derive(serde::Deserialize)]
pub(crate) struct InviteReply { invite: String }

pub(crate) fn generate(app: &mut App, password: String) {
    let Ok(operator_key) = std::env::var("NEOISM_MOBILE_OPERATOR_KEY") else {
        app.modal = Some(Modal::notice("Pairing unavailable", "This Mac's Tailscale device pairing service is not configured."));
        return;
    };
    let endpoint = crate::daemon::public_endpoint().and_then(|s| url::Url::parse(&s).ok());
    let Some(mut endpoint) = endpoint.filter(|u| match u.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }) else {
        app.modal = Some(Modal::notice("Pair a phone", "Connect desktop Neoism to its local daemon before pairing."));
        return;
    };
    let scheme = if endpoint.scheme() == "wss" { "https" } else { "http" };
    if endpoint.set_scheme(scheme).is_err() { return; }
    endpoint.set_path("/mobile-device/invite");
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    app.device_invite = Some(crate::device_pair::request(endpoint, serde_json::json!({"password": password}), Some(operator_key), None, app.waker.clone()));
    app.modal = Some(Modal::notice("Pair a phone", "Generating a one-time code…"));
}

pub(crate) fn poll(app: &mut App) -> bool {
    let Some(result) = app.device_invite.as_ref().and_then(|reply| reply.lock().unwrap().take()) else { return false };
    app.device_invite = None;
    if !app.modal.as_ref().is_some_and(|m| m.title == "Pair a phone") { return false; }
    app.modal = Some(match result {
        Ok(reply) if crate::device_pair::parse(&reply.invite).is_ok() => {
            app.device_qr = Some(reply.invite);
            Modal::notice("Pair this Mac", "Connect your iPhone to Tailscale, then scan this code in Neoism and enter the pairing password. The code expires shortly and can only be used once.")
        }
        Ok(_) => Modal::notice("Pairing unavailable", "The Mac returned an invalid device invite."),
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
    let available = (224.0 * s).min(app.window[0] - 60.0).min(app.window[1] * 0.40).max(0.0);
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
        el()
    });
    // The invite link remains available even if an unusually long payload cannot be encoded.
    el().w_full().center().mb(10.0 * s).child(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_qr_opens_device_pairing_without_starting_a_fast_server() {
        let mut app = App::new();
        app.cwd = "/projects/current workspace".into();
        open(&mut app);
        assert!(app.server.connecting().is_none());
        assert!(app.server.ready_invite.is_none());
        let form = app.form.as_ref().unwrap();
        assert_eq!(form.kind, servers::form::Kind::PairPhone);
        assert_eq!(form.value("token"), Some(""));
        assert_eq!(form.current_focus(), servers::form::Focus::Field(0));
        assert_eq!(form.submit, "Generate QR Code");
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