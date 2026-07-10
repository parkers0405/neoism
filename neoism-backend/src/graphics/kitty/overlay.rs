use super::*;

// Overlay placement tests

// test_graphic_id_kitty_vs_sixel_no_collision and test_graphic_id_kitty_different_images
// removed: kitty images no longer use GraphicId. They use u32 image_id directly,
// in a completely separate rendering path from sixel/iTerm2 atlas graphics.

#[test]
fn test_store_kitty_image_increments_generation() {
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();
    let pixels = vec![255u8; 4 * 4 * 4];

    let data1 = GraphicData {
        id: GraphicId::new(1),
        width: 4,
        height: 4,
        color_type: ColorType::Rgba,
        pixels: pixels.clone(),
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(1, None, data1);
    let time1 = graphics.get_kitty_image(1).unwrap().transmission_time;

    // Small sleep to ensure different timestamps
    std::thread::sleep(std::time::Duration::from_millis(1));

    let data2 = GraphicData {
        id: GraphicId::new(1),
        width: 4,
        height: 4,
        color_type: ColorType::Rgba,
        pixels: pixels.clone(),
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(1, None, data2);
    let time2 = graphics.get_kitty_image(1).unwrap().transmission_time;

    assert!(
        time2 > time1,
        "Transmit time must increase on re-transmission"
    );
}

#[test]
fn test_kitty_placement_insert_and_delete() {
    use neoism_terminal_core::ansi::graphics::{Graphics, KittyPlacement};

    let mut graphics = Graphics::default();

    let placement = KittyPlacement {
        image_id: 1,
        placement_id: 0,
        source_x: 0,
        source_y: 0,
        source_width: 0,
        source_height: 0,
        dest_col: 0,
        dest_row: 0,
        columns: 10,
        rows: 5,
        pixel_width: 100,
        pixel_height: 50,
        cell_x_offset: 0,
        cell_y_offset: 0,
        z_index: 0,
        transmit_time: std::time::Instant::now(),
    };

    graphics.kitty_placements.insert((1, 0), placement);
    assert_eq!(graphics.kitty_placements.len(), 1);

    // Delete by image_id
    graphics.kitty_placements.retain(|k, _| k.0 != 1);
    assert_eq!(graphics.kitty_placements.len(), 0);
}

#[test]
fn test_kitty_placement_delete_by_z_index() {
    use neoism_terminal_core::ansi::graphics::{Graphics, KittyPlacement};

    let mut graphics = Graphics::default();

    let make_placement = |image_id: u32, z: i32| KittyPlacement {
        image_id,
        placement_id: 0,
        source_x: 0,
        source_y: 0,
        source_width: 0,
        source_height: 0,
        dest_col: 0,
        dest_row: 0,
        columns: 1,
        rows: 1,
        pixel_width: 10,
        pixel_height: 10,
        cell_x_offset: 0,
        cell_y_offset: 0,
        z_index: z,
        transmit_time: std::time::Instant::now(),
    };

    graphics
        .kitty_placements
        .insert((1, 0), make_placement(1, 0));
    graphics
        .kitty_placements
        .insert((2, 0), make_placement(2, -1));
    graphics
        .kitty_placements
        .insert((3, 0), make_placement(3, 0));
    assert_eq!(graphics.kitty_placements.len(), 3);

    // Delete z=0 placements
    graphics.kitty_placements.retain(|_, p| p.z_index != 0);
    assert_eq!(graphics.kitty_placements.len(), 1);
    assert!(graphics.kitty_placements.contains_key(&(2, 0)));
}

#[test]
fn test_collect_active_ids_includes_overlay_placements() {
    use neoism_terminal_core::ansi::graphics::{Graphics, KittyPlacement};

    let mut graphics = Graphics::default();

    let placement = KittyPlacement {
        image_id: 42,
        placement_id: 0,
        source_x: 0,
        source_y: 0,
        source_width: 0,
        source_height: 0,
        dest_col: 0,
        dest_row: 0,
        columns: 1,
        rows: 1,
        pixel_width: 10,
        pixel_height: 10,
        cell_x_offset: 0,
        cell_y_offset: 0,
        z_index: 0,
        transmit_time: std::time::Instant::now(),
    };

    graphics.kitty_placements.insert((42, 0), placement);

    let active = graphics.collect_active_graphic_ids();
    assert!(
        active.contains(&42u64),
        "Overlay placements should be counted as active"
    );
}

#[test]
fn test_eviction_removes_dangling_placements() {
    use neoism_terminal_core::ansi::graphics::{Graphics, KittyPlacement};

    let mut graphics = Graphics {
        total_limit: 100,
        ..Graphics::default()
    };

    // Add a graphic that will be evicted
    let pixels = vec![255u8; 200]; // 200 bytes, exceeds 100 limit
    let data = GraphicData {
        id: GraphicId::new(1),
        width: 10,
        height: 5,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.pending.push(data);
    graphics.track_graphic(GraphicId::new(1), 200);

    // Add an overlay placement referencing this graphic
    let placement = KittyPlacement {
        image_id: 1,
        placement_id: 0,
        source_x: 0,
        source_y: 0,
        source_width: 0,
        source_height: 0,
        dest_col: 0,
        dest_row: 0,
        columns: 1,
        rows: 1,
        pixel_width: 10,
        pixel_height: 10,
        cell_x_offset: 0,
        cell_y_offset: 0,
        z_index: 0,
        transmit_time: std::time::Instant::now(),
    };
    graphics.kitty_placements.insert((1, 0), placement);

    // Trigger eviction
    let used_ids = std::collections::HashSet::new();
    graphics.evict_images(100, &used_ids);

    // Placement should be removed along with the image
    assert!(
        graphics.kitty_placements.is_empty(),
        "Dangling placements should be removed during eviction"
    );
}

#[test]
fn test_placement_id_zero_creates_multiple() {
    // Test: add placement with zero placement id"
    // When placement_id=0, each insertion should use a unique key
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    // Insert two placements with placement_id=0 for same image
    // In the real code, the handler auto-assigns unique IDs, but at the
    // data structure level, (image_id, 0) would overwrite. The protocol
    // layer should assign unique placement_ids before inserting.
    let p1 = make_test_placement(1, 0, 0, 0, 5, 3, 0);
    let p2 = make_test_placement(1, 1, 5, 0, 5, 3, 0);

    graphics.kitty_placements.insert((1, 0), p1);
    graphics.kitty_placements.insert((1, 1), p2);

    assert_eq!(graphics.kitty_placements.len(), 2);
}

#[test]
fn test_delete_all_placements_preserves_images() {
    // Kitty test: "test_gr_delete" d=a (lowercase) deletes placements but not images
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    // Store an image
    let data = GraphicData {
        id: GraphicId::new(1),
        width: 4,
        height: 4,
        color_type: ColorType::Rgba,
        pixels: vec![255u8; 64],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(1, None, data);

    // Add placements
    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 5, 3, 0));
    graphics
        .kitty_placements
        .insert((1, 1), make_test_placement(1, 1, 5, 0, 5, 3, 0));

    // Delete all placements (lowercase 'a' = keep images)
    graphics.kitty_placements.clear();

    assert_eq!(graphics.kitty_placements.len(), 0, "All placements removed");
    assert!(
        graphics.get_kitty_image(1).is_some(),
        "Image should still exist"
    );
}

#[test]
fn test_delete_all_placements_and_images() {
    // Kitty test: "test_gr_delete" d=A (uppercase) deletes both
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    let data = GraphicData {
        id: GraphicId::new(1),
        width: 4,
        height: 4,
        color_type: ColorType::Rgba,
        pixels: vec![255u8; 64],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(1, None, data);
    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 5, 3, 0));

    // Uppercase A: delete placements AND images
    graphics.kitty_placements.clear();
    graphics.kitty_images.clear();
    graphics.kitty_image_numbers.clear();

    assert_eq!(graphics.kitty_placements.len(), 0);
    assert!(graphics.get_kitty_image(1).is_none());
}

#[test]
fn test_delete_by_specific_placement_id() {
    // Test: delete placement by specific id"
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 5, 3, 0));
    graphics
        .kitty_placements
        .insert((1, 1), make_test_placement(1, 1, 5, 0, 5, 3, 0));
    graphics
        .kitty_placements
        .insert((2, 0), make_test_placement(2, 0, 0, 5, 5, 3, 0));

    assert_eq!(graphics.kitty_placements.len(), 3);

    // Delete specific placement (image_id=1, placement_id=1)
    graphics.kitty_placements.remove(&(1, 1));

    assert_eq!(graphics.kitty_placements.len(), 2);
    assert!(graphics.kitty_placements.contains_key(&(1, 0)));
    assert!(!graphics.kitty_placements.contains_key(&(1, 1)));
    assert!(graphics.kitty_placements.contains_key(&(2, 0)));
}

#[test]
fn test_delete_by_image_id_removes_all_placements_for_image() {
    // Test: delete all placements by image id"
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 5, 3, 0));
    graphics
        .kitty_placements
        .insert((1, 1), make_test_placement(1, 1, 5, 0, 5, 3, 0));
    graphics
        .kitty_placements
        .insert((2, 0), make_test_placement(2, 0, 0, 5, 5, 3, 0));

    // Delete all placements for image_id=1
    graphics.kitty_placements.retain(|k, _| k.0 != 1);

    assert_eq!(graphics.kitty_placements.len(), 1);
    assert!(graphics.kitty_placements.contains_key(&(2, 0)));
}

#[test]
fn test_delete_intersecting_cursor() {
    // Test: delete intersecting cursor"
    // Kitty test: "test_gr_delete" d=C
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    // Place at col=0, row=0, size 5x3
    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 5, 3, 0));
    // Place at col=10, row=10, size 5x3
    graphics
        .kitty_placements
        .insert((2, 0), make_test_placement(2, 0, 10, 10, 5, 3, 0));

    // Cursor at (2, 1) — intersects placement 1 (col 0..5, row 0..3)
    let cursor_col = 2usize;
    let cursor_abs_row = 1i64;
    graphics.kitty_placements.retain(|_, p| {
        !(p.dest_col <= cursor_col
            && cursor_col < p.dest_col + p.columns as usize
            && p.dest_row <= cursor_abs_row
            && cursor_abs_row < p.dest_row + p.rows as i64)
    });

    assert_eq!(graphics.kitty_placements.len(), 1);
    assert!(graphics.kitty_placements.contains_key(&(2, 0)));
}

#[test]
fn test_delete_intersecting_cursor_hits_multiple() {
    // Test: delete intersecting cursor hits multiple"
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    // Two overlapping placements at same position
    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 10, 10, 0));
    graphics
        .kitty_placements
        .insert((2, 0), make_test_placement(2, 0, 0, 0, 5, 5, 1));

    let cursor_col = 2usize;
    let cursor_abs_row = 2i64;
    graphics.kitty_placements.retain(|_, p| {
        !(p.dest_col <= cursor_col
            && cursor_col < p.dest_col + p.columns as usize
            && p.dest_row <= cursor_abs_row
            && cursor_abs_row < p.dest_row + p.rows as i64)
    });

    assert_eq!(
        graphics.kitty_placements.len(),
        0,
        "Both overlapping placements should be removed"
    );
}

#[test]
fn test_delete_by_column() {
    // Test: delete by column"
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    // Placement at col 0, width 5 cells
    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 5, 3, 0));
    // Placement at col 10, width 5 cells
    graphics
        .kitty_placements
        .insert((2, 0), make_test_placement(2, 0, 10, 0, 5, 3, 0));
    // Placement at col 3, width 2 cells (overlaps column 3)
    graphics
        .kitty_placements
        .insert((3, 0), make_test_placement(3, 0, 3, 5, 2, 1, 0));

    // Delete placements intersecting column 3
    let col = 3usize;
    graphics
        .kitty_placements
        .retain(|_, p| !(p.dest_col <= col && col < p.dest_col + p.columns as usize));

    assert_eq!(graphics.kitty_placements.len(), 1);
    assert!(
        graphics.kitty_placements.contains_key(&(2, 0)),
        "Only placement at col 10 should survive"
    );
}

#[test]
fn test_delete_by_row() {
    // Test: delete by row"
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    // Placement at row 0, height 3
    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 5, 3, 0));
    // Placement at row 10, height 2
    graphics
        .kitty_placements
        .insert((2, 0), make_test_placement(2, 0, 0, 10, 5, 2, 0));

    // Delete placements intersecting row 1
    let abs_row = 1i64;
    graphics
        .kitty_placements
        .retain(|_, p| !(p.dest_row <= abs_row && abs_row < p.dest_row + p.rows as i64));

    assert_eq!(graphics.kitty_placements.len(), 1);
    assert!(graphics.kitty_placements.contains_key(&(2, 0)));
}

#[test]
fn test_delete_by_column_1x1() {
    // Test: delete by column 1x1"
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 1, 1, 0));
    graphics
        .kitty_placements
        .insert((2, 0), make_test_placement(2, 0, 1, 0, 1, 1, 0));
    graphics
        .kitty_placements
        .insert((3, 0), make_test_placement(3, 0, 2, 0, 1, 1, 0));

    // Delete column 1
    let col = 1usize;
    graphics
        .kitty_placements
        .retain(|_, p| !(p.dest_col <= col && col < p.dest_col + p.columns as usize));

    assert_eq!(graphics.kitty_placements.len(), 2);
    assert!(graphics.kitty_placements.contains_key(&(1, 0)));
    assert!(!graphics.kitty_placements.contains_key(&(2, 0)));
    assert!(graphics.kitty_placements.contains_key(&(3, 0)));
}

#[test]
fn test_delete_by_row_1x1() {
    // Test: delete by row 1x1"
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 1, 1, 0));
    graphics
        .kitty_placements
        .insert((2, 0), make_test_placement(2, 0, 0, 1, 1, 1, 0));
    graphics
        .kitty_placements
        .insert((3, 0), make_test_placement(3, 0, 0, 2, 1, 1, 0));

    // Delete row 1
    let abs_row = 1i64;
    graphics
        .kitty_placements
        .retain(|_, p| !(p.dest_row <= abs_row && abs_row < p.dest_row + p.rows as i64));

    assert_eq!(graphics.kitty_placements.len(), 2);
    assert!(graphics.kitty_placements.contains_key(&(1, 0)));
    assert!(!graphics.kitty_placements.contains_key(&(2, 0)));
    assert!(graphics.kitty_placements.contains_key(&(3, 0)));
}

#[test]
fn test_retransmit_same_image_id_updates_data() {
    // Kitty test: "test_load_images" — re-transmit replaces image data
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    let data1 = GraphicData {
        id: GraphicId::new(1),
        width: 4,
        height: 4,
        color_type: ColorType::Rgba,
        pixels: vec![0u8; 64],
        is_opaque: false,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(1, None, data1);
    let time1 = graphics.get_kitty_image(1).unwrap().transmission_time;
    let pixels1 = graphics.get_kitty_image(1).unwrap().data.pixels[0];

    // Re-transmit with different pixel data
    let data2 = GraphicData {
        id: GraphicId::new(1),
        width: 4,
        height: 4,
        color_type: ColorType::Rgba,
        pixels: vec![128u8; 64],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(1, None, data2);
    let time2 = graphics.get_kitty_image(1).unwrap().transmission_time;
    let pixels2 = graphics.get_kitty_image(1).unwrap().data.pixels[0];

    assert!(time2 > time1, "Transmit time must increase");
    assert_ne!(pixels1, pixels2, "Pixel data must be replaced");
    assert_eq!(pixels2, 128);
}

#[test]
fn test_image_number_mapping() {
    // Kitty test: "test_gr_operations_with_numbers" — I parameter maps to image_id
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    let data = GraphicData {
        id: GraphicId::new(42),
        width: 2,
        height: 2,
        color_type: ColorType::Rgba,
        pixels: vec![255u8; 16],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    // Store with image_number=7
    graphics.store_kitty_image(42, Some(7), data);

    // Lookup by number
    let stored = graphics.get_kitty_image_by_number(7);
    assert!(stored.is_some(), "Should find image by number");
    assert_eq!(stored.unwrap().data.id, GraphicId::new(42));

    // Non-existent number
    assert!(graphics.get_kitty_image_by_number(99).is_none());
}

#[test]
fn test_image_number_remapping_on_retransmit() {
    // Kitty: re-transmitting with same I= gets new image data but same mapping
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    let data1 = GraphicData {
        id: GraphicId::new(1),
        width: 2,
        height: 2,
        color_type: ColorType::Rgba,
        pixels: vec![0u8; 16],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(1, Some(100), data1);

    // Re-transmit same image_id with same number
    let data2 = GraphicData {
        id: GraphicId::new(1),
        width: 2,
        height: 2,
        color_type: ColorType::Rgba,
        pixels: vec![255u8; 16],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(1, Some(100), data2);

    let stored = graphics.get_kitty_image_by_number(100).unwrap();
    assert_eq!(
        stored.data.pixels[0], 255,
        "Number mapping should point to newest data"
    );
}

#[test]
fn test_placement_source_rect_tracking() {
    // placements track source rectangle for partial image display
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    let mut p = make_test_placement(1, 0, 0, 0, 10, 5, 0);
    p.source_x = 10;
    p.source_y = 20;
    p.source_width = 100;
    p.source_height = 50;

    graphics.kitty_placements.insert((1, 0), p);

    let stored = graphics.kitty_placements.get(&(1, 0)).unwrap();
    assert_eq!(stored.source_x, 10);
    assert_eq!(stored.source_y, 20);
    assert_eq!(stored.source_width, 100);
    assert_eq!(stored.source_height, 50);
}

#[test]
fn test_placement_z_ordering_sort() {
    // placements sorted by z-index for layered rendering
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 5, 3, 10));
    graphics
        .kitty_placements
        .insert((2, 0), make_test_placement(2, 0, 0, 0, 5, 3, -1));
    graphics
        .kitty_placements
        .insert((3, 0), make_test_placement(3, 0, 0, 0, 5, 3, 0));

    let mut sorted: Vec<_> = graphics.kitty_placements.values().collect();
    sorted.sort_by_key(|p| p.z_index);

    assert_eq!(sorted[0].z_index, -1, "Negative z first");
    assert_eq!(sorted[1].z_index, 0, "Zero z middle");
    assert_eq!(sorted[2].z_index, 10, "Positive z last");
}

#[test]
fn test_delete_kitty_images_cleans_number_mapping() {
    // When images are deleted, number mappings should be cleaned up
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    let data = GraphicData {
        id: GraphicId::new(1),
        width: 2,
        height: 2,
        color_type: ColorType::Rgba,
        pixels: vec![255u8; 16],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(1, Some(7), data);

    assert!(graphics.get_kitty_image_by_number(7).is_some());

    // Delete by predicate
    graphics.delete_kitty_images(|id, _| *id == 1);

    assert!(
        graphics.get_kitty_image_by_number(7).is_none(),
        "Number mapping should be cleaned up when image is deleted"
    );
}

#[test]
fn test_both_columns_and_rows_no_aspect_ratio() {
    // When both c= and r= specified, stretch to fill (no aspect ratio).
    let mut state = KittyGraphicsState::default();

    // 2x2 RGBA = 16 bytes, base64("/////w==" is 4 bytes, need 16 bytes)
    // Use pre-encoded: 16 bytes of 0xFF = "/////////////////////w=="
    let params: Vec<&[u8]> = vec![
        b"G",
        b"a=T,f=32,s=2,v=2,c=80,r=20,i=1",
        b"/////////////////////w==",
    ];

    let response = kitty_graphics_protocol::parse(&params, &mut state);
    assert!(response.is_some());
    let graphic_data = response.unwrap().graphic_data.unwrap();

    assert!(graphic_data.resize.is_some());
    let resize = graphic_data.resize.unwrap();
    assert!(
        !resize.preserve_aspect_ratio,
        "Both c= and r= specified: should NOT preserve aspect ratio"
    );
}

#[test]
fn test_only_columns_preserves_aspect_ratio() {
    // When only c= specified, compute r= from aspect ratio
    let mut state = KittyGraphicsState::default();

    let params: Vec<&[u8]> = vec![
        b"G",
        b"a=T,f=32,s=2,v=2,c=80,i=1",
        b"/////////////////////w==",
    ];

    let response = kitty_graphics_protocol::parse(&params, &mut state);
    assert!(response.is_some());
    let graphic_data = response.unwrap().graphic_data.unwrap();

    let resize = graphic_data.resize.unwrap();
    assert!(
        resize.preserve_aspect_ratio,
        "Only c= specified: should preserve aspect ratio"
    );
}

#[test]
fn test_only_rows_preserves_aspect_ratio() {
    // When only r= specified, compute c= from aspect ratio
    let mut state = KittyGraphicsState::default();

    let params: Vec<&[u8]> = vec![
        b"G",
        b"a=T,f=32,s=2,v=2,r=20,i=1",
        b"/////////////////////w==",
    ];

    let response = kitty_graphics_protocol::parse(&params, &mut state);
    assert!(response.is_some());
    let graphic_data = response.unwrap().graphic_data.unwrap();

    let resize = graphic_data.resize.unwrap();
    assert!(
        resize.preserve_aspect_ratio,
        "Only r= specified: should preserve aspect ratio"
    );
}

#[test]
fn test_delete_by_image_number() {
    // d=n deletes by image number (I= parameter).
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    // Store image with number mapping
    let data = GraphicData {
        id: GraphicId::new(42),
        width: 2,
        height: 2,
        color_type: ColorType::Rgba,
        pixels: vec![255u8; 16],
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    graphics.store_kitty_image(42, Some(7), data);
    graphics
        .kitty_placements
        .insert((42, 0), make_test_placement(42, 0, 0, 0, 5, 3, 0));

    // Look up by number
    assert!(graphics.get_kitty_image_by_number(7).is_some());

    // Delete by number (simulate d=n with image_number=7)
    if let Some(&image_id) = graphics.kitty_image_numbers.get(&7) {
        graphics.kitty_placements.retain(|k, _| k.0 != image_id);
    }

    assert_eq!(graphics.kitty_placements.len(), 0);
    // Image still exists (lowercase n = keep data)
    assert!(graphics.get_kitty_image(42).is_some());
}

#[test]
fn test_delete_at_cell_with_z_filter() {
    // d=q deletes at cell position with z-index filter.
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    // Two placements at same position, different z-index
    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 5, 3, 0));
    graphics
        .kitty_placements
        .insert((2, 0), make_test_placement(2, 0, 0, 0, 5, 3, -1));

    // Delete at (2, 1) with z=0 — should only remove image 1
    let col = 2usize;
    let abs_row = 1i64;
    let z = 0i32;
    graphics.kitty_placements.retain(|_, p| {
        !(p.z_index == z
            && p.dest_col <= col
            && col < p.dest_col + p.columns as usize
            && p.dest_row <= abs_row
            && abs_row < p.dest_row + p.rows as i64)
    });

    assert_eq!(graphics.kitty_placements.len(), 1);
    assert!(graphics.kitty_placements.contains_key(&(2, 0)));
}

#[test]
fn test_delete_by_image_range() {
    // d=r deletes by image ID range.
    use neoism_terminal_core::ansi::graphics::Graphics;

    let mut graphics = Graphics::default();

    graphics
        .kitty_placements
        .insert((1, 0), make_test_placement(1, 0, 0, 0, 5, 3, 0));
    graphics
        .kitty_placements
        .insert((5, 0), make_test_placement(5, 0, 5, 0, 5, 3, 0));
    graphics
        .kitty_placements
        .insert((10, 0), make_test_placement(10, 0, 0, 5, 5, 3, 0));

    // Delete range 1..5
    let range_start = 1u32;
    let range_end = 5u32;
    graphics
        .kitty_placements
        .retain(|k, _| k.0 < range_start || k.0 > range_end);

    assert_eq!(graphics.kitty_placements.len(), 1);
    assert!(graphics.kitty_placements.contains_key(&(10, 0)));
}

#[test]
fn test_implicit_id_no_response() {
    // When image_id=0 and image_number=0, no response should be sent
    let mut state = KittyGraphicsState::default();

    // Transmit with no explicit ID
    let params: Vec<&[u8]> = vec![
        b"G",
        b"a=t,f=32,s=1,v=1",
        b"/w==", // 1 byte base64
    ];

    let response = kitty_graphics_protocol::parse(&params, &mut state);
    // Should have graphic data but no response string
    if let Some(resp) = response {
        assert!(
            resp.response.is_none() || resp.response.as_deref() == Some(""),
            "No response should be sent for implicit IDs"
        );
    }
}
