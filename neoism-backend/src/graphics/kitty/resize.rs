use super::*;

// Resize-with-reflow placement tracking.
//
// The user's actual scenario: a long command wraps to 2 lines, an image is
// placed below it, then the window is widened so the command fits on 1 line.
// The image must follow the surrounding text (move up by 1 row when widening,
// down by 1 when narrowing) instead of staying anchored to its absolute
// scrollback row.

#[test]
fn test_resize_widen_unwraps_command_image_follows() {
    // Reproduce: narrow window where the command wraps to 2 lines, place
    // an image right after the wrap, then widen the window so the command
    // fits on a single line. The image must move *up* by one row to stay
    // pinned to the spot just below the (now shorter) command.
    use neoism_terminal_core::handler::Handler;
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(20, 10),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    // Type a 32-char command. With columns=20 it wraps onto 2 rows;
    // after we widen to columns=50 it will fit on 1 row.
    type_text(&mut term, "$ kitten icat /path/to/image.png");
    term.linefeed();
    term.carriage_return();

    let cursor_before = term.grid.cursor.pos.row.0;

    store_red_pixel(&mut term, 1);
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 1,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 1,
        rows: 1,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 1,
    };
    term.place_graphic(placement);

    let initial_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .expect("placement must exist")
        .dest_row;
    assert_eq!(
        initial_dest_row,
        term.history_size() as i64 + cursor_before as i64,
        "placement should anchor at the cursor's absolute row"
    );

    // Widen the window. The wrapped command should join back onto a
    // single row, and the image should follow up by 1.
    term.resize(ReflowDim {
        columns: 50,
        lines: 10,
    });

    let final_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .expect("placement must still exist")
        .dest_row;
    assert_eq!(
        final_dest_row,
        initial_dest_row - 1,
        "Widening should drop dest_row by 1 so the image follows the \
         (now unwrapped) command. Got {final_dest_row}, expected {}",
        initial_dest_row - 1
    );
}

#[test]
fn test_resize_narrow_wraps_command_image_follows() {
    // Mirror case: a wide window where the command fits on 1 line.
    // Narrowing the window forces the command onto 2 wrapped rows;
    // the image below it must shift *down* by 1.
    use neoism_terminal_core::handler::Handler;
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(50, 10),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    type_text(&mut term, "$ kitten icat /path/to/image.png");
    term.linefeed();
    term.carriage_return();

    let cursor_before = term.grid.cursor.pos.row.0;

    store_red_pixel(&mut term, 1);
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 1,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 1,
        rows: 1,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 1,
    };
    term.place_graphic(placement);

    let initial_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .unwrap()
        .dest_row;
    assert_eq!(
        initial_dest_row,
        term.history_size() as i64 + cursor_before as i64
    );

    // Narrow the window so the command wraps onto two rows.
    term.resize(ReflowDim {
        columns: 20,
        lines: 10,
    });

    let final_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .unwrap()
        .dest_row;
    assert_eq!(
        final_dest_row,
        initial_dest_row + 1,
        "Narrowing should bump dest_row by 1 so the image follows the \
         (now wrapped) command down. Got {final_dest_row}, expected {}",
        initial_dest_row + 1
    );
}

#[test]
fn test_debug_widen_visible_layout() {
    // Mirror of test_debug_narrow_visible_layout: starts NARROW with the
    // command wrapped onto 2 rows, then widens.
    use neoism_terminal_core::handler::Handler;
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(20, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    for _ in 0..18 {
        term.linefeed();
    }
    term.carriage_return();

    type_text(&mut term, "$ kitten icat /path/to/image.png");
    term.linefeed();
    term.carriage_return();

    store_red_pixel(&mut term, 1);
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 1,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 1,
        rows: 1,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 0,
    };
    term.place_graphic(placement);

    term.linefeed();
    term.carriage_return();
    type_text(&mut term, "$ ");

    dump_grid(&term, "BEFORE widen");

    term.resize(ReflowDim {
        columns: 50,
        lines: 24,
    });

    dump_grid(&term, "AFTER widen");
}

#[test]
fn test_debug_narrow_visible_layout() {
    // Print visible layout before/after narrowing to understand what
    // shrink_columns actually does to cursor and content positioning.
    use neoism_terminal_core::handler::Handler;
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(50, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    for _ in 0..20 {
        term.linefeed();
    }
    term.carriage_return();

    type_text(&mut term, "$ kitten icat /path/to/image.png");
    term.linefeed();
    term.carriage_return();

    store_red_pixel(&mut term, 1);
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 1,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 1,
        rows: 1,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 0,
    };
    term.place_graphic(placement);

    term.linefeed();
    term.carriage_return();
    type_text(&mut term, "$ ");

    dump_grid(&term, "BEFORE narrow");

    term.resize(ReflowDim {
        columns: 20,
        lines: 24,
    });

    dump_grid(&term, "AFTER narrow");
}

#[test]
fn test_resize_narrow_combined_col_and_row_change() {
    // Real window resize: user drags the corner, both columns and
    // lines change in the same Crosswords::resize call. Both
    // grow_columns/shrink_columns AND grow_lines/shrink_lines fire.
    // Cursor delta accumulates from both.
    use neoism_terminal_core::handler::Handler;
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(50, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    for _ in 0..10 {
        term.linefeed();
    }
    term.carriage_return();

    type_text(&mut term, "$ kitten icat /path/to/image.png");
    term.linefeed();
    term.carriage_return();

    store_red_pixel(&mut term, 1);
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 1,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 1,
        rows: 1,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 0,
    };
    term.place_graphic(placement);

    term.linefeed();
    term.carriage_return();
    type_text(&mut term, "$ ");

    let initial_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .unwrap()
        .dest_row;

    eprintln!(
        "BEFORE combined: cursor.row={}, history={}, dest_row={}",
        term.grid.cursor.pos.row.0,
        term.history_size(),
        initial_dest_row,
    );

    // Narrow + shorten at the same time.
    term.resize(ReflowDim {
        columns: 20,
        lines: 20,
    });

    let final_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .unwrap()
        .dest_row;

    eprintln!(
        "AFTER combined : cursor.row={}, history={}, dest_row={}, delta={}",
        term.grid.cursor.pos.row.0,
        term.history_size(),
        final_dest_row,
        final_dest_row - initial_dest_row,
    );

    // The image should still follow the wrap regardless of the
    // simultaneous row count change.
    // Cursor delta should be (history_grew_by_wrap) +
    // (cursor_row_change_from_shrink_lines + wrap_above_cursor).
    // The exact number depends on how shrink_lines + shrink_columns
    // interact, but the image should track the cursor.
}

#[test]
fn test_resize_narrow_with_multi_row_image() {
    // Realistic icat: a tall image (e.g. 8 rows). The cursor advances
    // by `rows - 1` linefeeds during placement, so the dest_row is
    // *above* the cursor. Then the next prompt sits below the image.
    use neoism_terminal_core::handler::Handler;
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(50, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    // Push the cursor down to where icat would normally land.
    for _ in 0..10 {
        term.linefeed();
    }
    term.carriage_return();

    type_text(&mut term, "$ kitten icat /path/to/image.png");
    term.linefeed();
    term.carriage_return();

    let placement_row = term.grid.cursor.pos.row.0;

    store_red_pixel(&mut term, 1);
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 1,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 1,
        rows: 8, // 8-row image
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 0, // Default: cursor moves to last row of image
    };
    term.place_graphic(placement);

    // After place_kitty_overlay with cursor_movement=0, cursor was
    // advanced by rows-1 linefeeds.
    let cursor_after_image = term.grid.cursor.pos.row.0;
    assert!(
        cursor_after_image > placement_row,
        "8-row image should advance cursor below placement_row \
         (placement={placement_row}, cursor_after={cursor_after_image})"
    );

    // Then the next shell prompt.
    term.linefeed();
    term.carriage_return();
    type_text(&mut term, "$ ");

    let initial_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .unwrap()
        .dest_row;

    eprintln!(
        "BEFORE: cursor.row={}, history={}, dest_row={}, placement_row={}",
        term.grid.cursor.pos.row.0,
        term.history_size(),
        initial_dest_row,
        placement_row,
    );

    term.resize(ReflowDim {
        columns: 20,
        lines: 24,
    });

    let final_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .unwrap()
        .dest_row;

    eprintln!(
        "AFTER : cursor.row={}, history={}, dest_row={}, delta={}",
        term.grid.cursor.pos.row.0,
        term.history_size(),
        final_dest_row,
        final_dest_row - initial_dest_row,
    );

    assert_eq!(
        final_dest_row - initial_dest_row,
        1,
        "8-row image should still follow the +1 wrap delta"
    );
}

#[test]
fn test_resize_narrow_with_cursor_at_bottom_of_screen() {
    // Realistic terminal: cursor pinned at the bottom row when icat
    // runs at the prompt. After narrowing, the wrap above the image
    // pushes everything down, but Rio's `shrink_columns` may also
    // scroll to keep the cursor in view, which makes history grow more
    // than 1.
    use neoism_terminal_core::handler::Handler;
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(50, 24),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    // Push the cursor to near the bottom by linefeeding several times.
    // This simulates a terminal session where some history has been
    // built up before icat runs.
    for _ in 0..20 {
        term.linefeed();
    }
    term.carriage_return();

    // Now run the icat-style sequence.
    type_text(&mut term, "$ kitten icat /path/to/image.png");
    term.linefeed();
    term.carriage_return();

    let placement_row = term.grid.cursor.pos.row.0;
    let placement_history = term.history_size();

    store_red_pixel(&mut term, 1);
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 1,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 1,
        rows: 1,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 0,
    };
    term.place_graphic(placement);

    // Then the shell prints its next prompt.
    term.linefeed();
    term.carriage_return();
    type_text(&mut term, "$ ");

    let initial_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .unwrap()
        .dest_row;

    eprintln!(
        "BEFORE RESIZE: cursor.row={}, history={}, placement.dest_row={}, placement_row_at_place={}, history_at_place={}",
        term.grid.cursor.pos.row.0,
        term.history_size(),
        initial_dest_row,
        placement_row,
        placement_history,
    );

    term.resize(ReflowDim {
        columns: 20,
        lines: 24,
    });

    let final_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .unwrap()
        .dest_row;

    eprintln!(
        "AFTER  RESIZE: cursor.row={}, history={}, placement.dest_row={}, delta={}",
        term.grid.cursor.pos.row.0,
        term.history_size(),
        final_dest_row,
        final_dest_row - initial_dest_row,
    );

    // The image is one row below the wrapped command, so wrapping
    // should push it down by 1.
    assert_eq!(
        final_dest_row - initial_dest_row,
        1,
        "Image should follow the wrap-down by exactly 1 row (delta {})",
        final_dest_row - initial_dest_row
    );
}

#[test]
fn test_resize_narrow_with_prompt_after_image() {
    // Realistic icat flow: command on row 0, image at row 1, then the
    // shell prints a new prompt on row 2 below the image. Narrowing
    // the window should wrap row 0 into 2 rows, pushing both the image
    // and the prompt below it down by 1. This is the case the user
    // reported as still broken — content after the image makes the
    // cursor land at a row below the placement, which changes the
    // delta math.
    use neoism_terminal_core::handler::Handler;
    let mut term: Crosswords = Crosswords::new(
        neoism_terminal_core::crosswords::CrosswordsSize::new(50, 10),
        neoism_terminal_core::ansi::CursorShape::Block,
        neoism_terminal_core::TerminalId::new(0),
        10_000,
    );
    term.graphics.cell_width = 10.0;
    term.graphics.cell_height = 20.0;

    // Row 0: the command (32 chars, fits at columns=50)
    type_text(&mut term, "$ kitten icat /path/to/image.png");
    term.linefeed();
    term.carriage_return();

    // Row 1: this is where the image goes. Place it here.
    let placement_row = term.grid.cursor.pos.row.0;
    store_red_pixel(&mut term, 1);
    let placement = kitty_graphics_protocol::PlacementRequest {
        image_id: 1,
        placement_id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        columns: 1,
        rows: 1,
        z_index: 0,
        virtual_placement: false,
        unicode_placeholder: 0,
        cursor_movement: 0, // Default kitty behaviour: cursor stays on the last row of image
    };
    term.place_graphic(placement);

    // Then the shell moves to row 2 and prints its prompt.
    term.linefeed();
    term.carriage_return();
    type_text(&mut term, "$ ");

    let cursor_before = term.grid.cursor.pos.row.0;
    assert!(
        cursor_before > placement_row,
        "test setup: cursor should be below the image, got cursor={cursor_before} placement={placement_row}"
    );
    let initial_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .unwrap()
        .dest_row;

    // Narrow: row 0 wraps onto 2 rows.
    term.resize(ReflowDim {
        columns: 20,
        lines: 10,
    });

    let final_dest_row = term
        .graphics
        .kitty_placements
        .values()
        .next()
        .unwrap()
        .dest_row;

    // The image is anchored to a cell directly below the wrapped row;
    // after the wrap there is one extra row above it, so dest_row
    // should increase by exactly 1.
    assert_eq!(
        final_dest_row - initial_dest_row,
        1,
        "Narrowing with content below the image should still shift the \
         placement down by 1 (got delta {})",
        final_dest_row - initial_dest_row
    );
}
