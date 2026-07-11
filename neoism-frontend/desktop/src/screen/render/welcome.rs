// Split from screen/misc.rs. Hosts the welcome-screen render path.

use super::super::*;

impl Screen<'_> {
    pub(crate) fn render_welcome(&mut self, mut before_present: impl FnMut()) {
        let window_id = self.context_manager.window_id();
        crate::router::routes::welcome::screen(
            &mut self.sugarloaf,
            &self.context_manager.current().dimension,
        );
        crate::app::freeze_watchdog::mark_render_stage(
            window_id,
            "welcome.pre_present_notify.begin",
        );
        before_present();
        crate::app::freeze_watchdog::mark_render_stage(
            window_id,
            "welcome.pre_present_notify.end",
        );
        crate::app::freeze_watchdog::mark_render_stage(
            window_id,
            "welcome.sugarloaf.present.begin",
        );
        self.sugarloaf.render();
        crate::app::freeze_watchdog::mark_render_stage(
            window_id,
            "welcome.sugarloaf.present.end",
        );
    }
}
