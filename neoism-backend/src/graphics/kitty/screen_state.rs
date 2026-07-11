use super::*;

// Per-screen kitty graphics state isolation.

#[test]
fn test_swap_alt_isolates_kitty_images() {
    // Per spec: each terminal screen owns its own image cache. After
    // swapping into the alt screen, main-screen images must not be
    // visible, and vice versa.
    let mut term = make_test_term();

    // Store two images on the main screen.
    store_red_pixel(&mut term, 1);
    store_red_pixel(&mut term, 2);
    assert!(term.graphics.get_kitty_image(1).is_some());
    assert!(term.graphics.get_kitty_image(2).is_some());

    // Swap to alt screen.
    term.swap_alt();

    assert!(
        term.graphics.get_kitty_image(1).is_none(),
        "Main-screen image 1 must be hidden after swapping to alt screen"
    );
    assert!(
        term.graphics.get_kitty_image(2).is_none(),
        "Main-screen image 2 must be hidden after swapping to alt screen"
    );

    // Store a different image on the alt screen.
    store_red_pixel(&mut term, 3);
    assert!(term.graphics.get_kitty_image(3).is_some());
    // The main-screen images are still hidden.
    assert!(term.graphics.get_kitty_image(1).is_none());

    // Swap back to main screen.
    term.swap_alt();

    assert!(
        term.graphics.get_kitty_image(1).is_some(),
        "Image 1 must reappear when swapping back to main screen"
    );
    assert!(term.graphics.get_kitty_image(2).is_some());
    assert!(
        term.graphics.get_kitty_image(3).is_none(),
        "Alt-screen image 3 must not leak into main screen"
    );

    // Swap back to alt — image 3 should be there again.
    term.swap_alt();
    assert!(
        term.graphics.get_kitty_image(3).is_some(),
        "Alt-screen image 3 must be preserved across screen swaps"
    );
}

#[test]
fn test_swap_alt_isolates_placements() {
    // Placements are also per-screen — putting a placement on the main
    // screen should not appear on the alt screen.
    let mut term = make_test_term();
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    store_red_pixel(&mut term, 1);
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 1,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 0,
        rows: 1,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 1,
    };
    term.place_graphic(placement);
    assert!(
        !term.graphics.kitty_placements.is_empty(),
        "Main-screen placement should be present after place_graphic"
    );

    term.swap_alt();
    assert!(
        term.graphics.kitty_placements.is_empty(),
        "Main-screen placements must not be visible on the alt screen"
    );

    term.swap_alt();
    assert!(
        !term.graphics.kitty_placements.is_empty(),
        "Main-screen placements must reappear after swapping back"
    );
}

#[test]
fn test_swap_alt_isolates_image_numbers() {
    // Image-number mappings (I=) are per-screen too.
    let mut term = make_test_term();
    let g = GraphicData {
        id: GraphicId::new(1),
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
    term.graphics.store_kitty_image(1, Some(50), g);
    assert!(term.graphics.get_kitty_image_by_number(50).is_some());

    term.swap_alt();
    assert!(
        term.graphics.get_kitty_image_by_number(50).is_none(),
        "Image-number mapping must not bleed across screens"
    );

    term.swap_alt();
    assert!(
        term.graphics.get_kitty_image_by_number(50).is_some(),
        "Image-number mapping must come back when we swap to its screen"
    );
}

#[test]
fn test_swap_alt_marks_kitty_dirty() {
    // The renderer relies on the dirty flag to know when to rebuild
    // the overlay layer; swap must set it.
    let mut term = make_test_term();
    term.graphics.kitty_graphics_dirty = false;
    term.swap_alt();
    assert!(
        term.graphics.kitty_graphics_dirty,
        "swap_alt must mark kitty graphics dirty so the renderer rebuilds"
    );
}

#[test]
fn test_full_reset_clears_both_screens() {
    // reset_state should clear images on both main and alt screens.
    let mut term = make_test_term();

    // Image on main screen.
    store_red_pixel(&mut term, 1);
    // Swap to alt and store another image.
    term.swap_alt();
    store_red_pixel(&mut term, 2);
    // Sanity: alt has image 2, not 1.
    assert!(term.graphics.get_kitty_image(2).is_some());
    assert!(term.graphics.get_kitty_image(1).is_none());

    // Full reset.
    term.reset_state();

    // Both screens should be empty.
    assert!(term.graphics.get_kitty_image(1).is_none());
    assert!(term.graphics.get_kitty_image(2).is_none());
    assert!(term.graphics.kitty_inactive_screen.kitty_images.is_empty());
}

// Eviction prefers inactive-screen images.

#[test]
fn test_eviction_prefers_inactive_screen_images() {
    use neoism_terminal_core::ansi::graphics::{Graphics, KittyScreenState, StoredImage};

    let mut graphics = Graphics {
        total_limit: 100, // tiny limit so a 60-byte add forces eviction
        ..Graphics::default()
    };

    // Active screen: image 1, 50 bytes, no placement (unused).
    let active_data = GraphicData {
        id: GraphicId::new(1),
        width: 5,
        height: 5,
        color_type: ColorType::Rgba,
        pixels: vec![1u8; 50],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(1, None, active_data);

    // Inactive screen: image 2, 50 bytes, no placement either.
    // Pre-load via the inactive_screen field directly so we don't need
    // to drive a swap.
    let inactive_data = GraphicData {
        id: GraphicId::new(2),
        width: 5,
        height: 5,
        color_type: ColorType::Rgba,
        pixels: vec![2u8; 50],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now() - std::time::Duration::from_secs(60),
    };
    graphics.kitty_inactive_screen = KittyScreenState::default();
    graphics.kitty_inactive_screen.kitty_images.insert(
        2,
        StoredImage {
            data: inactive_data,
            transmission_time: std::time::Instant::now()
                - std::time::Duration::from_secs(60),
        },
    );
    // Inactive bytes also count toward total_bytes (kept consistent).
    graphics.total_bytes += 50;

    // Now total_bytes = 100. Adding 60 more would push us to 160 > 100,
    // so eviction must free 60 bytes. The inactive image (50 bytes) is
    // tier 0 and gets evicted first; the active unused image (tier 1)
    // is the next candidate to free the remaining 10 bytes.
    let used = std::collections::HashSet::new();
    let ok = graphics.evict_images(60, &used);
    assert!(ok, "Eviction should free enough");

    assert!(
        !graphics.kitty_inactive_screen.kitty_images.contains_key(&2),
        "Inactive image should be evicted before active images"
    );
}

#[test]
fn test_eviction_keeps_active_used_image_when_inactive_available() {
    use neoism_terminal_core::ansi::graphics::{Graphics, KittyScreenState, StoredImage};

    let mut graphics = Graphics {
        total_limit: 100,
        ..Graphics::default()
    };

    // Active screen: image 1 with a *live* placement (used).
    let active = GraphicData {
        id: GraphicId::new(1),
        width: 5,
        height: 5,
        color_type: ColorType::Rgba,
        pixels: vec![1u8; 50],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(1, None, active);
    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 5, 1, 0));

    // Inactive screen: image 2 (older, unused on its screen).
    let inactive = GraphicData {
        id: GraphicId::new(2),
        width: 5,
        height: 5,
        color_type: ColorType::Rgba,
        pixels: vec![2u8; 50],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.kitty_inactive_screen = KittyScreenState::default();
    graphics.kitty_inactive_screen.kitty_images.insert(
        2,
        StoredImage {
            data: inactive,
            transmission_time: std::time::Instant::now(),
        },
    );
    graphics.total_bytes += 50;

    // active placements protect image 1.
    let mut used = std::collections::HashSet::new();
    used.insert(1u64);

    let ok = graphics.evict_images(50, &used);
    assert!(ok);

    // The active visible image must survive; the inactive image is gone.
    assert!(
        graphics.kitty_images.contains_key(&1),
        "Active visible image must not be evicted while an inactive \
         alternative exists"
    );
    assert!(
        !graphics.kitty_inactive_screen.kitty_images.contains_key(&2),
        "Inactive image should be the eviction target"
    );
}
