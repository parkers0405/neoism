use super::border::{compute, MIN_COLS, MIN_LINES};
use neoism_backend::config::layout::Margin;
use neoism_backend::sugarloaf::layout::TextDimensions;
use neoism_terminal_core::crosswords::grid::Dimensions;

#[derive(Copy, Clone, Debug)]
pub struct ContextDimension {
    pub width: f32,
    pub height: f32,
    pub columns: usize,
    pub lines: usize,
    pub dimension: TextDimensions,
    /// Font-derived cell height before an editor pane distributes its
    /// fractional vertical remainder across complete rows.
    pub nominal_cell_height: f32,
    pub margin: Margin,
    pub line_height: f32,
}

impl Default for ContextDimension {
    fn default() -> ContextDimension {
        ContextDimension {
            width: 0.,
            height: 0.,
            columns: MIN_COLS,
            lines: MIN_LINES,
            line_height: 1.,
            dimension: TextDimensions::default(),
            nominal_cell_height: 0.0,
            margin: Margin::default(),
        }
    }
}

impl ContextDimension {
    pub fn build(
        width: f32,
        height: f32,
        dimension: TextDimensions,
        line_height: f32,
        margin: Margin,
    ) -> Self {
        let (columns, lines) = compute(width, height, dimension, line_height, margin);
        Self {
            width,
            height,
            columns,
            lines,
            dimension,
            nominal_cell_height: dimension.height,
            margin,
            line_height,
        }
    }

    #[inline]
    pub fn update_width(&mut self, width: f32) {
        self.width = width;
        self.update();
    }

    #[inline]
    pub fn update_height(&mut self, height: f32) {
        self.height = height;
        self.update();
    }

    #[inline]
    pub fn update_line_height(&mut self, line_height: f32) {
        self.line_height = line_height;
        self.update();
    }

    #[inline]
    pub fn update_dimensions(&mut self, dimensions: TextDimensions) {
        self.dimension = dimensions;
        self.nominal_cell_height = dimensions.height;
        self.update();
    }

    #[inline]
    pub fn base_cell_height(&self) -> f32 {
        if self.nominal_cell_height > 0.0 {
            self.nominal_cell_height
        } else {
            self.dimension.height
        }
    }

    /// Apply a pane-local editor row fit calculated after tabs,
    /// breadcrumbs, splits, and status chrome are known.
    #[inline]
    pub fn apply_editor_row_fit(&mut self, fit: neoism_ui::chrome_policy::EditorRowFit) {
        self.lines = usize::from(fit.rows).max(1);
        self.dimension.height = fit.cell_height.max(1.0);
    }

    #[inline]
    pub fn restore_nominal_cell_height(&mut self) {
        self.dimension.height = self.base_cell_height().max(1.0);
        self.update();
    }

    #[inline]
    fn update(&mut self) {
        let (columns, lines) = compute(
            self.width,
            self.height,
            self.dimension,
            self.line_height,
            self.margin,
        );

        self.columns = columns;
        self.lines = lines;
    }
}

impl Dimensions for ContextDimension {
    #[inline]
    fn columns(&self) -> usize {
        self.columns
    }

    #[inline]
    fn screen_lines(&self) -> usize {
        self.lines
    }

    #[inline]
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn square_width(&self) -> f32 {
        self.dimension.width
    }

    fn square_height(&self) -> f32 {
        self.dimension.height
    }
}
