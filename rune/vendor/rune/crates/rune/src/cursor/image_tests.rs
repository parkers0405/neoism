use super::*;
use crate::{Cursor, Cx, El, Env, Runtime, el};

/// A `w`×`h` px PNG, opaque red with a transparent left column.
fn png(w: u32, h: u32) -> &'static [u8] {
    let img = ::image::RgbaImage::from_fn(w, h, |x, _| if x == 0 { ::image::Rgba([0, 0, 255, 0]) } else { ::image::Rgba([255, 0, 0, 255]) });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, ::image::ImageFormat::Png).unwrap();
    Box::leak(out.into_inner().into_boxed_slice())
}

#[test]
fn an_element_resolves_to_its_image_cursor_and_its_children_inherit_it() {
    static PENCIL: std::sync::LazyLock<CursorImage> = std::sync::LazyLock::new(|| CursorImage::png(png(64, 64), [2.0, 30.0]));
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(200.0, 200.0)
            .child(el().key("canvas").size(200.0, 100.0).cursor(Cursor::Image(*PENCIL)).on_press(|_: &mut ()| {})
                .child(el().key("swatch").size(40.0, 40.0)))
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 50.0);
    assert_eq!(rt.cursor(), Cursor::Image(*PENCIL));
    rt.pointer_move(20.0, 20.0);
    assert_eq!(rt.cursor(), Cursor::Image(*PENCIL), "inherited");
    rt.pointer_down(100.0, 50.0);
    rt.pointer_move(100.0, 150.0);
    assert_eq!(rt.cursor(), Cursor::Image(*PENCIL), "a held press keeps it");
    rt.pointer_up(100.0, 150.0);
    assert_eq!(rt.cursor(), Cursor::Default);
}

#[test]
fn images_are_equal_by_bytes_and_hot_spot() {
    static A: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    static B: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    static C: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 9];
    let a = CursorImage::png(&A, [1.0, 2.0]);
    assert_eq!(a, CursorImage::png(&A, [1.0, 2.0]));
    assert_eq!(Cursor::Image(a), Cursor::Image(CursorImage::png(&A, [1.0, 2.0])));
    assert_eq!(a, CursorImage::png(&B, [1.0, 2.0]), "the same bytes elsewhere (a const copied at each use)");
    assert_ne!(a, CursorImage::png(&C, [1.0, 2.0]));
    assert_ne!(a, CursorImage::png(&A[..7], [1.0, 2.0]));
    assert_ne!(a, CursorImage::png(&A, [1.0, 2.5]), "another hot spot is another cursor");
    use std::hash::BuildHasher;
    let state = std::hash::RandomState::new();
    assert_eq!(state.hash_one(a), state.hash_one(CursorImage::png(&B, [1.0, 2.0])));
    assert_eq!(format!("{a:?}"), "CursorImage { png_bytes: 8, hot: [1.0, 2.0] }");
}

#[test]
fn the_cache_decodes_each_image_once_per_scale() {
    let image = CursorImage::png(png(32, 32), [4.0, 4.0]);
    let mut cache = ImageCursors::<u32>::new();
    let mut made = 0;
    for _ in 0..3 {
        assert_eq!(cache.get(image, 2.0, |d| { made += 1; Ok(d.width) }), Some(32));
    }
    assert_eq!((cache.decodes, made), (1, 1), "hit, not decoded again");
    let copy: &'static [u8] = Box::leak(image.bytes().to_vec().into_boxed_slice());
    assert_eq!(cache.get(CursorImage::png(copy, [4.0, 4.0]), 2.0, |_| Ok(0)), Some(32), "a copy of the bytes hits too");
    assert_eq!(cache.decodes, 1);
    assert_eq!(cache.get(image, 1.0, |d| Ok(d.width)), Some(16), "another display's scale is another cursor");
    assert_eq!(cache.decodes, 2);
    assert_eq!(cache.cached(image, 2.0), Some(Some(32)));
}

#[test]
fn the_cache_is_bounded_and_keeps_the_newest() {
    let mut cache = ImageCursors::<usize>::new();
    let bytes = png(8, 8);
    for i in 0..MAX_ENTRIES + 5 {
        cache.get(CursorImage::png(bytes, [i as f32, 0.0]), 2.0, |_| Ok(i));
    }
    assert_eq!(cache.entries.len(), MAX_ENTRIES);
    assert_eq!(cache.cached(CursorImage::png(bytes, [0.0, 0.0]), 2.0), None, "the oldest went");
    let last = MAX_ENTRIES + 4;
    assert_eq!(cache.cached(CursorImage::png(bytes, [last as f32, 0.0]), 2.0), Some(Some(last)));
    assert!(cache.bytes <= MAX_BYTES);
}

#[test]
fn the_hot_spot_is_clamped_into_the_image() {
    let bytes = png(32, 20);
    let hot = |p: [f32; 2], scale| decode(CursorImage::png(bytes, p), scale).unwrap().hot;
    assert_eq!(hot([3.5, 2.0], 2.0), [7, 4], "points at 2x");
    assert_eq!(hot([3.5, 2.0], 1.0), [3, 2], "and at 1x");
    assert_eq!(hot([100.0, -5.0], 2.0), [31, 0]);
    assert_eq!(hot([f32::NAN, f32::INFINITY], 2.0), [0, 19]);
    assert_eq!(hot([16.0, 10.0], 1.0), [15, 9], "the far edge is just outside a 16×10 px image");
}

#[test]
fn decoding_scales_to_the_display_and_keeps_edges_clean() {
    let d = decode(CursorImage::png(png(32, 20), [0.0, 0.0]), 1.0).unwrap();
    assert_eq!((d.width, d.height, d.rgba.len()), (16, 10, 16 * 10 * 4));
    let edge = &d.rgba[..4];
    assert_eq!(edge[2], 0, "no blue from the transparent column bleeds in: {edge:?}");
    let d = decode(CursorImage::png(png(32, 20), [0.0, 0.0]), 3.0).unwrap();
    assert_eq!((d.width, d.height), (48, 30));
}

#[test]
fn a_bad_png_falls_back_to_the_arrow_once() {
    static NOT_PNG: &[u8] = b"not a png";
    let image = CursorImage::png(NOT_PNG, [0.0, 0.0]);
    let mut cache = ImageCursors::<()>::new();
    let mut made = false;
    assert_eq!(cache.get(image, 2.0, |_| { made = true; Ok(()) }), None);
    assert!(!made);
    assert_eq!(cache.get(image, 2.0, |_| Ok(())), None);
    assert_eq!(cache.decodes, 1, "remembered, so decoded and logged once");
    assert_eq!(cache.cached(image, 2.0), Some(None));
    assert!(decode(CursorImage::png(png(300, 8), [0.0, 0.0]), 2.0).is_err(), "too big");
    let refused = CursorImage::png(png(8, 8), [0.0, 0.0]);
    assert_eq!(cache.get(refused, 2.0, |_| Err::<(), _>("the OS refused it".into())), None);
}

#[test]
fn web_css_names_the_png_at_2x_with_the_hot_spot_in_points() {
    let css = |bytes, hot| super::css(CursorImage::png(bytes, hot), "blob:x");
    assert_eq!(css(png(64, 40), [2.0, 30.0]).unwrap(), r#"image-set(url("blob:x") 2x) 2 19, auto"#, "clamped into the 20 pt height");
    assert_eq!(css(png(64, 40), [3.7, f32::NAN]).unwrap(), r#"image-set(url("blob:x") 2x) 3 0, auto"#);
    assert_eq!(css(png(5, 5), [9.0, 9.0]).unwrap(), r#"image-set(url("blob:x") 2x) 2 2, auto"#, "an odd side rounds up");
    assert!(css(png(300, 8), [0.0, 0.0]).is_err(), "too big");
    assert!(css(&b"not a png at all, just some text"[..], [0.0, 0.0]).is_err());
    assert!(css(&png(8, 8)[..20], [0.0, 0.0]).is_err(), "cut short");
}

/// The NSCursor `Cursor::Image` sets: its image is sized in points with the PNG's pixels as
/// its bitmap, and its hot spot is in points.
#[cfg(target_os = "macos")]
#[test]
fn a_macos_image_cursor_is_sized_in_points() {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{NSPoint, NSSize};
    let d = decode(CursorImage::png(png(64, 40), [2.0, 30.0]), SOURCE_SCALE).unwrap();
    let cursor = crate::platform::macos::cursor::ns_cursor(&d).unwrap();
    // SAFETY: reading back AppKit objects made above.
    let (size, hot, pixels) = unsafe {
        let image: Retained<AnyObject> = msg_send_id![&*cursor, image];
        let size: NSSize = msg_send![&*image, size];
        let hot: NSPoint = msg_send![&*cursor, hotSpot];
        let reps: Retained<AnyObject> = msg_send_id![&*image, representations];
        let rep: Retained<AnyObject> = msg_send_id![&*reps, firstObject];
        let (w, h): (isize, isize) = (msg_send![&*rep, pixelsWide], msg_send![&*rep, pixelsHigh]);
        (size, hot, (w, h))
    };
    println!("NSCursor image {}×{} pt, bitmap {}×{} px, hot spot ({}, {}) pt", size.width, size.height, pixels.0, pixels.1, hot.x, hot.y);
    assert_eq!((size.width, size.height), (32.0, 20.0), "points, not pixels");
    assert_eq!(pixels, (64, 40), "the 2x pixels stay");
    assert_eq!((hot.x, hot.y), (2.0, 19.5), "the hot spot in points, clamped into the 20 pt height");
}
