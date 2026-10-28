// This file was heavily inspired by neovide implementation.

use neoism_terminal_core::ansi::CursorShape;
use sugarloaf::Sugarloaf;
use web_time::Instant;

/// Animation duration for long jumps (seconds).
/// Matches ghostty-pixel-scroll default — slightly faster than neovide's 0.15.
const ANIMATION_LENGTH: f32 = 0.10;

/// Animation duration for short (≤2 cell horizontal) movements.
/// Short typing moves need a faster leading edge so inserted text cannot
/// visually outrun the beam cursor while the trailing edge still floats.
const SHORT_ANIMATION_LENGTH: f32 = 0.04;

/// Trail size 0.0–1.0.
/// 1.0 = max stretch (leading edge jumps instantly, trailing edge lags most).
/// 0.8 matches ghostty-pixel-scroll — moderate stretch, less extreme than neovide.
const TRAIL_SIZE: f32 = 0.8;
const DEPTH: f32 = 0.0;
const ORDER: u8 = 30;

/// Insert / replace bar widths as a fraction of the cell. Matches
/// ghostty-pixel-scroll's `cell_percentage` default for non-block shapes.
const BEAM_WIDTH: f32 = 0.15;
const UNDERLINE_HEIGHT: f32 = 0.20;

#[derive(Clone)]
struct Spring {
    position: f32,
    velocity: f32,
}

impl Spring {
    #[inline]
    fn new() -> Self {
        Self {
            position: 0.0,
            velocity: 0.0,
        }
    }

    #[inline]
    fn reset(&mut self) {
        self.position = 0.0;
        self.velocity = 0.0;
    }

    /// Advance by variable `dt`. Returns `true` while still moving.
    #[inline]
    fn update(&mut self, dt: f32, animation_length: f32) -> bool {
        if animation_length <= dt {
            self.reset();
            return false;
        }
        if self.position == 0.0 {
            return false;
        }

        // Critically-damped spring (zeta = 1.0).
        // omega chosen so destination is reached within ~2% tolerance in
        // `animation_length` time.
        let omega = 4.0 / animation_length;

        // Analytical solution for critically-damped harmonic oscillation.
        let a = self.position;
        let b = a * omega + self.velocity;
        let c = (-omega * dt).exp();

        self.position = (a + b * dt) * c;
        self.velocity = c * (-a * omega - b * dt * omega + b);

        if self.position.abs() < 0.01 {
            self.reset();
            false
        } else {
            true
        }
    }
}

#[derive(Clone)]
struct Corner {
    spring_x: Spring,
    spring_y: Spring,
    /// Current animated pixel position.
    x: f32,
    y: f32,
    /// Offset relative to cursor center (shape-aware).
    rel_x: f32,
    rel_y: f32,
    prev_dest_x: f32,
    prev_dest_y: f32,
    anim_length: f32,
}

impl Corner {
    fn new(rel_x: f32, rel_y: f32) -> Self {
        Self {
            spring_x: Spring::new(),
            spring_y: Spring::new(),
            x: 0.0,
            y: 0.0,
            rel_x,
            rel_y,
            prev_dest_x: -1e6,
            prev_dest_y: -1e6,
            // Ghostty/Neovide no-jump scroll following still needs a
            // real spring length. If this starts at 0, the first
            // scroll-only destination change snaps and the cursor
            // looks like a static rectangle instead of gliding.
            anim_length: ANIMATION_LENGTH,
        }
    }

    #[inline]
    fn destination(
        &self,
        center_x: f32,
        center_y: f32,
        cell_w: f32,
        cell_h: f32,
    ) -> (f32, f32) {
        (
            center_x + self.rel_x * cell_w,
            center_y + self.rel_y * cell_h,
        )
    }

    #[inline]
    fn update(
        &mut self,
        center_x: f32,
        center_y: f32,
        cell_w: f32,
        cell_h: f32,
        dt: f32,
        immediate_movement: bool,
    ) -> bool {
        let (dest_x, dest_y) = self.destination(center_x, center_y, cell_w, cell_h);

        if (dest_x - self.prev_dest_x).abs() > 0.01
            || (dest_y - self.prev_dest_y).abs() > 0.01
        {
            self.spring_x.position = dest_x - self.x;
            self.spring_y.position = dest_y - self.y;
            self.prev_dest_x = dest_x;
            self.prev_dest_y = dest_y;
        }

        // Teleport: snap to destination without animating.
        if immediate_movement {
            self.x = dest_x;
            self.y = dest_y;
            self.spring_x.reset();
            self.spring_y.reset();
            return false;
        }

        let mut animating = self.spring_x.update(dt, self.anim_length);
        animating |= self.spring_y.update(dt, self.anim_length);
        self.x = dest_x - self.spring_x.position;
        self.y = dest_y - self.spring_y.position;

        animating
    }

    /// Direction alignment: dot product of the corner's relative direction
    /// with the travel direction.  Higher = more aligned with movement =
    /// "leading".  Matches neovide's `calculate_direction_alignment`.
    #[inline]
    fn direction_alignment(
        &self,
        center_x: f32,
        center_y: f32,
        cell_w: f32,
        cell_h: f32,
    ) -> f32 {
        let (dest_x, dest_y) = self.destination(center_x, center_y, cell_w, cell_h);

        // Corner's relative direction (normalized).
        let rel_len = (self.rel_x * self.rel_x + self.rel_y * self.rel_y)
            .sqrt()
            .max(1e-6);
        let corner_dir_x = self.rel_x / rel_len;
        let corner_dir_y = self.rel_y / rel_len;

        // Travel direction (from current animated pos to destination).
        let dx = dest_x - self.x;
        let dy = dest_y - self.y;
        let travel_len = (dx * dx + dy * dy).sqrt().max(1e-6);

        (dx / travel_len) * corner_dir_x + (dy / travel_len) * corner_dir_y
    }
}

pub struct TrailCursor {
    /// Four corners: [top-left, top-right, bottom-right, bottom-left].
    corners: [Corner; 4],
    /// Current cursor shape — drives corner relative positions so the
    /// trail quad matches a beam in insert mode and underline in replace.
    current_shape: CursorShape,
    /// Current destination center (physical pixels).
    dest_cx: f32,
    dest_cy: f32,
    /// Previous destination center, used to detect jumps.
    prev_dest_cx: f32,
    prev_dest_cy: f32,
    /// Center before the current jump — preserved so `compute_jump` can
    /// measure travel distance (since `set_destination` overwrites
    /// `prev_dest` before `animate` runs).
    jump_from_cx: f32,
    jump_from_cy: f32,
    /// One-shot flag: set when destination changes, consumed in `animate`.
    jumped: bool,
    /// Last logical destination change. Overlay cursors use this to stay
    /// visible briefly while typing/moving before blink resumes.
    last_destination_change: Option<Instant>,
    /// True until the first real destination is set — first frame teleports.
    first_frame: bool,
    animating: bool,
}

impl TrailCursor {
    /// The animated quad in host pixels, in TL/TR/BR/BL order. Alternate
    /// painters consume the same spring positions as Sugarloaf's triangles.
    pub fn quad_points(&self) -> [[f32; 2]; 4] {
        self.corners.each_ref().map(|corner| [corner.x, corner.y])
    }

    pub fn new() -> Self {
        Self {
            corners: [
                Corner::new(-0.5, -0.5), // top-left
                Corner::new(0.5, -0.5),  // top-right
                Corner::new(0.5, 0.5),   // bottom-right
                Corner::new(-0.5, 0.5),  // bottom-left
            ],
            current_shape: CursorShape::Block,
            dest_cx: 0.0,
            dest_cy: 0.0,
            prev_dest_cx: -1e6,
            prev_dest_cy: -1e6,
            jump_from_cx: -1e6,
            jump_from_cy: -1e6,
            jumped: false,
            last_destination_change: None,
            first_frame: true,
            animating: false,
        }
    }

    /// Adjust corner relative positions so the trail quad matches the
    /// active cursor shape. Block fills the cell, Beam collapses width
    /// to a thin vertical bar on the left, Underline collapses height
    /// to a thin horizontal bar at the bottom. Hidden zeroes the quad.
    /// Mirrors ghostty-pixel-scroll's `setCursorShape` (animation.zig).
    pub fn set_cursor_shape(&mut self, shape: CursorShape) {
        if self.current_shape == shape {
            return;
        }
        self.current_shape = shape;
        // Standard block corners; scale x/y to collapse one axis.
        const STD: [(f32, f32); 4] = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)];
        for (i, corner) in self.corners.iter_mut().enumerate() {
            let (x, y) = STD[i];
            let (rx, ry) = match shape {
                CursorShape::Block => (x, y),
                // Beam: thin bar pinned to the left edge of the cell.
                // Right edge moves from x=0.5 → -0.5 + BEAM_WIDTH.
                CursorShape::Beam => ((x + 0.5) * BEAM_WIDTH - 0.5, y),
                // Underline: thin bar pinned to the bottom edge.
                CursorShape::Underline => (x, -((-y + 0.5) * UNDERLINE_HEIGHT - 0.5)),
                CursorShape::Hidden => (0.0, 0.0),
            };
            corner.rel_x = rx;
            corner.rel_y = ry;
        }
    }

    /// Update the cursor destination.  Called once per frame **before**
    /// `animate()`.  Sets the `jumped` flag when the destination changes
    /// (matching neovide's `update_cursor_destination`).
    pub fn set_destination(
        &mut self,
        cursor_x: f32,
        cursor_y: f32,
        cell_width: f32,
        cell_height: f32,
    ) {
        self.set_destination_inner(cursor_x, cursor_y, cell_width, cell_height, true);
    }

    /// Update the visual destination without marking a new logical
    /// cursor jump. Neovide does this when only the scroll spring moves:
    /// the cursor destination follows the sliding grid every frame, but
    /// corner ranking is recalculated only when the raw grid cursor
    /// row/col changes. Re-ranking on every fractional scroll frame
    /// makes held down/up at the viewport edge feel sticky and poppy.
    pub fn set_destination_no_jump(
        &mut self,
        cursor_x: f32,
        cursor_y: f32,
        cell_width: f32,
        cell_height: f32,
    ) {
        self.set_destination_inner(cursor_x, cursor_y, cell_width, cell_height, false);
    }

    fn set_destination_inner(
        &mut self,
        cursor_x: f32,
        cursor_y: f32,
        cell_width: f32,
        cell_height: f32,
        mark_jump: bool,
    ) {
        // Center of cursor cell.
        let cx = cursor_x + cell_width * 0.5;
        let cy = cursor_y + cell_height * 0.5;
        self.dest_cx = cx;
        self.dest_cy = cy;

        // Detect a jump (destination changed).
        if (cx - self.prev_dest_cx).abs() > 0.01 || (cy - self.prev_dest_cy).abs() > 0.01
        {
            self.jump_from_cx = self.prev_dest_cx;
            self.jump_from_cy = self.prev_dest_cy;
            self.prev_dest_cx = cx;
            self.prev_dest_cy = cy;
            self.last_destination_change = Some(Instant::now());
            if mark_jump {
                self.jumped = true;
            }
        }
    }

    /// Run animation for one frame.  Called once per frame **after**
    /// `set_destination()`.  If `jumped` is set, computes corner ranking
    /// and assigns animation lengths exactly once per jump (matching
    /// neovide's `animate`).
    pub fn animate(&mut self, cell_width: f32, cell_height: f32, dt: f32) {
        let dt = dt.clamp(0.0, 0.05);
        let cx = self.dest_cx;
        let cy = self.dest_cy;

        // First frame: teleport all corners to destination without
        // animation (matches neovide's `immediate_movement`).
        let immediate = self.first_frame;
        if self.first_frame {
            self.first_frame = false;
        }

        // On jump: compute ranking and set animation lengths (one-shot).
        if self.jumped && !immediate {
            self.compute_jump(cx, cy, cell_width, cell_height);
        }
        self.jumped = false;

        // Spring update every frame (matching neovide).
        let mut still_animating = false;
        for corner in &mut self.corners {
            if corner.update(cx, cy, cell_width, cell_height, dt, immediate) {
                still_animating = true;
            }
        }

        self.animating = still_animating;
    }

    /// Snap the trail to the current destination without drawing an
    /// after-image. Used by surfaces that already animate their own
    /// cursor or need an immediate focus handoff.
    pub fn snap_to_destination(&mut self, cell_width: f32, cell_height: f32) {
        let cx = self.dest_cx;
        let cy = self.dest_cy;
        for corner in &mut self.corners {
            let _ = corner.update(cx, cy, cell_width, cell_height, 0.0, true);
        }
        self.jumped = false;
        self.first_frame = false;
        self.animating = false;
    }

    /// Forget the previous destination so the next render snaps to the
    /// active pane/cell instead of animating from stale workspace metrics.
    pub fn reset(&mut self) {
        for corner in &mut self.corners {
            corner.spring_x.reset();
            corner.spring_y.reset();
            corner.x = 0.0;
            corner.y = 0.0;
            corner.prev_dest_x = -1e6;
            corner.prev_dest_y = -1e6;
            corner.anim_length = ANIMATION_LENGTH;
        }
        self.dest_cx = 0.0;
        self.dest_cy = 0.0;
        self.prev_dest_cx = -1e6;
        self.prev_dest_cy = -1e6;
        self.jump_from_cx = -1e6;
        self.jump_from_cy = -1e6;
        self.jumped = false;
        self.last_destination_change = None;
        self.first_frame = true;
        self.animating = false;
    }

    pub fn blink_hold_visible(&self, blinking_interval_ms: u64) -> bool {
        match self.last_destination_change {
            Some(last_change) => {
                last_change.elapsed().as_millis() < blinking_interval_ms as u128
            }
            None => false,
        }
    }

    /// Compute corner direction-alignment ranking and assign animation
    /// lengths.  Called exactly once per cursor jump (matching neovide's
    /// `Corner::jump` called from the `if self.jumped` block).
    fn compute_jump(&mut self, cx: f32, cy: f32, cell_width: f32, cell_height: f32) {
        // Compute jump vector in cell units for short-movement detection.
        // `jump_from` is the center *before* this jump was detected.
        let jump_x = if cell_width > 0.0 {
            ((cx - self.jump_from_cx) / cell_width).abs()
        } else {
            0.0
        };
        let jump_y = if cell_height > 0.0 {
            ((cy - self.jump_from_cy) / cell_height).abs()
        } else {
            0.0
        };
        let is_short = jump_x <= 2.001 && jump_y < 0.001;

        // Direction-alignment ranking (neovide-style).
        let mut alignments: [(usize, f32); 4] = [
            (
                0,
                self.corners[0].direction_alignment(cx, cy, cell_width, cell_height),
            ),
            (
                1,
                self.corners[1].direction_alignment(cx, cy, cell_width, cell_height),
            ),
            (
                2,
                self.corners[2].direction_alignment(cx, cy, cell_width, cell_height),
            ),
            (
                3,
                self.corners[3].direction_alignment(cx, cy, cell_width, cell_height),
            ),
        ];

        // Sort ascending: lowest alignment = most trailing.
        alignments.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });

        // Build per-corner rank array.
        let mut ranks = [0usize; 4];
        for (rank, &(corner_idx, _)) in alignments.iter().enumerate() {
            ranks[corner_idx] = rank;
        }

        let base_length = if is_short {
            ANIMATION_LENGTH.min(SHORT_ANIMATION_LENGTH)
        } else {
            ANIMATION_LENGTH
        };
        let leading = base_length * (1.0 - TRAIL_SIZE).clamp(0.0, 1.0);
        let trailing = base_length;
        let mid = (leading + trailing) / 2.0;

        for (i, corner) in self.corners.iter_mut().enumerate() {
            corner.anim_length = match ranks[i] {
                0 => trailing,
                1 => mid,
                _ => leading,
            };
        }
    }

    /// Draw the cursor trail as a single convex quad spanned by the four
    /// animated corners — emitted as two triangles through the existing
    /// `DrawCmd::Vertices` pipeline. Matches neovide's approach of
    /// `PathBuilder::move_to(TL).line_to(TR).line_to(BR).line_to(BL).close()`
    /// into a single `draw_path`. The old scanline fill (up to 640 rects
    /// per frame) was a workaround for `sugarloaf.rect` being axis-aligned
    /// only; `sugarloaf.triangle` already accepts arbitrary vertex
    /// positions, so one fan covers the same pixels in one draw call.
    pub fn draw(
        &self,
        sugarloaf: &mut Sugarloaf,
        scale_factor: f32,
        cursor_color: [f32; 4],
    ) {
        if !self.animating {
            return;
        }

        self.draw_quad(sugarloaf, scale_factor, cursor_color);
    }

    /// Draw the cursor even when the trail is settled. Tree focus has no
    /// GPU cell cursor underneath it, so the same four-corner cursor
    /// primitive must also render the resting cursor shape.
    pub fn draw_always(
        &self,
        sugarloaf: &mut Sugarloaf,
        scale_factor: f32,
        cursor_color: [f32; 4],
    ) {
        self.draw_quad(sugarloaf, scale_factor, cursor_color);
    }

    fn draw_quad(
        &self,
        sugarloaf: &mut Sugarloaf,
        scale_factor: f32,
        cursor_color: [f32; 4],
    ) {
        let inv = 1.0 / scale_factor;

        // Corner positions in *logical* pixels (sugarloaf.triangle scales
        // by scale_factor internally). Ordered TL, TR, BR, BL — same
        // winding as neovide's path builder. Keep the float positions:
        // rounding here quantizes sub-pixel spring motion and makes the
        // cursor glide feel like it has turned back into a static block.
        let pts: [(f32, f32); 4] = [
            (self.corners[0].x * inv, self.corners[0].y * inv),
            (self.corners[1].x * inv, self.corners[1].y * inv),
            (self.corners[2].x * inv, self.corners[2].y * inv),
            (self.corners[3].x * inv, self.corners[3].y * inv),
        ];

        // Fan from TL: (TL, TR, BR) + (TL, BR, BL). Two triangles share
        // TL and BR, so the shared diagonal seam is hidden inside the
        // convex hull — same as any triangle-fan tessellation.
        sugarloaf.triangle_ordered(
            pts[0].0,
            pts[0].1,
            pts[1].0,
            pts[1].1,
            pts[2].0,
            pts[2].1,
            DEPTH,
            cursor_color,
            ORDER,
        );
        sugarloaf.triangle_ordered(
            pts[0].0,
            pts[0].1,
            pts[2].0,
            pts[2].1,
            pts[3].0,
            pts[3].1,
            DEPTH,
            cursor_color,
            ORDER,
        );
    }

    /// `true` while the spring corners haven't settled *visibly*.
    #[inline]
    pub fn is_animating(&self) -> bool {
        self.animating
    }
}
