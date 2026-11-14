//! `--desktop BACKDROP.png`: a headless run of every window an app makes, each drawn at its
//! placed frame with rounded corners and a soft shadow over a backdrop, as a screen recording of
//! the app would show them. Each window renders into its own texture with its own state; one
//! pass composites them in z-order.


use std::sync::Arc;

use serde_json::json;

use crate::AnyWindowSpec;
use crate::desktop::displays::VirtualDesktop;
use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::time::Instant;
use crate::tooling::headless_run::WindowFrame;
use crate::tooling::{Args, Ev, Kind, Pos, Shot, Timeline, Trace, die};

mod composite;
pub(crate) mod displays;
#[cfg(test)]
mod tests;
mod window;

use composite::{Composite, DOWN, KEY_SHADOW, SHADOW, Slot, Uniform, pixels};
pub(crate) use window::{Changes, Member, Window};

/// Bound on rounds of windows asking things of each other within one frame.
const PASSES: usize = 8;

/// The windows, back to front, and which has the keyboard.
struct Desk {
    members: Vec<Box<dyn Member>>,
    /// Indices into `members`, bottom first.
    z: Vec<usize>,
    key: Option<usize>,
    /// The app is frontmost: its key window has focus.
    active: bool,
    /// The window under the pointer, and the one a press captured.
    hover: Option<usize>,
    pressed: Option<usize>,
    /// The window a script's file drag is over.
    files: Option<usize>,
    /// Output origin in global points, and points to pixels.
    origin: (f32, f32),
    scale: f32,
}

impl Desk {
    fn focus(&mut self, key: Option<usize>, active: bool) {
        let before = self.key.filter(|_| self.active);
        let after = key.filter(|_| active);
        if before != after {
            if let Some(i) = before { self.members[i].focus(false); }
            if let Some(i) = after { self.members[i].focus(true); }
        }
        self.key = key;
        self.active = active;
    }

    /// Brings `i` to the front of its level; always-on-top windows stay above the rest.
    fn front(&mut self, i: usize) {
        self.z.retain(|&j| j != i);
        self.z.push(i);
        let members = &self.members;
        self.z.sort_by_key(|&j| members[j].on_top());
    }

    /// The window that gets the keyboard when no window asks for it: the topmost visible one
    /// that takes clicks.
    fn topmost_visible(&self) -> Option<usize> {
        self.z.iter().rev().copied().find(|&i| self.can_key(i))
    }

    /// Whether window `i` can have the keyboard: it shows and takes clicks.
    fn can_key(&self, i: usize) -> bool {
        self.members[i].visible() && !self.members[i].click_through()
    }

    fn apply(&mut self, i: usize, changes: Changes) {
        if changes.front && self.members[i].visible() { self.front(i); }
        if changes.keys && self.can_key(i) { self.focus(Some(i), true); }
    }

    /// A hidden key window hands the keyboard to the topmost visible one.
    fn settle_key(&mut self) {
        if self.key.is_some_and(|k| self.can_key(k)) { return; }
        let next = self.topmost_visible();
        if next != self.key { self.focus(next, self.active); }
        for i in [self.hover, self.pressed].into_iter().flatten() {
            if !self.members[i].visible() {
                if self.hover == Some(i) { self.hover = None; }
                if self.pressed == Some(i) { self.pressed = None; }
            }
        }
    }

    /// Applies what every window asked, again while draining one window makes requests of
    /// another (a focus change runs `on_focus`, which may close its window or update others).
    fn drain(&mut self, desktop: &VirtualDesktop, t: f64) {
        for _ in 0..PASSES {
            for i in 0..self.members.len() {
                let changes = self.members[i].drain(desktop, t);
                self.apply(i, changes);
            }
            self.settle_key();
            if !self.members.iter().any(|m| m.pending()) { return; }
        }
        eprintln!("rune: windows still made requests of each other after {PASSES} passes at t = {t:.3}; the rest wait a frame");
    }

    /// Steps, without time passing, each shown window whose frame no longer shows what this
    /// frame's handlers asked of it, so the frame draws their result rather than a frame later.
    fn catch_up(&mut self, desktop: &VirtualDesktop, t: f64) {
        for _ in 0..PASSES {
            let mut stepped = false;
            for m in &mut self.members {
                if m.visible() && m.stale() { m.advance(0.0); stepped = true; }
            }
            if !stepped { return; }
            self.drain(desktop, t);
        }
    }

    /// Top-left in output pixels, rounded so window pixels land on output pixels.
    fn origin_px(&self, i: usize) -> (f32, f32) {
        let f = self.members[i].frame();
        (((f[0] - self.origin.0) * self.scale).round(), ((f[1] - self.origin.1) * self.scale).round())
    }

    fn origin_pt(&self, i: usize) -> (f32, f32) {
        let (x, y) = self.origin_px(i);
        (x / self.scale, y / self.scale)
    }

    fn hit(&self, (x, y): (f32, f32)) -> Option<usize> {
        self.z.iter().rev().copied().find(|&i| {
            let m = &self.members[i];
            let (ox, oy) = self.origin_pt(i);
            let f = m.frame();
            m.visible() && !m.click_through() && x >= ox && y >= oy && x < ox + f[2] && y < oy + f[3]
        })
    }

    /// The element's centre in output points: the key window first, then front to back.
    fn find(&self, key: &str) -> Option<(f32, f32)> {
        let order = self.key.into_iter().chain(self.z.iter().rev().copied().filter(|&i| Some(i) != self.key));
        order.filter(|&i| self.members[i].visible()).find_map(|i| {
            let (x, y) = self.members[i].find(key)?;
            let (ox, oy) = self.origin_pt(i);
            Some((x + ox, y + oy))
        })
    }

    fn pointer(&self) -> Option<(f32, f32)> {
        let i = self.hover?;
        let (x, y) = self.members[i].pointer()?;
        let (ox, oy) = self.origin_pt(i);
        Some((x + ox, y + oy))
    }

    /// Pointer input at `at` in output points: to the window a press captured, else the one under it.
    fn pointer_input(&mut self, kind: &Kind, at: (f32, f32)) {
        let captured = (matches!(kind, Kind::Move) || kind.is_up()).then_some(self.pressed).flatten();
        let target = captured.or_else(|| self.hit(at));
        if (matches!(kind, Kind::Move) || kind.is_down()) && self.hover != target && captured.is_none() {
            if let Some(h) = self.hover { self.members[h].leave(); }
            self.hover = target;
        }
        if kind.is_down() {
            self.pressed = target;
            match target {
                // A click brings its window to the front with the keyboard, activating the app.
                Some(i) => { self.front(i); self.focus(Some(i), true); }
                // A click on the backdrop activates another app.
                None => self.focus(self.key, false),
            }
        }
        if kind.is_up() { self.pressed = None; }
        if let Some(i) = target {
            let (ox, oy) = self.origin_pt(i);
            self.members[i].input(kind, (at.0 - ox, at.1 - oy));
        }
    }

    /// A script's file drag goes to the window under it, whether or not that window is key; a
    /// drag that moves off a window leaves it.
    fn file_input(&mut self, step: &crate::tooling::FileStep, at: (f32, f32)) {
        use crate::tooling::FileStep;
        let target = if matches!(step, FileStep::Leave) { self.files } else { self.hit(at) };
        if let Some(old) = self.files.filter(|&old| Some(old) != target) {
            self.members[old].input(&Kind::Files(FileStep::Leave), (0.0, 0.0));
        }
        self.files = target.filter(|_| matches!(step, FileStep::Hover(_)));
        if let Some(i) = target {
            let (ox, oy) = self.origin_pt(i);
            self.members[i].input(&Kind::Files(step.clone()), (at.0 - ox, at.1 - oy));
        }
    }

    /// The app reopened: every window's `on_reopen` runs, then, unless one handled it or the app
    /// is an accessory, the default shows a window when none does (`reopen::target`) and
    /// activates the app with it in front.
    fn reopen(&mut self, accessory: bool, desktop: &VirtualDesktop, t: f64) {
        let candidates = |desk: &Self| -> Vec<_> { desk.members.iter().map(|m| m.reopen_candidate()).collect() };
        let has_visible = crate::platform::reopen::has_visible(&candidates(self));
        let mut handled = false;
        for m in &mut self.members { handled |= m.reopen_hook(has_visible); }
        if handled || accessory { return; }
        // The default sees what the hooks did.
        self.drain(desktop, t);
        if let Some(i) = crate::platform::reopen::target(&candidates(self)) {
            let changes = self.members[i].reopen();
            self.apply(i, changes);
        }
    }

    /// Window `i`'s frame in output points, whether it shows, and whether it has the keyboard
    /// (never while another app is active).
    fn state(&self, i: usize) -> ([f32; 4], bool, bool) {
        let m = &self.members[i];
        let f = m.frame();
        ([f[0] - self.origin.0, f[1] - self.origin.1, f[2], f[3]], m.visible(), self.active && self.key == Some(i))
    }

    fn snapshot(&self, i: usize, t: f64) -> WindowFrame {
        let (frame, visible, focused) = self.state(i);
        WindowFrame { t, key: self.members[i].key().to_owned(), frame, visible, focused }
    }
}

/// Renders `windows` composited over the `--desktop` backdrop. `selected` is the window
/// `--scene`, `--tree` and `--trace` describe.
/// `accessory` is `RunOptions::accessory`: reopening brings back no window.
pub(crate) fn run(windows: Vec<AnyWindowSpec>, selected: usize, args: Args, accessory: bool) {
    let launched = Instant::now();
    let backdrop = args.desktop.clone().expect("--desktop");
    if args.list_scenes { die("--list-scenes lists one window's scenes; drop --desktop"); }
    let source = crate::tooling::read_script(&args);
    let script = source.as_ref().map(crate::tooling::load_script).unwrap_or_default();
    let timeline = Timeline::of(&args, &script).unwrap_or_else(|e| die(&e));
    let scale = args.scale.unwrap_or(2.0);
    let desktop = crate::tooling::desktop(&args, source.as_ref());
    let primary = desktop.primary().frame.map(|v| v as f32);
    let size = args.size.unwrap_or((primary[2], primary[3]));
    let frames_log = Arc::new(std::sync::Mutex::new(Vec::new()));
    crate::tooling::headless_run::publish(crate::tooling::headless_run::HeadlessRun {
        script: args.script.clone(),
        events: script.iter().map(Ev::public).collect(),
        scale,
        fps: timeline.fps,
        size,
        start: timeline.start_frame as f64 * timeline.dt,
        frames: timeline.frames,
        end: timeline.last_frame as f64 * timeline.dt,
        desktop: Some(backdrop.clone()),
        windows: frames_log.clone(),
    });

    let mut members: Vec<Box<dyn Member>> = windows.into_iter().enumerate()
        .map(|(i, w)| w.desktop(&args, i == selected, scale)).collect();
    for m in &mut members { m.start(&desktop); }
    let Timeline { fps, dt, start_frame, frames, snap_frame, last_frame } = timeline;
    let mut trace = args.trace.as_ref().map(|p| {
        let f = members[selected].frame();
        members[selected].trace_on();
        let mut trace = Trace::open(p, (f[2], f[3]), fps, scale);
        trace.all = args.trace_all;
        trace
    });
    let n = members.len();
    let mut desk = Desk {
        members, z: (0..n).collect(), key: None, active: true, hover: None, pressed: None, files: None,
        origin: (primary[0], primary[1]), scale,
    };
    let members = &desk.members;
    desk.z.sort_by_key(|&i| members[i].on_top());
    let initial = desk.topmost_visible();
    desk.focus(initial, true);

    let t0 = Instant::now();
    let features = desk.members.iter().fold(wgpu::Features::empty(), |f, m| f | m.features());
    let gpu = Gpu::try_with_features(Gpu::instance_for(features), None, features).unwrap_or_else(|e| die(&e));
    let max = gpu.device.limits().max_texture_dimension_2d;
    let (wp, hp) = pixels(size, scale, max).unwrap_or_else(|| die(&format!(
        "--desktop: the output, {}x{} points at --scale {scale}, must be 1 to {max} pixels a side", size.0, size.1)));
    let comp = Composite::new(&gpu, &backdrop, (wp, hp));
    let mut writer = crate::tooling::record::Writer::new((wp, hp));
    if let Some(path) = &args.video {
        writer.video(path, fps).unwrap_or_else(|e| die(&e));
    }
    let target = Offscreen::new(&gpu.device, wp, hp);
    let mut readback = crate::tooling::record::Readback::new(&gpu.device, &target, crate::tooling::record::DEPTH);
    let mut slots: Vec<Option<Slot>> = (0..n).map(|_| None).collect();
    let mut overlay = (args.keys.is_some() || args.pointer)
        .then(|| (crate::input::keycaps::Overlay::new(size, scale, args.keys, args.keys_theme, args.pointer), Renderer::new(&gpu.device, OFFSCREEN_FORMAT)));
    let dark = args.theme.as_deref() != Some("light");
    let init_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let mut strip: Vec<Vec<u8>> = Vec::new();
    let crops_px: Vec<[u32; 4]> = args.crops.iter().map(|c| c.map(|v| (v * scale).round() as u32)).collect();
    let gap = (4.0 * scale) as u32;
    let mut frames_written = 0usize;
    let has_video = writer.has_video();
    let mut deliver = |shot: Shot, px: Vec<u8>| {
        if shot.dump {
            if args.strip.is_some() { strip.push(crate::tooling::tile(&px, wp, hp, &crops_px, gap)); }
            if let Some(dir) = &args.out {
                writer.png(dir.join(format!("frame_{:04}.png", shot.k - start_frame)), px.clone()).unwrap_or_else(|e| die(&e));
            }
        }
        if shot.snap { writer.png(args.snap.clone().unwrap(), px.clone()).unwrap_or_else(|e| die(&e)); }
        if shot.video {
            frames_written += 1;
            writer.frame(px).unwrap_or_else(|e| die(&e));
        }
    };

    // Logs a window's frame when it moves, shows, hides or takes or loses the keyboard.
    let mut recorded = vec![None; n];
    let mut record = |desk: &Desk, t: f64| {
        let mut log = frames_log.lock().unwrap();
        for (i, last) in recorded.iter_mut().enumerate() {
            let now = desk.state(i);
            if *last != Some(now) {
                log.push(desk.snapshot(i, t));
                *last = Some(now);
            }
        }
    };

    // Events at t = 0 find keys and hit elements in a laid-out first frame (see `headless_configured`).
    if script.iter().find(|e| !matches!(e.kind, Kind::Mark(_))).is_some_and(|e| e.t <= 1e-9) {
        desk.drain(&desktop, 0.0);
        for m in &mut desk.members { m.advance(0.0); }
        desk.drain(&desktop, 0.0);
    }

    let wall = Instant::now();
    let mut cpu_ms = Vec::new();
    let mut next_ev = 0;
    let mut last_down = (0.0f32, 0.0f32);
    let mut drawn: Vec<(usize, bool)> = Vec::with_capacity(n);
    for k in 0..=last_frame {
        let real_t = k as f64 * dt;
        let frame_start = Instant::now();
        while next_ev < script.len() && script[next_ev].t <= real_t + 1e-9 {
            let e = &script[next_ev];
            next_ev += 1;
            let at = || match &e.pos {
                Pos::Xy(x, y) => (*x, *y),
                Pos::Key(key) => desk.find(key).unwrap_or_else(|| die(&format!("script: no element with key {key:?} in a visible window"))),
                Pos::Rel(_, _) if e.kind.at_pointer() => desk.pointer().unwrap_or((0.0, 0.0)),
                Pos::Rel(dx, dy) => (last_down.0 + dx, last_down.1 + dy),
            };
            let (x, y) = at();
            match &e.kind {
                Kind::Mark(_) => {}
                Kind::Focus(focused) => desk.focus(desk.key, *focused),
                Kind::Reopen => desk.reopen(accessory, &desktop, real_t),
                Kind::Shortcut(shortcut) => fire(&desk, |m| m.shortcut(shortcut)).unwrap_or_else(|e| die(&format!("script shortcut: {e}"))),
                Kind::Menu(item) => fire(&desk, |m| m.menu(item)).unwrap_or_else(|e| die(&format!("script menu: {e}"))),
                Kind::PopupMenu(picked) => {
                    let open = desk.key.into_iter().chain(desk.z.iter().rev().copied()).find(|&i| desk.members[i].popup_open());
                    let i = open.or(desk.key).unwrap_or(selected);
                    desk.members[i].resolve_popup(picked.as_deref()).unwrap_or_else(|e| die(&format!("script pop-up menu: {e}")));
                }
                Kind::FileDialog(path) => {
                    let open = desk.key.into_iter().chain(desk.z.iter().rev().copied()).find(|&i| desk.members[i].file_dialog_open());
                    let i = open.or(desk.key).unwrap_or(selected);
                    desk.members[i].resolve_file_dialog(path.clone()).unwrap_or_else(|e| die(&format!("script file dialog: {e}")));
                }
                // While another app is active, as after a click on the backdrop, keys go to it.
                Kind::Key(..) | Kind::KeyUp(_) | Kind::Physical(_) | Kind::PhysicalKey(..) | Kind::Paste(_) => {
                    if let Some(i) = desk.key.filter(|_| desk.active) { desk.members[i].input(&e.kind, (x, y)); }
                }
                Kind::Files(step) => desk.file_input(step, (x, y)),
                Kind::OpenUrl(_) => die("script open_url: a link opens in a single window's run (`--script` without a desktop)"),
                // The rest are pointer input.
                kind => {
                    if kind.is_down() { last_down = (x, y); }
                    desk.pointer_input(&e.kind, (x, y));
                }
            }
            if let Some((ov, _)) = overlay.as_mut() {
                match &e.kind {
                    Kind::Key(key, _) | Kind::PhysicalKey(_, key, _) => ov.key(e.t, key),
                    Kind::Shortcut(chord) => ov.chord(e.t, chord),
                    &Kind::Physical(crate::PhysicalInput::Key { key, mods, pressed: true, .. }) => {
                        if let Ok(chord) = crate::Shortcut::from_physical_key(key, mods) { ov.chord(e.t, &chord) }
                    }
                    Kind::Down => ov.press(e.t, x, y),
                    _ => {}
                }
            }
        }
        desk.drain(&desktop, real_t);
        let step = if k == 0 { 0.0 } else { dt };
        for m in &mut desk.members { m.advance(step); }
        // Handlers that ran this frame may show, hide, place, focus or close windows, or update
        // a window that already stepped.
        desk.drain(&desktop, real_t);
        desk.catch_up(&desktop, real_t);
        record(&desk, real_t);
        let update_ms = frame_start.elapsed().as_secs_f64() * 1000.0;

        let key = desk.key.filter(|_| desk.active);
        drawn.clear();
        for &i in &desk.z {
            let m = &mut desk.members[i];
            if !m.visible() { continue; }
            let f = m.frame();
            let px = (((f[2] * scale).round() as u32).max(1), ((f[3] * scale).round() as u32).max(1));
            if px.0.max(px.1) > max {
                die(&format!("--desktop: window {:?} is {}x{} pixels, over the GPU's {max} a side", m.key(), px.0, px.1));
            }
            if slots[i].as_ref().is_none_or(|s| s.px != px) { slots[i] = Some(Slot::new(&gpu.device, &comp, px, scale)); }
            let slot = slots[i].as_ref().unwrap();
            m.render(&gpu, slot, px, step, update_ms);
            let (ox, oy) = desk.origin_px(i);
            let (sigma, offset, opacity) = if key == Some(i) { KEY_SHADOW } else { SHADOW };
            slot.write(&gpu.queue, Uniform {
                origin: [ox, oy], size: [px.0 as f32, px.1 as f32], out_size: [wp as f32, hp as f32],
                shadow_offset: [0.0, offset * scale], mask_size: [slot.mask.0 as f32, slot.mask.1 as f32],
                radius: desk.members[i].radius() * scale, pad: slot.pad as f32, sigma: sigma * scale / DOWN as f32,
                opacity, _unused: [0.0; 2],
            });
            drawn.push((i, desk.members[i].shadow()));
        }
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        comp.draw(&mut enc, &target.view, drawn.iter().map(|&(i, shadow)| (slots[i].as_ref().unwrap(), shadow)));
        if let Some((ov, ui)) = overlay.as_mut() {
            let app_dark = key.and_then(|i| desk.members[i].backdrop_dark()).unwrap_or(dark);
            let ort = ov.frame(real_t, step, app_dark);
            if !ort.instances.is_empty() {
                ui.prepare(&gpu.device, &gpu.queue, &mut ort.text.atlas, &ort.instances, (wp, hp));
                ui.draw_over(&mut enc, &target.view);
            }
        }
        let in_range = k >= start_frame && k < start_frame + frames;
        let shot = Shot {
            k,
            dump: in_range && (args.out.is_some() || args.strip.is_some()) && (k - start_frame).is_multiple_of(args.every),
            video: in_range && has_video,
            snap: k == snap_frame && args.snap.is_some(),
        };
        let first_pixels = args.first_pixels && k == 0;
        let want_pixels = shot.dump || shot.video || shot.snap || first_pixels;
        if want_pixels {
            for (shot, px) in readback.make_room(&gpu.device) { deliver(shot, px); }
            readback.copy(&mut enc, &target, shot);
        }
        gpu.queue.submit([enc.finish()]);
        if want_pixels { readback.submitted(); }
        let ms = frame_start.elapsed().as_secs_f64() * 1000.0;
        cpu_ms.push(ms);
        if let Some(tr) = trace.as_mut() { desk.members[selected].trace(tr, real_t * 1000.0, ms); }
        if first_pixels {
            let done = readback.finish(&gpu.device);
            let marker = json!({
                "headless_ms": launched.elapsed().as_secs_f64() * 1000.0, "gpu_init_ms": init_ms, "frame_cpu_ms": ms,
                "pixel_bytes": done.last().map_or(0, |(_, px)| px.len()),
            });
            println!("rune-first-pixels {marker}");
            for (shot, px) in done { deliver(shot, px); }
        } else if want_pixels {
            for (shot, px) in readback.ready(&gpu.device) { deliver(shot, px); }
        }
        if k == snap_frame && let Some(p) = &args.tree {
            let mut tree = desk.members[selected].tree(real_t);
            tree["windows"] = (0..n).map(|i| {
                let f = desk.snapshot(i, real_t);
                json!({ "key": f.key, "frame": f.frame, "visible": f.visible, "focused": f.focused })
            }).collect();
            let text = serde_json::to_string_pretty(&tree).unwrap();
            std::fs::write(p, text).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
        }
    }
    for (shot, px) in readback.finish(&gpu.device) { deliver(shot, px); }
    drop(deliver);
    if let Some(video) = writer.finish().unwrap_or_else(|e| die(&e)) {
        eprintln!("rune: wrote {} ({frames_written} frames at {fps} fps)", video.display());
    }
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    if let Some(p) = &args.strip { crate::tooling::write_strip(p, &strip, (wp, hp), &crops_px, gap, args.cols); }
    let wall = wall.elapsed().as_secs_f64();
    let mean = cpu_ms.iter().sum::<f64>() / cpu_ms.len() as f64;
    eprintln!("rune: desktop of {n} windows, {} frames in {wall:.2} s wall, gpu init {init_ms:.1} ms, frame cpu mean {mean:.3} ms", last_frame + 1);
    for m in &mut desk.members {
        if let Some(s) = m.summary(&gpu.device) { eprintln!("{s}"); }
    }
}

/// Sends a shortcut or menu pick to the key window's app, else the first other app that knows it.
fn fire<E>(desk: &Desk, f: impl Fn(&dyn Member) -> Result<(), E>) -> Result<(), E> {
    let order = desk.key.into_iter().chain(desk.z.iter().rev().copied().filter(|&i| Some(i) != desk.key));
    let mut first = None;
    for i in order {
        match f(desk.members[i].as_ref()) {
            Ok(()) => return Ok(()),
            Err(e) => { first.get_or_insert(e); }
        }
    }
    first.map_or(Ok(()), Err)
}

