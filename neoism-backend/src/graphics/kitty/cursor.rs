use super::*;

// Cursor Movement Tests

#[test]
fn test_cursor_movement_default() {
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(80, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );

    let initial_cursor_row = term.grid.cursor.pos.row.0;

    // Set proper cell dimensions for testing
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    // Create a 100x100 pixel image (will be resized to fit 2 rows)
    let pixels = vec![255u8; 100 * 100 * 4];
    let graphic = GraphicData {
        id: GraphicId::new(1),
        width: 100,
        height: 100,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque: true,
        resize: Some(ResizeCommand {
            width: ResizeParameter::Auto,
            height: ResizeParameter::Cells(2),
            preserve_aspect_ratio: true,
        }),
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };

    term.store_graphic(graphic);

    // Place with cursor_movement=0 (move cursor to after image)
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 1,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 0,
        rows: 2,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 0,
    };

    term.place_graphic(placement);

    let final_cursor_row = term.grid.cursor.pos.row.0;
    let final_cursor_col = term.grid.cursor.pos.col.0;

    // With cursor_movement=0 (Kitty default), cursor stays ON last row of image
    // For a 2-row image starting at row 0 (occupies rows 0-1), cursor should be at row 1, col 0
    assert_eq!(
        final_cursor_row, 1,
        "Cursor should be at row 1 (last row of image) with cursor_movement=0. Initial: {}, Final: {}",
        initial_cursor_row,
        final_cursor_row
    );
    assert_eq!(
        final_cursor_col, 0,
        "Cursor should be at column 0 after carriage return"
    );
}

#[test]
fn test_cursor_movement_no_move() {
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(80, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );

    // Set proper cell dimensions for testing
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    // Start at a specific position
    term.grid.cursor.pos.row.0 = 5;
    term.grid.cursor.pos.col.0 = 10;

    // Create a 100x100 pixel image
    let pixels = vec![255u8; 100 * 100 * 4];
    let graphic = GraphicData {
        id: GraphicId::new(2),
        width: 100,
        height: 100,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque: true,
        resize: Some(ResizeCommand {
            width: ResizeParameter::Auto,
            height: ResizeParameter::Cells(2),
            preserve_aspect_ratio: true,
        }),
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };

    term.store_graphic(graphic);

    // Place with cursor_movement=1 (don't move cursor)
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 2,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 0,
        rows: 2,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 1, // Don't move cursor
    };

    term.place_graphic(placement);

    // With cursor_movement=1, cursor behavior depends on placement x,y
    // This test verifies the no-move code path executes without panic
}

#[test]
fn test_protocol_parses_cursor_movement() {
    let mut state = KittyGraphicsState::default();

    // Test that C=0 is parsed
    let result = kitty_graphics_protocol::parse(&[b"G", b"a=p,i=1,C=0", b""], &mut state);
    assert!(result.is_some());
    let response = result.unwrap();
    assert!(response.placement_request.is_some());
    let placement = response.placement_request.unwrap();
    assert_eq!(
        placement.cursor_movement, 0,
        "C=0 should parse as cursor_movement=0"
    );

    // Test that C=1 is parsed
    let result = kitty_graphics_protocol::parse(&[b"G", b"a=p,i=1,C=1", b""], &mut state);
    assert!(result.is_some());
    let response = result.unwrap();
    assert!(response.placement_request.is_some());
    let placement = response.placement_request.unwrap();
    assert_eq!(
        placement.cursor_movement, 1,
        "C=1 should parse as cursor_movement=1"
    );

    // Test default (no C key)
    let result = kitty_graphics_protocol::parse(&[b"G", b"a=p,i=1", b""], &mut state);
    assert!(result.is_some());
    let response = result.unwrap();
    assert!(response.placement_request.is_some());
    let placement = response.placement_request.unwrap();
    assert_eq!(
        placement.cursor_movement, 0,
        "Default should be cursor_movement=0"
    );
}

// Row Calculation Tests

#[test]
fn test_image_row_occupation_exact_fit() {
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(80, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );

    // Start at row 0
    let initial_cursor_row = term.grid.cursor.pos.row.0;
    assert_eq!(initial_cursor_row, 0, "Cursor should start at row 0");

    // Set proper cell dimensions for testing
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    // Create a 100x100 pixel image (will be resized to fit 2 rows)
    let pixels = vec![255u8; 100 * 100 * 4];
    let graphic = GraphicData {
        id: GraphicId::new(1),
        width: 100,
        height: 100,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque: true,
        resize: Some(ResizeCommand {
            width: ResizeParameter::Auto,
            height: ResizeParameter::Cells(2),
            preserve_aspect_ratio: true,
        }),
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };

    term.store_graphic(graphic);

    // Place it with rows=2 (should occupy exactly 2 rows)
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 1,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 0,
        rows: 2,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 0,
    };

    term.place_graphic(placement);

    let final_cursor_row = term.grid.cursor.pos.row.0;

    // With fix: cursor stays ON last row of image (row 1)
    assert_eq!(
        final_cursor_row, 1,
        "Cursor should be at row 1 (last row of image) after placing a 2-row image, but got row {}",
        final_cursor_row
    );
}

#[test]
fn test_image_row_occupation_single_row() {
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(80, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );

    // Set proper cell dimensions for testing
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    let _initial_cursor_row = term.grid.cursor.pos.row.0;

    // Create a small image that fits in 1 row
    let pixels = vec![255u8; 50 * 20 * 4];
    let graphic = GraphicData {
        id: GraphicId::new(2),
        width: 50,
        height: 20,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque: true,
        resize: Some(ResizeCommand {
            width: ResizeParameter::Auto,
            height: ResizeParameter::Cells(1),
            preserve_aspect_ratio: true,
        }),
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };

    term.store_graphic(graphic);

    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 2,
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
        cursor_movement: 0,
    };

    term.place_graphic(placement);

    let final_cursor_row = term.grid.cursor.pos.row.0;

    // With fix: cursor stays ON last row of image (row 0)
    assert_eq!(
        final_cursor_row, 0,
        "Cursor should be at row 0 (last row of image) after placing a 1-row image, but got row {}",
        final_cursor_row
    );
}

#[test]
fn test_image_row_occupation_three_rows() {
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(80, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );

    let initial_cursor_row = term.grid.cursor.pos.row.0;

    // Set proper cell dimensions for testing
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    let pixels = vec![255u8; 100 * 150 * 4];
    let graphic = GraphicData {
        id: GraphicId::new(3),
        width: 100,
        height: 150,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque: true,
        resize: Some(ResizeCommand {
            width: ResizeParameter::Auto,
            height: ResizeParameter::Cells(3),
            preserve_aspect_ratio: true,
        }),
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };

    term.store_graphic(graphic);

    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 3,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 0,
        rows: 3,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 0,
    };

    term.place_graphic(placement);

    let final_cursor_row = term.grid.cursor.pos.row.0;

    // With fix: cursor stays ON last row of image (row 2)
    assert_eq!(
        final_cursor_row, 2,
        "Cursor should be at row 2 (last row of image) after placing a 3-row image, but got row {}. \
         Delta from start: {} (expected: 2)",
        final_cursor_row,
        final_cursor_row - initial_cursor_row
    );
}

#[test]
fn test_image_row_occupation_from_middle() {
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(80, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );

    // Move cursor to row 5
    term.grid.cursor.pos.row.0 = 5;
    let initial_cursor_row = term.grid.cursor.pos.row.0;
    assert_eq!(initial_cursor_row, 5);

    // Set proper cell dimensions for testing
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    let pixels = vec![255u8; 100 * 100 * 4];
    let graphic = GraphicData {
        id: GraphicId::new(4),
        width: 100,
        height: 100,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque: true,
        resize: Some(ResizeCommand {
            width: ResizeParameter::Auto,
            height: ResizeParameter::Cells(2),
            preserve_aspect_ratio: true,
        }),
        display_width: None,
        display_height: None,
        transmit_time: std::time::Instant::now(),
    };

    term.store_graphic(graphic);

    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 4,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 0,
        rows: 2,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 0,
    };

    term.place_graphic(placement);

    let final_cursor_row = term.grid.cursor.pos.row.0;

    // With fix: cursor stays ON last row of image (row 6)
    assert_eq!(
        final_cursor_row, 6,
        "Cursor should be at row 6 (last row of image) after placing a 2-row image from row 5, but got row {}",
        final_cursor_row
    );
}
