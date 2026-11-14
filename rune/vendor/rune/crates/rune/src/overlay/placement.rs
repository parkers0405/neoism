//! Where an overlay goes: its screen and its position on that screen's edge.

use winit::dpi::PhysicalPosition;
use winit::event_loop::ActiveEventLoop;

use super::{Placement, ScreenEdge};
use crate::el::Align;
use crate::{DisplayTarget, OverlayOptions};

/// On the primary screen's top or bottom edge, `margin` points in, centred or `inset` points
/// from a side.
pub(super) fn position(event_loop: &ActiveEventLoop, at: Placement, options: &OverlayOptions) -> Option<PhysicalPosition<i32>> {
    let monitor = monitor(event_loop, &options.display)?;
    let scale = monitor.scale_factor();
    let (origin, area) = (monitor.position(), monitor.size());
    if options.placement.is_some() {
        let r = crate::overlay::options::local_rect(at, options.placement, (area.width as f32 / scale as f32, area.height as f32 / scale as f32));
        return Some(PhysicalPosition::new(origin.x + (r[0] as f64 * scale).round() as i32, origin.y + (r[1] as f64 * scale).round() as i32));
    }
    let px = |v: f32| (v as f64 * scale).round() as i32;
    let (w, h) = (px(at.size.0), px(at.size.1));
    let x = match at.align {
        Align::Start => origin.x + px(at.inset),
        Align::Center => origin.x + (area.width as i32 - w) / 2,
        Align::End => origin.x + area.width as i32 - w - px(at.inset),
    };
    #[allow(unused_mut)]
    let (mut top, mut bottom) = (origin.y, origin.y + area.height as i32);
    // Inside the taskbar and app bars of whichever Windows display it's on.
    #[cfg(target_os = "windows")]
    if let Some(work) = crate::platform::win32::work_area(&monitor) {
        (top, bottom) = work;
    }
    let y = match at.edge {
        ScreenEdge::Top => top + px(at.margin),
        ScreenEdge::Bottom => bottom - h - px(at.margin),
    };
    Some(PhysicalPosition::new(x, y))
}

pub(super) fn monitor(event_loop: &ActiveEventLoop, target: &DisplayTarget) -> Option<winit::monitor::MonitorHandle> {
    monitor_at(event_loop, target, None)
}

pub(crate) fn monitor_at(event_loop: &ActiveEventLoop, target: &DisplayTarget, sampled: Option<(i32, i32)>) -> Option<winit::monitor::MonitorHandle> {
    let monitors: Vec<_> = event_loop.available_monitors().collect();
    let primary = event_loop.primary_monitor().and_then(|p| monitors.iter().position(|m| *m == p));
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    let resolved = if let DisplayTarget::Native(crate::NativeDisplayId::X11Output(output)) = target {
        if !crate::window::is_wayland(event_loop) { crate::platform::x11::overlay::output_crtc(*output).map(|id| DisplayTarget::Native(crate::NativeDisplayId::X11Crtc(id))) } else { None }
    } else { None };
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    let target = resolved.as_ref().unwrap_or(target);
    let requested = match target {
        DisplayTarget::Id(id) => monitors.iter().position(|m| *m == id.0),
        DisplayTarget::Native(id) => monitors.iter().position(|m| crate::DisplayId::from_monitor(m.clone(), crate::window::is_wayland(event_loop)).native_id() == Some(*id)),
        _ => None,
    };
    let bounds: Vec<_> = monitors.iter().map(|m| {
        let (p, s) = (m.position(), m.size());
        [f64::from(p.x), f64::from(p.y), f64::from(p.x) + f64::from(s.width), f64::from(p.y) + f64::from(s.height)]
    }).collect();
    #[cfg(target_os = "windows")]
    let pointer = (*target == DisplayTarget::UnderPointer).then(crate::platform::win32::pointer_position).flatten();
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    let pointer = if *target == DisplayTarget::UnderPointer && !crate::window::is_wayland(event_loop) { crate::platform::x11::overlay::pointer_position() } else { None };
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android", target_arch = "wasm32"))]
    let pointer: Option<(i32, i32)> = None;
    let i = crate::overlay::options::choose_display(&bounds, primary, requested, sampled.or(pointer).map(|(x, y)| (f64::from(x), f64::from(y))))?;
    Some(monitors[i].clone())
}
