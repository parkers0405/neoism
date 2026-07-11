use super::*;

// Free-data deletion bug regression tests.
//
// The parser lowercases `delete_action` and stores the original case in
// `delete_data: bool`. The dispatcher used to check
// `delete.action == b'I'` etc., which was always false because the parser
// already normalized to lowercase, so the uppercase free-data variants
// silently leaked image bytes. These tests pin the fix.

#[test]
fn test_delete_uppercase_i_actually_frees_image_data() {
    // Regression: d=I (uppercase) must remove the stored image, not just
    // its placements. Pre-fix the dispatcher checked `delete.action == b'I'`
    // which was always false, so the image cache leaked.
    let mut term = make_test_term();
    store_red_pixel(&mut term, 7);
    assert!(term.graphics.get_kitty_image(7).is_some());

    // Parser path: d=I sets delete_action='I', then is normalized to
    // lowercase 'i' with delete_data=true.
    let mut state = KittyGraphicsState::default();
    let params = vec![b"G".as_ref(), b"a=d,d=I,i=7"];
    let resp = kitty_graphics_protocol::parse(&params, &mut state).unwrap();
    let delete = resp.delete_request.expect("expected DeleteRequest");
    assert_eq!(delete.action, b'i');
    assert!(delete.delete_data, "uppercase I must set delete_data");

    term.delete_graphics(delete);

    assert!(
        term.graphics.get_kitty_image(7).is_none(),
        "d=I must free image data — the dispatcher should rely on \
         delete.delete_data, not on a dead `action == b'I'` check"
    );
}

#[test]
fn test_delete_uppercase_a_clears_all_image_data() {
    let mut term = make_test_term();
    store_red_pixel(&mut term, 1);
    store_red_pixel(&mut term, 2);
    store_red_pixel(&mut term, 3);
    assert_eq!(term.graphics.kitty_images.len(), 3);

    let delete = DeleteRequest {
        action: b'a',
        image_id: 0,
        image_number: 0,
        placement_id: 0,
        x: 0,
        y: 0,
        z_index: 0,
        delete_data: true, // simulating d=A
    };
    term.delete_graphics(delete);

    assert!(
        term.graphics.kitty_images.is_empty(),
        "d=A must clear all image data, not just placements"
    );
    assert!(term.graphics.kitty_image_numbers.is_empty());
}

#[test]
fn test_delete_lowercase_a_keeps_image_data() {
    // Per spec: lowercase deletes placements only, image data stays so a
    // future `a=p` can still place the same image.
    let mut term = make_test_term();
    store_red_pixel(&mut term, 1);

    let delete = DeleteRequest {
        action: b'a',
        image_id: 0,
        image_number: 0,
        placement_id: 0,
        x: 0,
        y: 0,
        z_index: 0,
        delete_data: false, // d=a (lowercase)
    };
    term.delete_graphics(delete);

    assert!(
        term.graphics.get_kitty_image(1).is_some(),
        "Lowercase d=a must keep image data — only placements are removed"
    );
}

#[test]
fn test_delete_uppercase_n_frees_image_via_number() {
    // d=N: delete by image number, free data
    let mut term = make_test_term();
    let graphic = GraphicData {
        id: GraphicId::new(42),
        width: 1,
        height: 1,
        color_type: ColorType::Rgba,
        pixels: vec![255, 0, 0, 255],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    // Store with image_number=9
    term.graphics.store_kitty_image(42, Some(9), graphic);
    assert!(term.graphics.get_kitty_image(42).is_some());
    assert!(term.graphics.get_kitty_image_by_number(9).is_some());

    // d=N with image_id=9 (the parser stores the image *number* into
    // image_id for the d=n/N case via the `i=` key per spec).
    let delete = DeleteRequest {
        action: b'n',
        image_id: 0,
        image_number: 9, // canonical: I= for d=n
        placement_id: 0,
        x: 0,
        y: 0,
        z_index: 0,
        delete_data: true,
    };
    term.delete_graphics(delete);

    assert!(
        term.graphics.get_kitty_image(42).is_none(),
        "d=N must free image data resolved through the number map"
    );
}

#[test]
fn test_delete_uppercase_r_frees_image_range() {
    // d=R deletes a range of image_ids and frees their data.
    let mut term = make_test_term();
    store_red_pixel(&mut term, 1);
    store_red_pixel(&mut term, 5);
    store_red_pixel(&mut term, 10);
    assert_eq!(term.graphics.kitty_images.len(), 3);

    // d=R with x=range_start, y=range_end (inclusive). Source x/y carry
    // these values per the parser's field reuse.
    let delete = DeleteRequest {
        action: b'r',
        image_id: 0,
        image_number: 0,
        placement_id: 0,
        x: 1, // range start
        y: 5, // range end
        z_index: 0,
        delete_data: true,
    };
    term.delete_graphics(delete);

    // Images 1 and 5 should be gone, 10 should remain.
    assert!(term.graphics.get_kitty_image(1).is_none());
    assert!(term.graphics.get_kitty_image(5).is_none());
    assert!(
        term.graphics.get_kitty_image(10).is_some(),
        "Image outside range must survive"
    );
}
