use super::*;

// Delete Tests

#[test]
fn test_delete_all() {
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(80, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );

    // Delete all graphics (d=a)
    let delete = DeleteRequest {
        action: b'a',
        image_id: 0,
        image_number: 0,
        placement_id: 0,
        x: 0,
        y: 0,
        z_index: 0,
        delete_data: false,
    };

    // Should not panic
    term.delete_graphics(delete);
}

// Placement Management Tests

#[test]
fn test_store_graphic() {
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(80, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );

    let pixels = vec![255u8, 0, 0, 255]; // 1x1 red pixel
    let graphic = GraphicData {
        id: GraphicId::new(100),
        width: 1,
        height: 1,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };

    // Store without displaying
    term.store_graphic(graphic);

    // Verify image is in cache
    let stored = term.graphics.get_kitty_image(100);
    assert!(stored.is_some(), "Image should be stored in cache");
    assert_eq!(stored.unwrap().data.width, 1);
}

#[test]
fn test_place_nonexistent_graphic() {
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(80, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );

    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 999, // Doesn't exist
        placement_id: 0,
        x: 5,
        y: 3,
        width: 0,
        height: 0,
        columns: 2,
        rows: 2,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 0,
    };

    // Should not panic, just warn
    term.place_graphic(placement);
}

// test_delete_by_kitty_image_id and test_delete_by_image_id_does_not_delete_wrong_id
// were removed: kitty images no longer go into grid cells (overlay path only).
// Equivalent tests exist as test_delete_by_image_id_removes_all_placements_for_image
// and test_delete_by_specific_placement_id.

#[test]
fn test_no_double_push_on_graphic_cell_drop() {
    use neoism_terminal_core::ansi::graphics::{GraphicCell, TextureRef};
    use parking_lot::Mutex;
    use std::sync::Arc;

    let texture_ops: Arc<Mutex<Vec<GraphicId>>> = Arc::new(Mutex::new(Vec::new()));

    let texture = Arc::new(TextureRef {
        id: GraphicId::new(99),
        width: 10,
        height: 20,
        cell_height: 20,
        texture_operations: Arc::downgrade(&texture_ops),
    });

    // Create two GraphicCells referencing the same texture (simulating multi-cell image)
    let cell1 = GraphicCell {
        texture: texture.clone(),
        offset_x: 0,
        offset_y: 0,
    };
    let cell2 = GraphicCell {
        texture: texture.clone(),
        offset_x: 10,
        offset_y: 0,
    };

    // Drop both cells — should NOT push to texture_operations (GraphicCell has no Drop impl)
    drop(cell1);
    drop(cell2);
    assert!(
        texture_ops.lock().is_empty(),
        "GraphicCell drop should NOT push to texture_operations"
    );

    // Drop the last Arc<TextureRef> — should push exactly once
    drop(texture);
    let ops = texture_ops.lock();
    assert_eq!(
        ops.len(),
        1,
        "TextureRef drop should push exactly once, got {}",
        ops.len()
    );
    assert_eq!(ops[0], GraphicId::new(99));
}

#[test]
fn test_placed_textures_tracks_inserts() {
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(80, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );

    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    assert!(
        term.graphics.placed_textures.is_empty(),
        "Should start with no placed textures"
    );

    // Insert a graphic
    let pixels = vec![255u8; 10 * 20 * 4];
    let graphic = GraphicData {
        id: GraphicId::new(1),
        width: 10,
        height: 20,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque: true,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };
    term.insert_graphic(graphic, None, Some(0));

    assert_eq!(
        term.graphics.placed_textures.len(),
        1,
        "Should track 1 placed texture after insert"
    );
}

#[test]
fn test_collect_active_ids_uses_weak_refs() {
    use neoism_terminal_core::ansi::graphics::TextureRef;
    use std::sync::Arc;

    let mut graphics = neoism_terminal_core::ansi::graphics::Graphics::default();

    // Simulate placing a texture
    let texture_ops = graphics.texture_operations.clone();
    let texture = Arc::new(TextureRef {
        id: GraphicId::new(1),
        width: 10,
        height: 20,
        cell_height: 20,
        texture_operations: Arc::downgrade(&texture_ops),
    });
    graphics.register_placed_texture(GraphicId::new(1), Arc::downgrade(&texture));

    // While texture is alive, it should appear in active IDs
    let active = graphics.collect_active_graphic_ids();
    assert!(
        active.contains(&1),
        "Active texture should appear in collect_active_graphic_ids"
    );

    // Drop the texture — weak ref becomes dead
    drop(texture);

    // Now it should be cleaned up
    let active = graphics.collect_active_graphic_ids();
    assert!(
        !active.contains(&1),
        "Dropped texture should NOT appear in collect_active_graphic_ids"
    );
    assert!(
        graphics.placed_textures.is_empty(),
        "Stale entry should be cleaned up"
    );
}
