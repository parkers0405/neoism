use super::*;

// kitten icat regression: multiple invocations must not collapse into
// the last image. Reproduces the user-reported issue where running
// `kitten icat` repeatedly only renders the most recent image.

#[test]
fn test_kitten_icat_two_invocations_without_explicit_id_keep_both_images() {
    // The user reported that running `kitten icat` multiple times only
    // renders the last image. icat doesn't always send an `i=` parameter,
    // and prior to this fix Rio's parser left image_id at 0, so every
    // implicit-id transmission collided in `kitty_images[0]` and
    // `kitty_placements[(0, 0)]`. After the fix the parser auto-assigns
    // a unique image_id and the placement layer auto-assigns a unique
    // internal placement_id, so both icat outputs survive.
    let mut term = make_test_term();
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    // Two distinguishable 1x1 RGBA pixels (red, then green).
    icat_invocation(&mut term, b"/wAA/w==", None); // red
    icat_invocation(&mut term, b"AP8A/w==", None); // green

    assert_eq!(
        term.graphics.kitty_images.len(),
        2,
        "Both icat invocations should produce distinct stored images"
    );
    assert_eq!(
        term.graphics.kitty_placements.len(),
        2,
        "Both icat placements should remain visible — only the last one \
         survived before the fix"
    );
}

#[test]
fn test_kitten_icat_two_invocations_with_same_explicit_id_each_get_unique_placement() {
    // Even when icat reuses the same `i=N` (which kitty itself allows
    // and uses for re-transmission), the *placements* should still be
    // distinct so both copies render. The image data is shared (the
    // second transmission overwrites it per spec) but each placement
    // gets its own internal placement_id.
    let mut term = make_test_term();
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    icat_invocation(&mut term, b"/wAA/w==", Some(1));
    icat_invocation(&mut term, b"/wAA/w==", Some(1));

    // One image (re-transmissions overwrite at same id per spec).
    assert_eq!(term.graphics.kitty_images.len(), 1);
    // Two placements (each `a=T` with implicit p=0 must get its own
    // internal placement_id so the prior placement isn't overwritten).
    assert_eq!(
        term.graphics.kitty_placements.len(),
        2,
        "Two `a=T` calls with the same image_id must produce two \
         placements, not collapse into one"
    );
}

#[test]
fn test_implicit_image_ids_are_distinct() {
    // Two parses with no `i=` should yield two different graphic IDs.
    let mut state = KittyGraphicsState::default();

    let p1 = vec![
        b"G".as_ref(),
        b"a=t,f=32,s=1,v=1".as_ref(),
        b"/wAA/w==".as_ref(),
    ];
    let r1 = kitty_graphics_protocol::parse(&p1, &mut state).unwrap();
    let id1 = r1.graphic_data.unwrap().id.get();

    let p2 = vec![
        b"G".as_ref(),
        b"a=t,f=32,s=1,v=1".as_ref(),
        b"AP8A/w==".as_ref(),
    ];
    let r2 = kitty_graphics_protocol::parse(&p2, &mut state).unwrap();
    let id2 = r2.graphic_data.unwrap().id.get();

    assert_ne!(
        id1, id2,
        "Two implicit-ID transmissions must get distinct allocated IDs"
    );
    assert!(id1 > 0, "Auto-assigned id must be non-zero");
    assert!(id2 > 0, "Auto-assigned id must be non-zero");
}

#[test]
fn test_implicit_image_id_still_suppresses_response() {
    // Per spec: even though we auto-assign an id internally, we must
    // not respond to commands the client transmitted *without* an
    // explicit id (otherwise the client would see a stray APC reply
    // it doesn't know how to interpret).
    let mut state = KittyGraphicsState::default();
    let params = vec![
        b"G".as_ref(),
        b"a=t,f=32,s=1,v=1".as_ref(),
        b"/wAA/w==".as_ref(),
    ];
    let resp = kitty_graphics_protocol::parse(&params, &mut state).unwrap();
    assert!(
        resp.response.is_none() || resp.response.as_deref() == Some(""),
        "Implicit-id transmissions must not produce a response"
    );
}

#[test]
fn test_explicit_image_id_still_responds() {
    // Sanity check that adding implicit-id auto-assignment didn't
    // accidentally suppress responses for explicit-id transmissions.
    let mut state = KittyGraphicsState::default();
    let params = vec![
        b"G".as_ref(),
        b"a=t,f=32,s=1,v=1,i=42".as_ref(),
        b"/wAA/w==".as_ref(),
    ];
    let resp = kitty_graphics_protocol::parse(&params, &mut state).unwrap();
    let body = resp.response.expect("explicit-id response must be present");
    assert!(body.contains("i=42"));
    assert!(body.contains("OK"));
}
