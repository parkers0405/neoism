use neoism_ui::user_event_policy::{
    occluded_event_action, resize_event_action, theme_changed_action,
    OccludedEventAction, ResizeEventAction, ThemeChangedAction,
};
use neoism_window::dpi::PhysicalSize;
use neoism_window::window::{Theme, WindowId};

use crate::app::Application;
use crate::bridges::utils::apply_theme_to_config;

impl Application<'_> {
    pub(in crate::app) fn handle_resized(
        &mut self,
        window_id: WindowId,
        new_size: PhysicalSize<u32>,
    ) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        match resize_event_action(new_size.width, new_size.height) {
            ResizeEventAction::SkipZeroSize => {
                route.window.screen.suspend_render_surface();
                return;
            }
            ResizeEventAction::ApplyResize => {
                route.window.screen.resize(new_size);
            }
        }
    }

    pub(in crate::app) fn handle_scale_factor_changed(
        &mut self,
        window_id: WindowId,
        scale_factor: f64,
    ) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        let scale = scale_factor as f32;
        route
            .window
            .screen
            .set_scale(scale, route.window.winit_window.inner_size());
        route.window.update_vblank_interval();
    }

    pub(in crate::app) fn handle_occluded(
        &mut self,
        window_id: WindowId,
        occluded: bool,
    ) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        let was_occluded = route.window.is_occluded;
        route.window.is_occluded = occluded;

        match occluded_event_action(was_occluded, occluded) {
            OccludedEventAction::UpdateOnly => {}
            OccludedEventAction::UpdateAndArmPostOcclusionRedraw => {
                route.window.needs_render_after_occlusion = true;
            }
        }
    }

    pub(in crate::app) fn handle_theme_changed(
        &mut self,
        window_id: WindowId,
        new_theme: Theme,
    ) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        match theme_changed_action(self.config.appearance.force_theme.is_some()) {
            ThemeChangedAction::IgnoreForcedTheme => return,
            ThemeChangedAction::ApplyNewTheme => {
                apply_theme_to_config(&mut self.config, Some(new_theme));
                route.window.screen.update_config(
                    &self.config,
                    &self.router.font_library,
                    false,
                );
                route.window.configure_window(&self.config);
            }
        }
    }
}
