//! Declarative event propagation. Listener actions are values, not callbacks;
//! propagation/default policy is fixed on the binding before dispatch begins.
use std::collections::HashMap;

use crate::input::{InputEvent, Key, Modifiers, PointerButton};
use crate::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    PointerDown, PointerUp, PointerMove, PointerCancel, Click,
    HoverEnter, HoverLeave, Wheel, Key, Text, Paste,
    ImePreedit, ImeCommit, ImeCancel, Focus, Blur,
}

impl EventKind {
    pub(crate) fn of(event: &InputEvent) -> Self {
        match event {
            InputEvent::PointerDown { .. } => Self::PointerDown,
            InputEvent::PointerUp { .. } => Self::PointerUp,
            InputEvent::PointerMove { .. } => Self::PointerMove,
            InputEvent::PointerCancel => Self::PointerCancel,
            InputEvent::Wheel { .. } => Self::Wheel,
            InputEvent::Key { .. } => Self::Key,
            InputEvent::Text(_) => Self::Text,
            InputEvent::Paste(_) => Self::Paste,
            InputEvent::ImePreedit { .. } => Self::ImePreedit,
            InputEvent::ImeCommit(_) => Self::ImeCommit,
            InputEvent::ImeCancel => Self::ImeCancel,
            InputEvent::Focus(true) => Self::Focus,
            InputEvent::Focus(false) => Self::Blur,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventPhase { Capture, Target, Bubble }

#[derive(Clone, Debug)]
pub struct EventBinding<Action> {
    pub kind: EventKind,
    pub phase: EventPhase,
    pub action: Action,
    pub stop_propagation: bool,
    pub prevent_default: bool,
}

impl<Action> EventBinding<Action> {
    pub fn new(kind: EventKind, phase: EventPhase, action: Action) -> Self {
        Self { kind, phase, action, stop_propagation: false, prevent_default: false }
    }
    /// Stops traversal after the current node's matching listeners have run.
    pub fn stop_propagation(mut self, stop: bool) -> Self { self.stop_propagation = stop; self }
    /// Suppresses built-in focus, keybinding, click or activation defaults.
    pub fn prevent_default(mut self, prevent: bool) -> Self { self.prevent_default = prevent; self }
}

#[derive(Clone, Debug)]
pub struct KeyBinding<Action> {
    pub key: Key,
    pub modifiers: Modifiers,
    pub action: Action,
    /// Keybindings do not repeat unless explicitly opted in.
    pub repeat: bool,
}

impl<Action> KeyBinding<Action> {
    pub fn new(key: Key, modifiers: Modifiers, action: Action) -> Self {
        Self { key, modifiers, action, repeat: false }
    }
    pub fn repeat(mut self, repeat: bool) -> Self { self.repeat = repeat; self }
}

#[derive(Clone, Debug)]
pub struct RoutedAction<Action> {
    /// The node whose listener/default produced this action.
    pub key: String,
    pub action: Action,
    /// Original input, including coordinates/IME payload for typed application updates.
    pub event: InputEvent,
    pub kind: EventKind,
    pub phase: EventPhase,
}

#[derive(Clone, Debug)]
pub struct Dispatch<Action> {
    pub actions: Vec<RoutedAction<Action>>,
    pub consumed: bool,
    pub prevent_default: bool,
    pub focus_changed: bool,
    pub needs_redraw: bool,
}

impl<Action> Default for Dispatch<Action> {
    fn default() -> Self {
        Self { actions: Vec::new(), consumed: false, prevent_default: false,
            focus_changed: false, needs_redraw: false }
    }
}

impl<Action> Dispatch<Action> {
    pub(crate) fn merge_notification(&mut self, mut other: Self) {
        // Enter/leave/focus notifications cannot cancel their originating input.
        other.prevent_default = false;
        self.merge(other);
    }

    pub(crate) fn merge(&mut self, mut other: Self) {
        self.actions.append(&mut other.actions);
        self.consumed |= other.consumed;
        self.prevent_default |= other.prevent_default;
        self.focus_changed |= other.focus_changed;
        self.needs_redraw |= other.needs_redraw;
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Mount {
    pub id: taffy::prelude::NodeId,
    pub parent: Option<String>,
    pub disabled: bool,
    pub scope: Option<bool>,
    pub focusable: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct RouteNode<Action> {
    pub mount: Mount,
    pub bounds: Rect,
    pub clip: Rect,
    pub bindings: Vec<EventBinding<Action>>,
    pub key_bindings: Vec<KeyBinding<Action>>,
    pub action: Option<Action>,
    pub capture_pointer: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Routes<Action> {
    pub nodes: HashMap<String, RouteNode<Action>>,
    pub order: Vec<String>,
}

impl<Action> Default for Routes<Action> {
    fn default() -> Self { Self { nodes: HashMap::new(), order: Vec::new() } }
}

impl<Action: Clone> Routes<Action> {
    pub fn path(&self, target: &str) -> Vec<String> {
        let mut path = Vec::new();
        let mut next = Some(target);
        while let Some(key) = next {
            let Some(node) = self.nodes.get(key) else { break; };
            path.push(key.to_owned());
            next = node.mount.parent.as_deref();
        }
        path.reverse();
        path
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<&str> {
        self.order.iter().rev().find_map(|key| {
            let node = &self.nodes[key];
            let participates = node.mount.disabled || node.action.is_some()
                || node.mount.focusable || node.capture_pointer || !node.bindings.is_empty();
            (participates && node.bounds.contains(x, y) && node.clip.contains(x, y))
                .then_some(key.as_str())
        })
    }

    pub fn enabled(&self, key: &str) -> bool {
        self.nodes.get(key).is_some_and(|node| !node.mount.disabled)
    }

    pub fn route(&self, target: &str, kind: EventKind, event: &InputEvent) -> Dispatch<Action> {
        let mut result = Dispatch::default();
        if !self.enabled(target) { result.consumed = true; return result; }
        let path = self.path(target);
        // Capture and bubble include the target, so the customary capture/bubble
        // listener works on leaves too. Target listeners run exactly once between.
        for phase in [EventPhase::Capture, EventPhase::Target, EventPhase::Bubble] {
            let keys: Vec<&String> = match phase {
                EventPhase::Capture => path.iter().collect(),
                EventPhase::Target => path.last().into_iter().collect(),
                EventPhase::Bubble => path.iter().rev().collect(),
            };
            for key in keys {
                let node = &self.nodes[key];
                if node.mount.disabled { return result; }
                let mut stop = false;
                for binding in &node.bindings {
                    if binding.kind == kind && binding.phase == phase {
                        result.actions.push(RoutedAction { key: key.clone(), action: binding.action.clone(),
                            event: event.clone(), kind, phase });
                        result.consumed = true;
                        result.prevent_default |= binding.prevent_default;
                        stop |= binding.stop_propagation;
                    }
                }
                if stop { return result; }
            }
        }
        result
    }

    /// Enter/leave are per-node transitions, not repeated bubbling events for
    /// every descendant crossed. Each changed ancestor receives one transition.
    pub fn transition(&self, key: &str, kind: EventKind, event: &InputEvent) -> Dispatch<Action> {
        let mut result = Dispatch::default();
        let Some(node) = self.nodes.get(key).filter(|node| !node.mount.disabled) else { return result; };
        for phase in [EventPhase::Capture, EventPhase::Target, EventPhase::Bubble] {
            for binding in &node.bindings {
                if binding.kind == kind && binding.phase == phase {
                    result.actions.push(RoutedAction { key: key.to_owned(), action: binding.action.clone(),
                        event: event.clone(), kind, phase });
                    result.consumed = true;
                    result.prevent_default |= binding.prevent_default;
                }
            }
        }
        result
    }

    pub fn has_click(&self, key: &str) -> bool {
        self.nodes.get(key).is_some_and(|node|
            node.bindings.iter().any(|binding| binding.kind == EventKind::Click))
    }
}

#[derive(Default)]
pub(crate) struct RoutingState {
    pub capture: Option<(String, PointerButton)>,
    pub down: Option<(String, PointerButton)>,
    pub hover: Vec<String>,
    /// Modal identity, and the identity-bearing focus to restore on removal.
    pub traps: Vec<(String, taffy::prelude::NodeId, Option<(String, taffy::prelude::NodeId)>)>,
}
