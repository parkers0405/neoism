use neoism_backend::sugarloaf::Sugarloaf;
use neoism_ui::primitives::ide_theme::IdeTheme;

use super::state::FileTree;

impl FileTree {
    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        panel_width: f32,
        panel_height: f32,
        theme: &IdeTheme,
        text_occlusion_rects: &[[f32; 4]],
        plugins: Option<&neoism_lua::PluginSnapshot>,
    ) {
        self.inner.render(
            sugarloaf,
            x_left,
            y_top,
            panel_width,
            panel_height,
            theme,
            text_occlusion_rects,
            plugins,
        );
    }
}
