use super::*;
use crate::render::{Gpu, Instance, KIND_RECT, KIND_SHADOW, NO_CLIP, Offscreen, Renderer};
use crate::{Color, Cx, El, Env, Runtime, el};
use bytemuck::Zeroable;

#[test]
fn native_frames_are_points_from_the_top_left() {
    let draw = MaterialDraw { id: NodeId(1), material: SystemMaterial::Glass, rect: [20.0, 40.0, 200.0, 100.0], radius: 24.0, continuous: false, opacity: 1.0 };
    assert_eq!(draw.points(2.0), ([10.0, 20.0, 100.0, 50.0], 12.0));
    assert_eq!(draw.points(1.0), ([20.0, 40.0, 200.0, 100.0], 24.0));
}

#[test]
fn one_radius_is_the_largest_corner_within_half_the_shorter_side() {
    assert_eq!(clamp_radius([8.0, 6.0, 4.0, 0.0], 100.0, 40.0), 8.0);
    assert_eq!(clamp_radius([8.0, 30.0, 4.0, 0.0], 100.0, 40.0), 20.0);
    assert_eq!(clamp_radius([500.0; 4], 30.0, 90.0), 15.0);
    assert_eq!(clamp_radius([-4.0; 4], 30.0, 90.0), 0.0);
    assert_eq!(clamp_radius([12.0; 4], 0.0, 90.0), 0.0);
}

#[test]
fn fallback_keeps_a_fill_and_tints_without_one() {
    let red = Color { r: 1.0, g: 0.0, b: 0.0, a: 0.5 };
    assert_eq!(fallback_fill(red, true), red);
    let dark = fallback_fill(Color { a: 0.0, ..red }, true);
    let light = fallback_fill(Color { a: 0.0, ..red }, false);
    assert!(dark.a > 0.9 && light.a > 0.9);
    assert!(dark.r < 0.3 && light.r > 0.9);
}

#[test]
fn mode_reads_the_environment_value() {
    assert_eq!(Mode::parse(None), Mode::Synced);
    assert_eq!(Mode::parse(Some("legacy")), Mode::Legacy);
    assert_eq!(Mode::parse(Some(" unsynced ")), Mode::Unsynced);
    assert_eq!(Mode::parse(Some("off")), Mode::Off);
    assert_eq!(Mode::parse(Some("0")), Mode::Off);
    assert_eq!(Mode::parse(Some("anything")), Mode::Synced);
}

const SHADOW: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.4 };

fn card() -> El {
    el().absolute(30.0, 20.0).size(120.0, 60.0).radius(18.0).shadow(8.0, 24.0, SHADOW)
        .system_material(SystemMaterial::Glass)
}

fn one_card(_: &u8, _: &mut Cx<u8>) -> El {
    el().size(200.0, 120.0).child(card())
}

fn runtime(view: fn(&u8, &mut Cx<u8>) -> El, native: bool) -> Runtime<u8> {
    let mut rt = Runtime::new(0, view, (200.0, 120.0), Env { scale: 2.0, system_dark: true });
    rt.native_materials = native;
    rt.frame(0.0);
    rt
}

fn kind(rt: &Runtime<u8>, kind: f32) -> Vec<Instance> {
    rt.instances.iter().filter(|q| q.params[1] == kind).copied().collect()
}

#[test]
fn material_elements_report_physical_rects_and_clamped_radii() {
    let rt = runtime(one_card, false);
    assert_eq!(rt.materials.len(), 1);
    let m = rt.materials[0];
    assert_eq!(m.material, SystemMaterial::Glass);
    assert_eq!(m.rect, [60.0, 40.0, 240.0, 120.0]);
    assert_eq!(m.radius, 36.0);
    assert_eq!(m.opacity, 1.0);
    assert_eq!(m.points(2.0), ([30.0, 20.0, 120.0, 60.0], 18.0));
}

#[test]
fn without_native_views_the_element_draws_its_fallback_and_a_full_shadow() {
    let rt = runtime(one_card, false);
    let fills = kind(&rt, KIND_RECT);
    let tint = fallback_fill(Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }, true).premul(1.0);
    assert!(fills.iter().any(|q| q.rect == [60.0, 40.0, 240.0, 120.0] && q.fill == tint), "{fills:?}");
    let shadows = kind(&rt, KIND_SHADOW);
    assert_eq!(shadows.len(), 1);
    assert_eq!(shadows[0].uv, [0.0; 4]);
}

#[test]
fn over_native_views_the_fill_goes_and_the_shadow_is_cut_out() {
    let rt = runtime(one_card, true);
    assert!(!rt.instances.iter().any(|q| q.params[1] == KIND_RECT && q.rect == [60.0, 40.0, 240.0, 120.0] && q.fill[3] > 0.0));
    let shadows = kind(&rt, KIND_SHADOW);
    assert_eq!(shadows.len(), 1);
    let s = shadows[0];
    // The hole is the element's own rect, relative to the shadow's grown and offset rect.
    assert_eq!([s.rect[0] + s.uv[0], s.rect[1] + s.uv[1], s.uv[2], s.uv[3]], [60.0, 40.0, 240.0, 120.0]);
    // Glass draws only continuous corners, so the hole is continuous too (a negative radius).
    assert_eq!(s.radii[3], -36.0);
}

#[test]
fn glass_rounds_continuously_and_popover_follows_the_flag() {
    fn cards(_: &u8, _: &mut Cx<u8>) -> El {
        let popover = |x: f32| el().absolute(x, 20.0).size(40.0, 40.0).radius(10.0).system_material(SystemMaterial::Popover);
        el().size(200.0, 120.0)
            .child(el().absolute(10.0, 20.0).size(40.0, 40.0).radius(10.0).system_material(SystemMaterial::Glass))
            .child(popover(60.0))
            .child(popover(110.0).continuous_corners())
    }
    let rt = runtime(cards, false);
    assert_eq!(rt.materials.iter().map(|m| m.continuous).collect::<Vec<_>>(), [true, false, true]);
    // The fallback fills take the same curve: negative radii draw continuous corners.
    let fill = |x: f32| kind(&rt, KIND_RECT).into_iter().find(|q| q.rect[0] == x * 2.0).unwrap().radii[0];
    assert_eq!([fill(10.0), fill(60.0), fill(110.0)], [-20.0, 20.0, -20.0]);
}

#[test]
fn a_dimmed_out_material_takes_no_native_view() {
    fn dimmed(_: &u8, _: &mut Cx<u8>) -> El {
        let card = |k: usize| el().absolute(k as f32 * 30.0, 0.0).size(20.0, 20.0).system_material(SystemMaterial::Popover);
        el().size(200.0, 120.0)
            .children((0..MAX_NATIVE_MATERIALS).map(|k| card(k).dim(0.0)))
            .child(card(MAX_NATIVE_MATERIALS))
    }
    let rt = runtime(dimmed, true);
    assert_eq!(rt.materials.len(), 1, "only the visible element takes a view");
    assert_eq!(rt.materials[0].rect[0], MAX_NATIVE_MATERIALS as f32 * 60.0);
}

#[test]
fn views_follow_their_element_across_insertions_and_unmounts() {
    use SystemMaterial::{Glass, Popover};
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    // A popover inserted before the glass leaves the glass on its view and gets a new one.
    assert_eq!(assign(&[(Glass, Some(a))], &[(Popover, b), (Glass, a)]), [None, Some(0)]);
    // Unmounting the first of two leaves the second on its own view.
    assert_eq!(assign(&[(Glass, Some(a)), (Glass, Some(b))], &[(Glass, b)]), [Some(1)]);
    // A new element reuses a spare view of its material, or one freed this frame, never another material's.
    assert_eq!(assign(&[(Glass, None), (Popover, None)], &[(Popover, c)]), [Some(1)]);
    assert_eq!(assign(&[(Glass, Some(a))], &[(Glass, c)]), [Some(0)]);
    assert_eq!(assign(&[(Glass, Some(a))], &[(Popover, a)]), [None]);
    // An element's own view wins over an earlier draw that could also take it.
    assert_eq!(assign(&[(Glass, Some(a)), (Glass, Some(b))], &[(Glass, c), (Glass, a)]), [Some(1), Some(0)]);
}

#[test]
fn a_window_move_never_turns_off_transactions_materials_need() {
    let mut flag = PresentFlag::default();
    assert_eq!(flag.materials(true), Some(true));
    // A resize animation starts and lands while materials are native: the layer stays on.
    assert_eq!(flag.moving(true), None);
    assert_eq!(flag.moving(false), None);
    // The last material unmounts: off.
    assert_eq!(flag.materials(false), Some(false));
    // A move alone turns it on and, landing, off again.
    assert_eq!(flag.moving(true), Some(true));
    assert_eq!(flag.materials(true), None);
    assert_eq!(flag.materials(false), None);
    assert_eq!(flag.moving(false), Some(false));
    assert_eq!(flag.moving(false), None);
}

#[test]
fn a_live_resize_keeps_transactions_on_until_it_ends() {
    let mut flag = PresentFlag::default();
    assert_eq!(flag.live(true), Some(true));
    // Frames during the drag land no window animation, which would turn the flag off.
    assert_eq!(flag.moving(false), None);
    assert_eq!(flag.materials(false), None);
    assert_eq!(flag.live(true), None);
    // The drag ends: later frames present on their own again.
    assert_eq!(flag.live(false), Some(false));
    // Materials keep it on past the drag.
    assert_eq!(flag.materials(true), Some(true));
    assert_eq!(flag.live(true), None);
    assert_eq!(flag.live(false), None);
}

#[cfg(target_os = "macos")]
#[test]
fn live_resize_end_notification_turns_off_transactions_unless_materials_or_move_need_them() {
    use objc2_video::{msg_send, rc::Retained, runtime::{AnyClass, AnyObject, Bool}};
    use std::{cell::Cell, rc::Rc};
    let (window, layer): (Retained<AnyObject>, Retained<AnyObject>) = unsafe {
        (msg_send![AnyClass::get(c"NSObject").unwrap(), new], msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new])
    };
    let flag = Rc::new(Cell::new(PresentFlag::default()));
    let token = macos::watch_live_end(&window, &layer, flag.clone()).expect("observer");
    let sync = || -> bool { unsafe { let b: Bool = msg_send![&*layer, presentsWithTransaction]; b.as_bool() } };
    let post_end = || unsafe {
        let center: Retained<AnyObject> = msg_send![AnyClass::get(c"NSNotificationCenter").unwrap(), defaultCenter];
        let name = objc2_foundation_video::NSString::from_str("NSWindowDidEndLiveResizeNotification");
        let _: () = msg_send![&*center, postNotificationName: &*name, object: &*window];
    };
    let mut f = flag.get();
    assert_eq!(f.live(true), Some(true));
    flag.set(f);
    unsafe { let _: () = msg_send![&*layer, setPresentsWithTransaction: Bool::YES]; }
    assert!(sync());
    post_end();
    assert!(!sync(), "NSWindowDidEndLiveResizeNotification turns off transactions when idle");

    // When materials also need transactions, ending live resize keeps the layer synced until materials stop.
    let mut f = flag.get();
    assert_eq!(f.materials(true), Some(true));
    assert_eq!(f.live(true), None);
    flag.set(f);
    unsafe { let _: () = msg_send![&*layer, setPresentsWithTransaction: Bool::YES]; }
    post_end();
    assert!(sync(), "materials keep transactions on after live resize ends");
    let mut f = flag.get();
    assert_eq!(f.materials(false), Some(false));
    flag.set(f);
    macos::unwatch_live_end(&token);
}

#[test]
fn materials_past_the_limit_draw_their_fallback() {
    fn many(_: &u8, _: &mut Cx<u8>) -> El {
        el().size(200.0, 120.0).children((0..MAX_NATIVE_MATERIALS + 1).map(|k| {
            el().absolute(k as f32 * 30.0, 0.0).size(20.0, 20.0).fill(Color::WHITE).system_material(SystemMaterial::Popover)
        }))
    }
    let rt = runtime(many, true);
    assert_eq!(rt.materials.len(), MAX_NATIVE_MATERIALS);
    let white = kind(&rt, KIND_RECT).into_iter().filter(|q| q.fill[3] > 0.99).count();
    assert_eq!(white, 1, "only the element past the limit paints its fill");
}

#[test]
fn material_opacity_includes_ancestors() {
    fn faded(_: &u8, _: &mut Cx<u8>) -> El {
        el().size(200.0, 120.0).child(el().size(200.0, 120.0).opacity(0.5).child(card().opacity(0.5)))
    }
    let rt = runtime(faded, true);
    assert!((rt.materials[0].opacity - 0.25).abs() < 1e-6);
}

#[test]
fn headless_runtimes_never_host_native_views_and_build_identically() {
    let a = Runtime::new(0u8, one_card, (200.0, 120.0), Env { scale: 2.0, system_dark: true });
    assert!(!a.native_materials);
    let (a, b) = (runtime(one_card, false), runtime(one_card, false));
    assert_eq!(bytemuck::cast_slice::<Instance, u8>(&a.instances), bytemuck::cast_slice::<Instance, u8>(&b.instances));
}

/// The shader leaves a shadow's hole clear and the rest as it was.
#[test]
fn shadow_hole_clears_the_interior_on_the_gpu() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, crate::render::OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 64, 64);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let shadow = Instance {
        rect: [8.0, 8.0, 48.0, 48.0], radii: [10.0; 4], fill: [0.0, 0.0, 0.0, 0.8],
        params: [0.0, KIND_SHADOW, 4.0, 1.0], clip: NO_CLIP, ..Instance::zeroed()
    };
    let mut read = |renderer: &mut Renderer, q: Instance| {
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[q], (64, 64));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0; 4]);
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        target.read(&gpu.device)
    };
    let full = read(&mut renderer, shadow);
    let cut = read(&mut renderer, Instance { radii: [10.0, 10.0, 10.0, 8.0], uv: [8.0, 8.0, 32.0, 32.0], ..shadow });
    let alpha = |px: &[u8], x: usize, y: usize| px[(y * 64 + x) * 4 + 3];
    assert!(alpha(&full, 32, 32) > 150);
    assert_eq!(alpha(&cut, 32, 32), 0, "the interior is clear");
    assert_eq!(alpha(&cut, 20, 20), 0);
    assert_eq!(alpha(&cut, 4, 32), alpha(&full, 4, 32), "outside the hole the shadow is unchanged");
    assert!(alpha(&cut, 10, 32) > 0);
    // The hole's own corner: rounded, so its very corner keeps some shadow.
    assert!(alpha(&cut, 16, 16) > 0);
}

/// With `srgb_blend`, a 62% white frost over the material reaches the window as Core Animation
/// would composite the same CALayer: 158 over black (linear blending gives 206), and
/// premultiplied 158 over nothing for the window server to blend. The shadow's hole stays clear.
#[test]
fn srgb_blend_frost_over_a_native_material_reads_as_core_animation() {
    fn frosted(frost: &u8, _: &mut Cx<u8>) -> El {
        let white = Color::WHITE.alpha(0.62 * f32::from(*frost));
        el().size(200.0, 120.0).child(card().continuous_corners().child(el().size(120.0, 60.0).fill(white)))
    }
    let gpu = Gpu::headless();
    let read = |srgb: bool, frost: u8, clear: [f32; 4]| {
        let mut rt = Runtime::new(frost, frosted, (200.0, 120.0), Env { scale: 2.0, system_dark: true });
        rt.native_materials = true;
        rt.set_srgb_blend(srgb);
        rt.frame(0.0);
        let format = if srgb { crate::render::OFFSCREEN_FORMAT.remove_srgb_suffix() } else { crate::render::OFFSCREEN_FORMAT };
        let mut renderer = Renderer::new(&gpu.device, format);
        renderer.set_srgb_blend(srgb);
        let target = Offscreen::with_format(&gpu.device, 400, 240, format);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (400, 240));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, clear);
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        let px = target.read(&gpu.device);
        let at = (100 * 400 + 180) * 4;
        [px[at], px[at + 1], px[at + 2], px[at + 3]]
    };
    let black = [0.0, 0.0, 0.0, 1.0];
    let near = |got: [u8; 4], want: [u8; 4]| assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1), "{got:?} != {want:?}");
    near(read(false, 1, black), [206, 206, 206, 255]);
    near(read(true, 1, black), [158, 158, 158, 255]);
    near(read(true, 1, [0.0; 4]), [158, 158, 158, 158]);
    near(read(true, 0, [0.0; 4]), [0, 0, 0, 0]);
}
