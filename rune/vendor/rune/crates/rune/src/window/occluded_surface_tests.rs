//! A Metal surface whose window reports itself covered, with no window server needed: the layer's
//! delegate stands in for the backing view, and is its own window.

use super::*;
use std::sync::atomic::AtomicUsize;
use objc2_video::{ClassType, define_class, msg_send, rc::Retained, runtime::{AnyClass, AnyObject, NSObject}};

/// The fake window's `occlusionState`: 0 covered, 2 (`NSWindowOcclusionStateVisible`) shown.
static OCCLUSION: AtomicUsize = AtomicUsize::new(0);

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "RuneFakeOccludedView"]
    struct FakeView;

    impl FakeView {
        #[unsafe(method(window))]
        fn window(&self) -> *const Self { self }

        #[unsafe(method(occlusionState))]
        fn occlusion_state(&self) -> usize { OCCLUSION.load(Ordering::Relaxed) }
    }
);

/// An hour covered costs no failed `get_current_texture` (each leaks a texture id in
/// wgpu-core 30.0.1) and a re-check at most every two seconds; uncovered, it draws.
#[test]
fn an_occluded_surface_never_asks_wgpu_and_draws_once_visible() {
    let view: Retained<FakeView> = unsafe { msg_send![FakeView::class(), new] };
    let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
    let _: () = unsafe { msg_send![&*layer, setDelegate: &*view] };
    let instance = Gpu::instance();
    let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
    let surface = unsafe { instance.create_surface_unsafe(target) }.expect("a Metal layer surface");
    let gpu = match Gpu::try_new(instance, Some(&surface)) {
        Ok(gpu) => gpu,
        Err(error) => return eprintln!("skipped: {error}"),
    };
    let Some((config, _)) = surface_config(&surface, &gpu.adapter, (64, 64), false) else {
        return eprintln!("skipped: the adapter can't present to a Metal layer");
    };
    surface.configure(&gpu.device, &config);

    OCCLUSION.store(0, Ordering::Relaxed);
    assert!(unsafe { crate::platform::macos::layer_occluded(&layer) });
    let mut meter = SurfaceMeter::new("occluded", Arc::default());
    let start = Instant::now();
    let (mut at, mut wait, mut checks) = (start, OCCLUDED_RETRY, 0u64);
    while at < start + Duration::from_secs(3600) {
        if let Ok(_frame) = acquire(&surface, &gpu.device, &config, 1, &mut meter) { panic!("an occluded surface got a drawable"); }
        checks += 1;
        at = Missed::Occluded.retry(false, at, &mut wait).expect("occlusion re-checks on a deadline");
    }
    let totals = meter.totals();
    assert_eq!(totals.failed(), 0, "wgpu was asked for drawables it refuses as occluded");
    assert_eq!(totals.skipped, checks);
    assert!(checks <= 3600 / 2 + 3, "{checks} re-checks in an hour occluded");

    OCCLUSION.store(2, Ordering::Relaxed);
    let frame = acquire(&surface, &gpu.device, &config, 1, &mut meter).expect("a visible surface gets a drawable");
    let target = frame.texture.create_view(&Default::default());
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &target, depth_slice: None, resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    });
    gpu.queue.submit([encoder.finish()]);
    gpu.queue.present(frame);
    meter.presented();
    assert_eq!((meter.totals().presents, meter.totals().failed()), (1, 0));
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "RuneFakeCoveredView"]
    struct CoveredView;

    impl CoveredView {
        #[unsafe(method(window))]
        fn window(&self) -> *const Self { self }

        #[unsafe(method(occlusionState))]
        fn occlusion_state(&self) -> usize { 0 }
    }
);

/// A window's first frame draws before the window server reports the window on screen:
/// `PresumeShown` gets it a drawable from wgpu, and puts the layer's own delegate back.
#[test]
fn a_presumed_shown_surface_gets_its_first_drawable_while_reported_covered() {
    let view: Retained<CoveredView> = unsafe { msg_send![CoveredView::class(), new] };
    let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
    let _: () = unsafe { msg_send![&*layer, setDelegate: &*view] };
    let instance = Gpu::instance();
    let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
    let surface = unsafe { instance.create_surface_unsafe(target) }.expect("a Metal layer surface");
    let gpu = match Gpu::try_new(instance, Some(&surface)) {
        Ok(gpu) => gpu,
        Err(error) => return eprintln!("skipped: {error}"),
    };
    let Some((config, _)) = surface_config(&surface, &gpu.adapter, (64, 64), false) else {
        return eprintln!("skipped: the adapter can't present to a Metal layer");
    };
    surface.configure(&gpu.device, &config);
    let mut meter = SurfaceMeter::new("first", Arc::default());
    assert!(acquire(&surface, &gpu.device, &config, 1, &mut meter).is_err(), "a covered window gets no drawable");

    let shown = crate::platform::macos::PresumeShown::new(&surface).expect("a Metal surface");
    assert!(!crate::platform::macos::surface_occluded(&surface));
    let stand_in: *const AnyObject = unsafe { msg_send![&*layer, delegate] };
    let null: *const AnyObject = unsafe { msg_send![AnyClass::get(c"NSNull").expect("Foundation"), null] };
    let action: *const AnyObject = unsafe { msg_send![stand_in, actionForLayer: &*layer, forKey: std::ptr::null::<AnyObject>()] };
    assert_eq!(action, null, "PresumeShown suppresses implicit Core Animation actions");
    let frame = acquire(&surface, &gpu.device, &config, 1, &mut meter).expect("the first frame's drawable");
    drop(frame);
    drop(shown);

    let delegate: *const AnyObject = unsafe { msg_send![&*layer, delegate] };
    assert!(std::ptr::eq(delegate, Retained::as_ptr(&view).cast()), "the layer's delegate is back");
    assert!(crate::platform::macos::surface_occluded(&surface));
    assert_eq!(meter.totals().failed(), 0, "wgpu was never asked while covered");
}
