//! UIKit owns the menu and dispatches localized standard responder actions. The custom editor
//! owns the document and never gives UIKit (or the pasteboard) its unselected draft text.
use objc2::runtime::AnyClass;
use objc2_ui_kit::{UIEditMenuConfiguration, UIEditMenuInteraction, UIPasteboard};

use super::*;
use crate::platform::ios::EditAction;

#[derive(Default)]
pub(super) struct EditMenu {
    interaction: RefCell<Option<Retained<UIEditMenuInteraction>>>,
    actions: Cell<Option<(bool, bool)>>,
}

impl EditMenu {
    pub(super) fn can_perform(&self, action: Sel) -> Option<bool> {
        // Check only type availability here, so opening the menu does not read paste data.
        Some(match action {
            a if a == sel!(cut:) || a == sel!(copy:) => self.actions.get().is_some_and(|a| a.0),
            a if a == sel!(selectAll:) => self.actions.get().is_some_and(|a| a.1),
            a if a == sel!(paste:) => self.actions.get().is_some()
                && unsafe { UIPasteboard::generalPasteboard().hasStrings() },
            _ => return None,
        })
    }
}

impl WinitView {
    pub(in crate::platform_impl::ios) fn present_edit_menu(&self, (x, y): (f32, f32), selection: bool, select_all: bool) {
        // The interaction is available on iOS 16+. Older stock winit deployments can still
        // use its keyboard bridge and Rune's selection without constructing this class.
        if AnyClass::get("UIEditMenuInteraction").is_none() || !unsafe { self.isFirstResponder() } {
            return;
        }
        let mtm = MainThreadMarker::new().unwrap();
        let existing = self.ivars().edit_menu.interaction.borrow().clone();
        let menu = existing.unwrap_or_else(|| unsafe {
            let menu = UIEditMenuInteraction::initWithDelegate(mtm.alloc(), None);
            self.addInteraction(ProtocolObject::from_ref(&*menu));
            self.ivars().edit_menu.interaction.replace(Some(menu.clone()));
            menu
        });
        self.ivars().edit_menu.actions.set(Some((selection, select_all)));
        unsafe {
            let config = UIEditMenuConfiguration::configurationWithIdentifier_sourcePoint(
                None, CGPoint::new(x.into(), y.into()), mtm,
            );
            menu.presentEditMenuWithConfiguration(&config);
        }
    }

    pub(in crate::platform_impl::ios) fn dismiss_edit_menu(&self) {
        let state = &self.ivars().edit_menu;
        if state.actions.take().is_some() {
            if let Some(menu) = state.interaction.borrow().clone() {
                unsafe { menu.dismissMenu(); }
            }
        }
    }

    pub(super) fn edit_menu_action(&self, selector: Sel) {
        if self.ivars().edit_menu.can_perform(selector) != Some(true) { return; }
        let Some(window) = self.window() else { return };
        let event = if selector == sel!(paste:) {
            // The read happens inside the system Paste action, so UIKit sees the explicit
            // paste gesture. Pass the text through Rune's normal paste handlers afterward.
            let Some(text) = (unsafe { UIPasteboard::generalPasteboard().string() }) else { return };
            EditAction::Paste(text.to_string())
        } else if selector == sel!(copy:) { EditAction::Copy }
        else if selector == sel!(cut:) { EditAction::Cut }
        else { EditAction::SelectAll };
        app_state::handle_nonuser_event(MainThreadMarker::new().unwrap(), EventWrapper::StaticEvent(Event::WindowEvent {
            window_id: RootWindowId(window.id()),
            event: WindowEvent::IosEditAction(event),
        }));
    }
}