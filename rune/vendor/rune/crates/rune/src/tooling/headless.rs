//! Offscreen deterministic frame loop, timeline resolution and `--chrome` traffic-light checks.

use std::io::Write;

use crate::compose::Frame;
use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::time::Instant;

use super::*;

// Render targets contain sRGB-encoded premultiplied linear colour. PNG uses straight
// colour, so unpremultiply in linear space, not in the encoded byte domain.
#[cfg(test)]
pub(crate) fn straight_alpha(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = f32::from(pixel[3]) / 255.0;
        if alpha == 0.0 { pixel[..3].fill(0); }
        else if alpha < 1.0 {
            for c in &mut pixel[..3] {
                *c = (crate::color::srgb_encode((crate::color::srgb_decode(f32::from(*c) / 255.0) / alpha).clamp(0.0, 1.0)) * 255.0).round() as u8;
            }
        }
    }
}

pub fn headless<S: 'static>(app: crate::app::App<S>, args: Args) {
    started(try_headless(app, args));
}

/// `headless`, returning the error when a window whose own arguments aren't headless can't open.
pub(crate) fn try_headless<S: 'static>(mut app: crate::app::App<S>, args: Args) -> Result<(), String> {
    crate::render::discard_prewarm();
    if args.html.is_some() { return crate::tooling::html::export(app, &args); }
    if app.windows.is_empty() && args.desktop.is_none() {
        if args.list_windows { println!("main"); return Ok(()); }
        if args.window.as_deref().is_some_and(|key| key != "main") {
            die("no such window; this app has only 'main'");
        }
        let scale = args.scale.unwrap_or(2.0);
        if let Some((w, h)) = app.window_options.physical_size {
            app.spec.size = (w as f32 / scale, h as f32 / scale);
        }
        let transparent = app.window_options.transparent;
        headless_configured(app, args, transparent);
        return Ok(());
    }
    let windows = std::mem::take(&mut app.windows);
    let displays = desktop(&args, read_script(&args).as_ref()).displays();
    let accessory = app.accessory;
    let main = crate::WindowSpec::from_app("main", app);
    headless_window(std::iter::once(main.into()).chain(windows).collect(), &displays, args, crate::RunOptions { accessory })
}

pub(crate) fn headless_configured<S: 'static>(mut app: crate::app::App<S>, args: Args, transparent: bool) {
    // RUNE_HEADLESS_WAIT=SECS holds the run before it starts, so a sampling profiler can attach.
    if let Some(secs) = std::env::var("RUNE_HEADLESS_WAIT").ok().and_then(|s| s.parse::<f64>().ok()) {
        std::thread::sleep(std::time::Duration::from_secs_f64(secs));
    }
    let (chrome, titlebar) = (app.chrome, app.titlebar);
    let launched = Instant::now();
    if args.list_scenes {
        for (n, _) in app.spec.scenes {
            println!("{n}");
        }
        return;
    }
    #[cfg(feature = "hot")]
    if args.snap_on_patch.is_some() {
        return crate::hot::snap_window(app, args);
    }
    crate::tooling::interaction::gate::headless();
    let mut comp = app.compositor.take();
    let (srgb_blend, font_smoothing) = (app.srgb_blend, app.font_smoothing);
    let (on_focus, on_ime, on_mods) = (app.on_focus, app.on_ime, app.on_mods);
    let (on_reopen, on_open_url) = (app.on_reopen, app.on_open_url);
    // A `--realtime` run stands in for a window, so it starts the app's live work as one does.
    let mut on_ready = app.on_ready.take().filter(|_| args.realtime);
    let mut reports = crate::frame::encoded::FrameReports::<S>::new(app.on_frame_encoded.take(), Some("main"));
    let source = read_script(&args);
    let script = source.as_ref().map(load_script).unwrap_or_default();
    let timeline = Timeline::of(&args, &script).unwrap_or_else(|e| die(&e));
    let scale = args.scale.unwrap_or(2.0);
    let size = args.size.unwrap_or(app.spec.size);
    // Check the encoder before app initialization or GPU work.
    let mut writer = crate::tooling::record::Writer::new(((size.0 * scale).round() as u32, (size.1 * scale).round() as u32));
    if let Some(path) = &args.video {
        writer.video(path, timeline.fps).unwrap_or_else(|e| die(&e));
    }
    crate::tooling::headless_run::publish(crate::tooling::headless_run::HeadlessRun {
        script: args.script.clone(),
        events: script.iter().map(Ev::public).collect(),
        scale,
        fps: timeline.fps,
        size,
        start: timeline.start_frame as f64 * timeline.dt,
        frames: timeline.frames,
        end: timeline.last_frame as f64 * timeline.dt,
        desktop: None,
        windows: Default::default(),
    });
    let fit_height = app.fit_content_height;
    let app_options = app.window_options.clone();
    let (modifier_keys, tab_visits_controls, playground) = (app.modifier_keys, app.tab_visits_controls, app.playground.take());
    let (spec, state, shared) = app.into_parts(args.scene.as_deref());
    let shortcuts = crate::shortcuts::Guard::attach(&shared, std::sync::Arc::new(|| {}), true, false);
    let mut menus = crate::native_menu::Guard::attach(&shared, true);
    let dark = args.theme.as_deref() != Some("light");
    let state_ms = launched.elapsed().as_secs_f64() * 1000.0;
    let mut rt = runtime_for(&spec, state, size, Env::new(dark, scale), std::sync::Arc::new(|| {}));
    crate::playground::attach(&mut rt, playground, &spec, args.playground.clone(), args.variant.clone(), false);
    let chrome = rt.headless_chrome(chrome, titlebar, app_options.borderless);
    rt.on_focus = on_focus;
    rt.on_mods = on_mods;
    rt.app_ime.hook = on_ime;
    rt.transparent = transparent;
    rt.fit_height = fit_height;
    rt.set_srgb_blend(srgb_blend);
    rt.set_font_smoothing(font_smoothing);
    rt.set_modifier_keys(modifier_keys);
    rt.set_tab_visits_controls(tab_visits_controls);
    if let Some(bars) = args.scroll_bars { rt.set_system_scroll_bars(bars); }
    // Only limits the app set: the canvas opens at any size, below the default minimum too.
    let limits = (app_options.min_size.or(spec.min_size).unwrap_or((0.0, 0.0)), app_options.max_size);
    if fit_height {
        let height = crate::window::frame::clamp_size((size.0, rt.content_height()), limits.0, limits.1).1;
        rt.set_viewport((size.0, height), scale);
    }
    if args.tree.is_some() { rt.key_warnings = Some(Vec::new()); }
    rt.track_marks = comp.is_some();
    let runtime_ms = launched.elapsed().as_secs_f64() * 1000.0 - state_ms;

    let desktop = desktop(&args, source.as_ref());
    let opened = desktop.open_at(app_options.position, size);
    rt.set_window_origin((opened[0], opened[1]));
    let mut placed = None;
    let Timeline { fps, dt, start_frame, frames, snap_frame, last_frame } = timeline;
    reports.set_refresh(Some(fps));

    let t0 = Instant::now();
    let features = comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
    let gpu = Gpu::try_with_features(Gpu::instance_for(features), None, features).unwrap_or_else(|e| die(&e));
    let mut target_format = if srgb_blend { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
    let mut renderer = Renderer::new(&gpu.device, target_format);
    renderer.set_srgb_blend(srgb_blend);
    renderer.attach_text(&mut rt.text.atlas);
    let (wp, hp) = ((size.0 * scale).round() as u32, (size.1 * scale).round() as u32);
    let mut target = Offscreen::with_format(&gpu.device, wp, hp, target_format);
    let mut readback = crate::tooling::record::Readback::new(&gpu.device, &target, crate::tooling::record::DEPTH);
    let mut alpha = transparent.then(|| crate::render::surface_alpha::AlphaSurface::new(&gpu.device, target_format, (wp, hp)));
    let mut overlay = (args.keys.is_some() || args.pointer).then(|| {
        let mut ov = crate::input::keycaps::Overlay::new(size, scale, args.keys, args.keys_theme, args.pointer);
        ov.set_srgb_blend(srgb_blend);
        let mut ui = Renderer::new(&gpu.device, target_format);
        ui.set_srgb_blend(srgb_blend);
        (ov, ui)
    });
    let init_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let mut trace = args.trace.as_ref().map(|p| {
        let mut trace = Trace::open(p, size, fps, scale);
        trace.all = args.trace_all;
        trace
    });
    rt.trace_channels = trace.as_ref().map(|_| Default::default());
    let gate_out = std::env::var_os("RUNE_MOTION_GATE").map(std::path::PathBuf::from);
    let mut gate = gate_out.as_ref().map(|_| crate::motion_gate::Sampler::new(&mut rt, fps as f64));
    rt.scene.store_mut().audit_retargets(trace.is_some());
    let mut strip: Vec<Vec<u8>> = Vec::new();
    let crops_px: Vec<[u32; 4]> = args.crops.iter().map(|c| c.map(|v| (v * scale).round() as u32)).collect();
    let gap = (4.0 * scale) as u32;
    let wall = Instant::now();
    let mut cpu_ms = Vec::new();
    let mut anim_ms = Vec::new();
    let mut rest_ms = Vec::new();
    let mut next_ev = 0;
    let mut last_down = (0.0f32, 0.0f32);
    // Events at t = 0 find keys and hit elements in a laid-out first frame, as input reaches a
    // real window only once it has drawn. That frame mounts the initial state with any queued
    // updates; frame 0 then draws the events' effects.
    let mut take_requests = |rt: &mut Runtime<S>| {
        shortcuts.drain();
        let requests = shared.take();
        menus.drain();
        for f in requests.updates {
            rt.update_if(f);
            menus.drain();
        }
        if let Some(p) = requests.position { rt.set_window_origin(desktop.to_points(p)); }
        requests.place
    };
    let mut frames_written = 0usize;
    let has_video = writer.has_video();
    let snap_crop = std::cell::Cell::new(None::<[u32; 4]>);
    // Finished readbacks, in frame order.
    let mut deliver = |shot: Shot, mut px: Vec<u8>| {
        if args.chrome_overlay
            && let Some(lights) = chrome.traffic_light_rects()
        {
            paint_traffic_lights(&mut px, wp, hp, scale, &lights);
        }
        if shot.dump {
            if args.strip.is_some() {
                strip.push(tile(&px, wp, hp, &crops_px, gap));
            }
            if let Some(dir) = &args.out {
                writer.png(dir.join(format!("frame_{:04}.png", shot.k - start_frame)), px.clone()).unwrap_or_else(|e| die(&e));
            }
        }
        if shot.snap && let (Some(path), Some(r)) = (&args.snap, snap_crop.get()) {
            write_png(path, r[2], r[3], &crop::crop(&px, wp, r)).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
        } else if shot.snap {
            writer.png(args.snap.clone().unwrap(), px.clone()).unwrap_or_else(|e| die(&e));
        }
        if shot.video {
            frames_written += 1;
            writer.frame(px).unwrap_or_else(|e| die(&e));
        }
    };
    let mut early_place = None;
    // `Tx::resize_window` moves the viewport on the virtual clock; the canvas keeps its size.
    let mut frame_anim = None;
    let mut resize_from = rt.viewport;
    // As on macOS, a frame change asked for while nothing moves waits a frame to start.
    let mut idle = true;
    if script.iter().find(|e| !matches!(e.kind, Kind::Mark(_))).is_some_and(|e| e.t <= 1e-9) {
        early_place = take_requests(&mut rt);
        rt.frame(0.0);
        rt.open_popup_headless();
    }

    #[cfg(feature = "frame-profile")]
    let mut profile = crate::frame::profile::Log::new(last_frame + 1);
    for k in 0..=last_frame {
        #[cfg(feature = "frame-profile")]
        let (alloc0, commits0, cpu0) = {
            crate::frame::profile::take();
            (procwatch::alloc::thread_allocations(), rt.stats.commits, crate::frame::profile::cpu_ms())
        };
        let real_t = k as f64 * dt;
        if args.realtime {
            let ahead = real_t - wall.elapsed().as_secs_f64();
            if ahead > 0.0 { std::thread::sleep(std::time::Duration::from_secs_f64(ahead)); }
        }
        let frame_start = Instant::now();
        #[cfg(feature = "frame-profile")]
        let events0 = next_ev;
        while next_ev < script.len() && script[next_ev].t <= real_t + 1e-9 {
            let e = &script[next_ev];
            next_ev += 1;
            if let Some(g) = gate.as_mut() && !matches!(e.kind, Kind::Mark(_)) { g.input("script"); }
            let (x, y) = match &e.pos {
                Pos::Xy(x, y) => (*x, *y),
                Pos::Key(key) => rt.find(key).unwrap_or_else(|| die(&format!("script: no element with key {key:?}"))),
                Pos::Rel(_, _) if e.kind.at_pointer() => rt.pointer().unwrap_or((0.0, 0.0)),
                Pos::Rel(dx, dy) => (last_down.0 + dx, last_down.1 + dy),
            };
            match &e.kind {
                Kind::Mark(name) if args.realtime => {
                    use std::io::Write as _;
                    println!("rune-mark {name} {}", e.t);
                    let _ = std::io::stdout().flush();
                }
                Kind::Mark(_) => {}
                Kind::Focus(focused) => rt.window_focus(*focused),
                // The one window always shows here.
                Kind::Reopen => if let Some(f) = on_reopen { rt.update(|s, tx| { f(s, true, tx); }); },
                Kind::OpenUrl(url) => if let Some(f) = on_open_url { rt.update(|s, tx| f(s, url, tx)); },
                Kind::Shortcut(shortcut) => shared.shortcuts.fire(shortcut).unwrap_or_else(|e| die(&format!("script shortcut: {e}"))),
                Kind::Menu(item) => crate::native_menu::fire(&shared, item).unwrap_or_else(|e| die(&format!("script menu: {e}"))),
                Kind::PopupMenu(picked) => rt.resolve_popup(picked.as_deref()).unwrap_or_else(|e| die(&format!("script pop-up menu: {e}"))),
                Kind::FileDialog(path) => rt.resolve_file_dialog(path.clone()).unwrap_or_else(|e| die(&format!("script file dialog: {e}"))),
                Kind::Move => rt.pointer_move(x, y),
                Kind::Down => {
                    last_down = (x, y);
                    rt.pointer_down(x, y);
                }
                Kind::Up => rt.pointer_up(x, y),
                Kind::SecondaryDown => { last_down = (x, y); rt.secondary_down(x, y); }
                Kind::SecondaryUp => rt.secondary_up(x, y),
                Kind::Key(k, false) => rt.key(k.clone()),
                Kind::Key(k, true) => rt.repeat_key(k.clone()),
                Kind::Physical(event) => rt.physical_input(*event),
                Kind::Paste(content) => rt.paste_content(content.clone()),
                Kind::Wheel(dx, dy) => wheel(&mut rt, (x, y), *dx, *dy),
                Kind::Fling(vx, vy) => fling(&mut rt, (x, y), *vx, *vy),
                Kind::Files(step) => file_input(&mut rt, step, (x, y)),
                &Kind::Resize(dw, dh) => eased_input::step_resize(&mut rt, &mut resize_from, (dw, dh), scale),
                kind => {
                    if kind.is_down() { last_down = (x, y); }
                    canvas_input(&mut rt, kind, (x, y));
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
        // Frame 0 has drawn: a window runs `on_ready` after its first frame.
        if k == 1 && let Some(mut ready) = on_ready.take() { ready(&crate::WindowHandle::new("main")); }
        let requested_place = take_requests(&mut rt);
        take_window_frame(&mut rt, &mut frame_anim, real_t, limits);
        if !idle { step_window_frame(&mut rt, &mut frame_anim, real_t, scale); }
        rt.window_resting = frame_anim.is_none() && rt.window_frame.is_none();
        let step = if k == 0 { 0.0 } else { dt };
        if let Some(g) = gate.as_mut() { g.before(&mut rt); }
        let mut animating = rt.frame(step);
        rt.open_popup_headless();
        // As on macOS, a frame change this commit asked for starts with the springs it
        // started, which have shown a step unless they held for the window.
        if rt.window_frame.is_some() {
            let shown = if rt.fit_held { 0.0 } else { step };
            take_window_frame(&mut rt, &mut frame_anim, real_t - shown, limits);
        }
        idle = !animating && frame_anim.is_none();
        // Updates, poll and input handlers place too; the latest request this frame wins.
        if let Some(place) = shared.take_place().or(requested_place).or(early_place.take()) {
            let (display, frame) = desktop.place(&place, size);
            rt.set_window_origin((frame[0], frame[1]));
            let record = json!({ "t": real_t, "display": display, "frame": frame });
            println!("rune-placed {record}");
            placed = Some(record);
        }
        let update_ms = frame_start.elapsed().as_secs_f64() * 1000.0;
        if renderer.srgb_blend() != rt.srgb_blend() {
            // `Tx::set_srgb_blend`: the same bytes (sRGB-encoded) reach the readback either way.
            target_format = if rt.srgb_blend() { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
            crate::window::rebuild_renderer(&gpu.device, &mut renderer, target_format, &mut rt);
            target = Offscreen::with_format(&gpu.device, wp, hp, target_format);
            if alpha.is_some() { alpha = Some(crate::render::surface_alpha::AlphaSurface::new(&gpu.device, target_format, (wp, hp))); }
            if let Some((ov, ui)) = overlay.as_mut() {
                ov.set_srgb_blend(rt.srgb_blend());
                ui.set_format(&gpu.device, target_format);
            }
        }
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (wp, hp));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let view = alpha.as_ref().map_or(&target.view, |a| &a.view);
        if let Some(c) = comp.as_mut() {
            let mut f = Frame {
                device: &gpu.device,
                queue: &gpu.queue,
                encoder: &mut enc,
                target: view,
                format: target_format,
                size: (wp, hp),
                dt: step,
                update_ms,
                rt: &mut rt,
                ui: &mut renderer,
            };
            c.render(&mut f);
            animating |= c.animating();
        } else {
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (wp, hp));
            renderer.draw(&mut enc, view, rt.clear_color());
        }
        if let Some((ov, ui)) = overlay.as_mut() {
            let ort = ov.frame(real_t, step, rt.backdrop_dark().unwrap_or(dark));
            if !ort.instances.is_empty() {
                ui.prepare(&gpu.device, &gpu.queue, &mut ort.text.atlas, &ort.instances, (wp, hp));
                ui.draw_over(&mut enc, view);
            }
        }
        if let Some(alpha) = &alpha { alpha.draw(&mut enc, &target.view); }
        let in_range = k >= start_frame && k < start_frame + frames;
        let shot = Shot {
            k,
            dump: in_range && (args.out.is_some() || args.strip.is_some()) && (k - start_frame).is_multiple_of(args.every),
            video: in_range && has_video,
            snap: k == snap_frame && args.snap.is_some(),
        };
        if shot.snap { snap_crop.set(crop::snap_rect(&rt, &args, scale, (wp, hp))); }
        let first_pixels = args.first_pixels && k == 0;
        let want_pixels = shot.dump || shot.video || shot.snap || first_pixels;
        if want_pixels {
            for (shot, px) in readback.make_room(&gpu.device) { deliver(shot, px); }
            readback.copy(&mut enc, &target, shot);
        }
        renderer.submit(&gpu.queue, enc);
        reports.drawn(reports.stamp(), None, frame_start, update_ms);
        rt.interaction_presented(Instant::now(), None);
        if want_pixels { readback.submitted(); }
        let ms = frame_start.elapsed().as_secs_f64() * 1000.0;
        #[cfg(feature = "frame-profile")]
        profile.push(crate::frame::profile::Row {
            phases: crate::frame::profile::take(), update_ms, encode_ms: ms - update_ms, events: next_ev - events0,
            allocations: procwatch::alloc::thread_allocations() - alloc0, commits: rt.stats.commits - commits0,
            why: std::mem::take(&mut rt.commit_why), shaped: rt.stats.shaped, animating, cpu_ms: cpu0.map(|start| crate::frame::profile::cpu_ms().unwrap() - start),
        });
        cpu_ms.push(ms);
        if k > 0 {
            if animating { anim_ms.push(ms) } else { rest_ms.push(ms) }
        }

        if comp.is_some() {
            // Pace like a display link so a compositor's async readbacks (GPU timers) land between frames.
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        }
        if let Some(tr) = trace.as_mut() {
            tr.frame(&mut rt, real_t * 1000.0, ms);
        }
        if let Some(g) = gate.as_mut() { g.frame(&mut rt, animating); }
        if first_pixels {
            let wait = Instant::now();
            let done = readback.finish(&gpu.device);
            let marker = json!({
                "headless_ms": launched.elapsed().as_secs_f64() * 1000.0,
                "state_ms": state_ms, "runtime_ms": runtime_ms, "gpu_init_ms": init_ms,
                "frame_cpu_ms": ms, "readback_ms": wait.elapsed().as_secs_f64() * 1000.0,
                "pixel_bytes": done.last().map_or(0, |(_, px)| px.len()),
            });
            println!("rune-first-pixels {marker}");
            std::io::stdout().flush().unwrap();
            for (shot, px) in done { deliver(shot, px); }
        } else if want_pixels {
            for (shot, px) in readback.ready(&gpu.device) { deliver(shot, px); }
        }
        if k == snap_frame
            && args.chrome_overlay
            && let Some(lights) = chrome.traffic_light_rects()
        {
            report_traffic_light_overlaps(&rt, &lights);
        }
        if k == snap_frame
            && let Some(p) = &args.tree
        {
            let mut tree = tree_json(&rt, real_t);
            if let Some(placed) = &placed { tree["placed"] = placed.clone(); }
            let text = serde_json::to_string_pretty(&tree).unwrap();
            std::fs::write(p, text).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
        }
    }
    for (shot, px) in readback.finish(&gpu.device) { deliver(shot, px); }
    rt.interaction_flush();
    drop(deliver);
    let written = frames_written;
    if let Some(video) = writer.finish().unwrap_or_else(|e| die(&e)) {
        eprintln!("rune: wrote {} ({written} frames at {fps} fps)", video.display());
    }
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();

    if let Some(p) = &args.strip { write_strip(p, &strip, (wp, hp), &crops_px, gap, args.cols); }
    if let (Some(g), Some(p)) = (gate, &gate_out) {
        crate::motion_gate::evidence("", &g.rec, Some(p.display().to_string()));
        let text: String = crate::motion_gate::check(&g.rec).iter().map(|f| format!("{f}\n")).collect();
        std::fs::write(p, text).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
    }

    if frames > 0 {
        let wall = wall.elapsed().as_secs_f64();
        eprintln!("rune: {} frames in {wall:.2} s wall ({:.1} frames/s)", last_frame + 1, (last_frame + 1) as f64 / wall);
    }
    let q = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(f64::total_cmp);
        if v.is_empty() { 0.0 } else { v[((v.len() - 1) as f64 * p).round() as usize] }
    };
    let mut sorted = cpu_ms.clone();
    #[cfg(feature = "frame-profile")]
    profile.report();
    let steady: Vec<f64> = cpu_ms.iter().skip(1).copied().collect();
    let mean = if steady.is_empty() { cpu_ms[0] } else { steady.iter().sum::<f64>() / steady.len() as f64 };
    eprintln!(
        "rune: {} frames at {fps} fps, gpu init {init_ms:.1} ms, first frame {:.2} ms, frame cpu mean {mean:.3} ms p50 {:.3} p95 {:.3} max {:.3}, {} instances",
        cpu_ms.len(),
        cpu_ms[0],
        q(&mut sorted, 0.5),
        q(&mut sorted, 0.95),
        q(&mut sorted, 1.0),
        rt.instances.len()
    );
    let (a50, a95) = (q(&mut anim_ms, 0.5), q(&mut anim_ms, 0.95));
    let (r50, r95) = (q(&mut rest_ms, 0.5), q(&mut rest_ms, 0.95));
    eprintln!(
        "rune: animating frames {} (cpu p50 {a50:.3} p95 {a95:.3} ms), at-rest frames {} (the window draws none; forced cpu p50 {r50:.3} p95 {r95:.3} ms), layout nodes {}, glyphs {}",
        anim_ms.len(),
        rest_ms.len(),
        rt.stats.layout_nodes,
        rt.stats.glyphs
    );
    if let Some(s) = comp.as_mut().and_then(|c| c.summary(&gpu.device)) {
        eprintln!("{s}");
    }
}

/// Takes the size handlers asked for a headless window, inside the limits the app set; its
/// viewport moves there on the virtual clock from `start`. Centring has nothing to centre on.
fn take_window_frame<S>(rt: &mut Runtime<S>, anim: &mut Option<crate::window::frame::FrameAnim>, start: f64,
    (min, max): ((f32, f32), Option<(f32, f32)>)) {
    use crate::window::frame::{FrameAnim, clamp_size};
    rt.fit_held = false;
    let Some(request) = rt.window_frame.take() else { return };
    let Some(size) = request.size else { return };
    let (w, h) = clamp_size(size, min, max);
    let from = [0.0, 0.0, rt.viewport.0, rt.viewport.1];
    *anim = Some(FrameAnim::retarget(anim.as_ref(), request.spring, start, from, [0.0, 0.0, w, h]));
}

/// Moves a headless window's viewport one frame along, before the frame lays out.
fn step_window_frame<S>(rt: &mut Runtime<S>, anim: &mut Option<crate::window::frame::FrameAnim>, now: f64, scale: f32) {
    let Some(moving) = *anim else { return };
    let [_, _, w, h] = crate::window::frame::snap(moving.sample(now).0, scale);
    rt.step_viewport((w, h), scale);
    if moving.done(now) { *anim = None; }
}

/// Which frames a headless run renders and dumps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Timeline {
    pub fps: f64,
    pub dt: f64,
    pub start_frame: usize,
    /// Frames dumped from `start_frame` (0 when nothing is dumped).
    pub frames: usize,
    pub snap_frame: usize,
    pub last_frame: usize,
}

impl Timeline {
    pub(crate) fn of(args: &Args, script: &[Ev]) -> Result<Timeline, String> {
        let mark = |name: &str| script.iter().find_map(|e| matches!(&e.kind, Kind::Mark(n) if n == name).then_some(e.t));
        let resolve = |spec: &String| {
            // Exact names win over offsets, including names such as "step-1" or "end+0.5".
            if let Some(t) = mark(spec) { return Ok(t); }
            let names: Vec<_> = script.iter().filter_map(|e| match &e.kind { Kind::Mark(n) => Some(n.as_str()), _ => None }).collect();
            let known = if names.is_empty() { "none".into() } else { names.join(", ") };
            // Only a complete numeric suffix is an offset. Starting at the first valid sign
            // keeps a scientific exponent's sign inside the number (end-1e-3).
            for (i, _) in spec.char_indices().skip(1).filter(|&(_, c)| c == '+' || c == '-') {
                if let Ok(offset) = spec[i..].parse::<f64>() && offset.is_finite() {
                    let name = &spec[..i];
                    return mark(name).map(|t| (t + offset).max(0.0))
                        .ok_or_else(|| format!("no mark {name:?} in the script (marks: {known})"));
                }
            }
            if spec.contains(['+', '-']) {
                Err(format!("invalid mark time {spec:?}: expected an exact mark name or NAME+SECONDS/NAME-SECONDS (marks: {known})"))
            } else {
                Err(format!("no mark {spec:?} in the script (marks: {known})"))
            }
        };
        let fps = args.fps.max(1.0);
        let dt = 1.0 / fps;
        let start = args.start_mark.as_ref().map_or(Ok(args.start), resolve)?;
        let at = args.at_mark.as_ref().map(resolve).transpose()?.or(args.at);
        let start_frame = (start * fps).round() as usize;
        let script_end = script.last().map_or(0.0, |e| e.t);
        // Output without a length runs to the `end` mark, or one second after the script.
        let until = |t: f64| ((t * fps).round() as usize).saturating_sub(start_frame) + 1;
        let auto = || until(mark("end").unwrap_or(script_end + 1.0));
        let frames = args.frames.unwrap_or_else(|| match (args.duration, args.video.is_some() || args.out.is_some()) {
            (Some(Length::Seconds(d)), _) => (d * fps).round().max(1.0) as usize,
            (Some(Length::Auto), _) | (None, true) => auto(),
            (None, false) => 0,
        });
        let snap_t = at.unwrap_or(if frames > 0 {
            (start_frame + frames - 1) as f64 * dt
        } else if script.is_empty() {
            0.0
        } else {
            script_end + 1.0
        });
        let snap_frame = (snap_t * fps).round() as usize;
        let last_frame = snap_frame.max(start_frame + frames.max(1) - 1);
        Ok(Timeline { fps, dt, start_frame, frames, snap_frame, last_frame })
    }
}

/// A frame whose pixels are being read back, and where they go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shot {
    pub k: usize,
    pub dump: bool,
    pub video: bool,
    pub snap: bool,
}

/// Antialiased discs where macOS draws close, minimise and zoom, plus a hairline box 4 pt
/// around them (the clearance an app should keep).
fn paint_traffic_lights(px: &mut [u8], w: u32, h: u32, scale: f32, lights: &[[f32; 4]; 3]) {
    const COLORS: [[f32; 3]; 3] = [[255.0, 95.0, 87.0], [254.0, 188.0, 46.0], [40.0, 200.0, 64.0]];
    let mut blend = |x: i64, y: i64, c: [f32; 3], a: f32| {
        if x < 0 || y < 0 || x >= i64::from(w) || y >= i64::from(h) || a <= 0.0 {
            return;
        }
        let i = ((y as u32 * w + x as u32) * 4) as usize;
        for k in 0..3 {
            px[i + k] = (f32::from(px[i + k]) * (1.0 - a) + c[k] * a).round() as u8;
        }
    };
    for (r, c) in lights.iter().zip(COLORS) {
        let (cx, cy, rad) = ((r[0] + r[2] / 2.0) * scale, (r[1] + r[3] / 2.0) * scale, r[2] / 2.0 * scale);
        for y in (cy - rad - 1.0) as i64..=(cy + rad + 1.0) as i64 {
            for x in (cx - rad - 1.0) as i64..=(cx + rad + 1.0) as i64 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                blend(x, y, c, (rad - d + 0.5).clamp(0.0, 1.0));
            }
        }
    }
    let m = 4.0;
    let (x0, y0) = ((lights[0][0] - m) * scale, (lights[0][1] - m) * scale);
    let (x1, y1) = ((lights[2][0] + lights[2][2] + m) * scale, (lights[2][1] + lights[2][3] + m) * scale);
    let cyan = [0.0, 200.0, 255.0];
    for x in x0 as i64..=x1 as i64 {
        blend(x, y0 as i64, cyan, 0.8);
        blend(x, y1 as i64, cyan, 0.8);
    }
    for y in y0 as i64..=y1 as i64 {
        blend(x0 as i64, y, cyan, 0.8);
        blend(x1 as i64, y, cyan, 0.8);
    }
}

/// Lists visible text, icons and controls whose drawn rect comes within 4 pt of a traffic light.
fn report_traffic_light_overlaps<S>(rt: &Runtime<S>, lights: &[[f32; 4]; 3]) {
    let m = 4.0;
    let zone = [lights[0][0] - m, lights[0][1] - m, lights[2][0] + lights[2][2] + m, lights[2][1] + lights[2][3] + m];
    let mut n = 0;
    for e in rt.elements() {
        let drawn = e.text.is_some() || matches!(e.role, "icon" | "button" | "textbox");
        let v = e.visible;
        let (x1, y1) = (v[0] + v[2], v[1] + v[3]);
        if drawn && e.opacity > 0.01 && v[2] > 0.0 && v[3] > 0.0 && v[0] < zone[2] && x1 > zone[0] && v[1] < zone[3] && y1 > zone[1] {
            eprintln!("rune: chrome: {} overlaps the traffic lights at {:?}", e.path, e.visible.map(r2));
            n += 1;
        }
    }
    eprintln!("rune: chrome: traffic lights at {:?}, {n} overlapping elements", lights[0].map(r2));
}
