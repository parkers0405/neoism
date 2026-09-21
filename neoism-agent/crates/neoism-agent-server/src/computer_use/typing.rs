//! Standalone whole-string typing facade. No MCP, application state, or target ownership.
use anyhow::{bail, ensure};
use serde::Deserialize;
#[derive(Debug, Default, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(super) enum Method {
    #[default]
    Auto,
    Keyboard,
    Native,
    Paste,
}
#[derive(Debug, Default, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(super) enum ClipboardPolicy {
    #[default]
    Forbid,
    Replace,
}

// Pure policy decision: only unrepresentability (not a failed probe) permits paste.
pub(super) fn select(
    method: Method,
    policy: ClipboardPolicy,
    representable: bool,
    linux: bool,
) -> anyhow::Result<Method> {
    if linux {
        match method {
            Method::Native => bail!("Native Unicode injection is unsupported on Linux"),
            Method::Keyboard => {
                ensure!(
                    representable,
                    "Text is not representable in the existing keyboard layout"
                );
                Ok(Method::Keyboard)
            }
            Method::Auto if representable => Ok(Method::Keyboard),
            Method::Auto | Method::Paste => {
                ensure!(policy==ClipboardPolicy::Replace,"Text requires clipboard replacement; clipboard_policy:replace was not granted");
                Ok(Method::Paste)
            }
        }
    } else {
        match method {
            Method::Auto | Method::Native => Ok(Method::Native),
            Method::Keyboard => {
                bail!("Forced layout keyboard typing is unsupported on this platform")
            }
            Method::Paste => bail!("Clipboard paste is unsupported on this platform"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_string_policy_never_implicitly_replaces_clipboard() {
        assert_eq!(
            select(Method::Auto, ClipboardPolicy::Forbid, true, true).unwrap(),
            Method::Keyboard
        );
        assert!(select(Method::Auto, ClipboardPolicy::Forbid, false, true).is_err());
        assert_eq!(
            select(Method::Auto, ClipboardPolicy::Replace, false, true).unwrap(),
            Method::Paste
        );
        assert!(select(Method::Keyboard, ClipboardPolicy::Replace, false, true).is_err());
        assert!(select(Method::Native, ClipboardPolicy::Replace, true, true).is_err());
        assert!(select(Method::Paste, ClipboardPolicy::Forbid, true, true).is_err());
        assert_eq!(
            select(Method::Auto, ClipboardPolicy::Replace, false, false).unwrap(),
            Method::Native
        );
        assert!(select(Method::Paste, ClipboardPolicy::Replace, false, false).is_err());
    }
}

/// Trusted caller consent, separate from model-supplied eligibility policy.
/// Only the permission-checked host (or a deliberately consenting live fixture)
/// constructs this capability. Never deserialize it from action JSON.
#[derive(Clone, Copy)]
pub(super) struct ClipboardPermit(());
impl ClipboardPermit {
    pub(super) fn granted() -> Self {
        Self(())
    }
}

/// Effect snapshots contain dispatch facts only, never the input payload.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TextEffects {
    pub method: &'static str,
    pub unit_kind: &'static str,
    pub phase: &'static str,
    pub completed_native_units: usize,
    pub current_unit_uncertain: bool,
    pub clipboard: &'static str,
    pub cleanup_failure: Option<String>,
}
#[cfg(target_os = "linux")]
pub(super) struct TextPlan<P> {
    keys: P,
    clipboard: Option<String>,
    unit_kind: &'static str,
}
#[cfg(target_os = "linux")]
impl<P> TextPlan<P> {
    pub(super) fn method(&self) -> &'static str {
        if self.clipboard.is_some() {
            "paste"
        } else {
            "keyboard"
        }
    }
}
#[cfg(target_os = "linux")]
pub(super) type PreparedText = TextPlan<super::linux_text::KeyPlan>;
#[cfg(target_os = "linux")]
impl PreparedText {
    pub(super) fn unit_count(&self) -> usize {
        self.keys.unit_count()
    }
    pub(super) fn unit_kind(&self) -> &'static str {
        self.keys.units_kind
    }
}
/// Prepare a whole string against the batch-owned session. No virtual keyboard
/// creation or clipboard publication. forced paste (including the legacy alias) permits LF/Tab.
#[cfg(target_os = "linux")]
pub(super) fn prepare_text(
    session: &super::linux_text::KeyboardSession,
    text: &str,
    method: Method,
    policy: ClipboardPolicy,
    paste_keys: &[enigo::Key],
    legacy_paste: bool,
    permit: Option<ClipboardPermit>,
    guard: &mut dyn FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<PreparedText> {
    let mut plan = prepare_with(
        text,
        method,
        policy,
        legacy_paste,
        permit,
        guard,
        || session.plan_text(text),
        || session.plan_keys(paste_keys),
        super::linux_clipboard::preflight,
    )?;
    plan.unit_kind = plan.keys.units_kind;
    Ok(plan)
}
// Mock seam below validation and policy, and immediately above native probes.
#[cfg(target_os = "linux")]
fn prepare_with<P>(
    text: &str,
    method: Method,
    policy: ClipboardPolicy,
    legacy_paste: bool,
    permit: Option<ClipboardPermit>,
    guard: &mut dyn FnMut() -> anyhow::Result<()>,
    plan_text: impl FnOnce() -> anyhow::Result<Option<P>>,
    plan_keys: impl FnOnce() -> anyhow::Result<P>,
    preflight: impl FnOnce(&mut dyn FnMut() -> anyhow::Result<()>) -> anyhow::Result<()>,
) -> anyhow::Result<TextPlan<P>> {
    ensure!(text.chars().count() <= 512, "Text exceeds 512 characters");
    ensure!(
        if legacy_paste || method == Method::Paste {
            !text.contains('\0')
        } else {
            !text.chars().any(char::is_control)
        },
        "Literal typing forbids all C0/C1 controls, including LF/Tab"
    );
    guard()?;
    let method = if legacy_paste { Method::Paste } else { method };
    let plan = if matches!(method, Method::Auto | Method::Keyboard) {
        plan_text()?
    } else {
        None
    };
    match select(method, policy, plan.is_some(), true)? {
        Method::Keyboard => Ok(TextPlan {
            keys: plan.unwrap(),
            clipboard: None,
            unit_kind: "unicode_scalar",
        }),
        Method::Paste => {
            ensure!(permit.is_some(),"Clipboard replacement requires explicit human computer_clipboard permission");
            // The chord must be representable BEFORE clipboard preflight/publication.
            let keys = plan_keys()?;
            preflight(guard)?;
            Ok(TextPlan {
                keys,
                clipboard: Some(text.to_owned()),
                unit_kind: "chord",
            })
        }
        _ => unreachable!(),
    }
}
/// Dispatch this exact plan once. Errors never trigger another method or retry.
/// The caller owns the borrowed session and MUST call `session.finish()` after
/// its final action, and report any cleanup error rather than relying on Drop.
#[cfg(target_os = "linux")]
pub(super) fn execute_text(
    session: &mut super::linux_text::KeyboardSession,
    plan: &PreparedText,
    guard: &mut dyn FnMut() -> anyhow::Result<()>,
    sink: &mut dyn FnMut(&TextEffects),
) -> anyhow::Result<()> {
    let session = std::cell::RefCell::new(session);
    execute_with(
        plan,
        guard,
        sink,
        |guard| session.borrow_mut().revalidate(&plan.keys, guard),
        |text, guard| super::linux_clipboard::publish(text, guard),
        |keys, guard, progress| session.borrow_mut().execute(keys, guard, progress),
    )
}
#[cfg(target_os = "linux")]
pub(super) fn execute_text_with_wait(
    session: &mut super::linux_text::KeyboardSession,
    plan: &PreparedText,
    guard: &mut dyn FnMut() -> anyhow::Result<()>,
    wait: &mut dyn FnMut() -> anyhow::Result<()>,
    sink: &mut dyn FnMut(&TextEffects),
) -> anyhow::Result<()> {
    let session = std::cell::RefCell::new(session);
    let wait = std::cell::RefCell::new(wait);
    execute_with(
        plan,
        guard,
        sink,
        |_| {
            session
                .borrow_mut()
                .revalidate(&plan.keys, &mut **wait.borrow_mut())
        },
        |text, guard| super::linux_clipboard::publish(text, guard),
        |keys, guard, progress| {
            session.borrow_mut().execute_with_checks(
                keys,
                &mut super::linux_text::Checks {
                    wait: &mut **wait.borrow_mut(),
                    full: guard,
                },
                progress,
            )
        },
    )
}
#[cfg(target_os = "linux")]
fn execute_with<P>(
    plan: &TextPlan<P>,
    guard: &mut dyn FnMut() -> anyhow::Result<()>,
    sink: &mut dyn FnMut(&TextEffects),
    mut revalidate: impl FnMut(&mut dyn FnMut() -> anyhow::Result<()>) -> anyhow::Result<()>,
    mut publish: impl FnMut(
        &str,
        &mut dyn FnMut() -> anyhow::Result<()>,
    ) -> anyhow::Result<()>,
    mut execute: impl FnMut(
        &P,
        &mut dyn FnMut() -> anyhow::Result<()>,
        &mut dyn FnMut(usize),
    ) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut effects = TextEffects {
        method: plan.method(),
        unit_kind: plan.unit_kind,
        phase: "prepared",
        completed_native_units: 0,
        current_unit_uncertain: false,
        clipboard: "unchanged",
        cleanup_failure: None,
    };
    sink(&effects);
    guard()?;
    let result = (|| {
        revalidate(guard)?;
        if let Some(text) = &plan.clipboard {
            effects.phase = "clipboard_publication";
            effects.clipboard = "may_changed";
            sink(&effects);
            if let Err(error) = publish(text, &mut *guard) {
                effects.clipboard = match super::linux_clipboard::change_phase(&error) {
                    Some(super::linux_clipboard::ChangePhase::Unchanged) => "unchanged",
                    Some(super::linux_clipboard::ChangePhase::ConfirmedPublication) => {
                        "changed"
                    }
                    _ => "may_changed",
                };
                sink(&effects);
                return Err(error);
            }
            effects.clipboard = "changed";
            sink(&effects);
        }
        guard()?;
        effects.phase = "dispatch";
        effects.current_unit_uncertain = true;
        sink(&effects);
        execute(&plan.keys, guard, &mut |units| {
            effects.completed_native_units += units;
            sink(&effects)
        })?;
        effects.phase = "complete";
        effects.current_unit_uncertain = false;
        sink(&effects);
        Ok(())
    })();
    if let Err(error) = &result {
        if let Some(facts) = super::linux_text::failure_facts(error) {
            effects.completed_native_units = facts.completed_units;
            effects.current_unit_uncertain = facts.current_unit_uncertain;
            if facts.cleanup_failed {
                effects.cleanup_failure = Some(format!("{error:#}"));
            }
        }
        sink(&effects);
    }
    result
}

#[cfg(all(test, target_os = "linux"))]
mod pipeline_tests {
    use super::*;
    use std::cell::Cell;
    fn plan(
        text: &str,
        method: Method,
        policy: ClipboardPolicy,
        permit: bool,
        represented: bool,
    ) -> anyhow::Result<TextPlan<usize>> {
        prepare_with(
            text,
            method,
            policy,
            false,
            permit.then(ClipboardPermit::granted),
            &mut || Ok(()),
            || Ok(represented.then_some(3)),
            || Ok(1),
            |guard| guard(),
        )
    }
    #[test]
    fn later_unrepresentable_string_rejects_entire_prefix_before_dispatch() {
        let events = Cell::new(0);
        let plans = [("hello", true), ("🦀", false)]
            .into_iter()
            .map(|(text, yes)| {
                plan(text, Method::Auto, ClipboardPolicy::Forbid, false, yes)
            })
            .collect::<anyhow::Result<Vec<_>>>();
        if let Ok(plans) = plans {
            for plan in plans {
                execute_with(
                    &plan,
                    &mut || Ok(()),
                    &mut |_| {},
                    |_| Ok(()),
                    |_, _| {
                        events.set(events.get() + 1);
                        Ok(())
                    },
                    |_, _, _| {
                        events.set(events.get() + 1);
                        Ok(())
                    },
                )
                .unwrap();
            }
        }
        assert_eq!(events.get(), 0);
    }
    #[test]
    fn no_fallback_or_retry_after_partial_dispatch() {
        let plan =
            plan("abc", Method::Auto, ClipboardPolicy::Replace, true, true).unwrap();
        let calls = Cell::new(0);
        let publications = Cell::new(0);
        let mut last = None;
        let error = execute_with(
            &plan,
            &mut || Ok(()),
            &mut |e| last = Some(e.clone()),
            |_| Ok(()),
            |_, _| {
                publications.set(publications.get() + 1);
                Ok(())
            },
            |_, _, progress| {
                calls.set(calls.get() + 1);
                progress(1);
                progress(1);
                bail!("layout changed; cleanup failed")
            },
        );
        assert!(error.is_err());
        assert_eq!(calls.get(), 1);
        assert_eq!(publications.get(), 0);
        let last = last.unwrap();
        assert_eq!(last.completed_native_units, 2);
        assert_eq!(last.method, "keyboard");
        assert!(last.current_unit_uncertain);
        assert!(
            last.cleanup_failure.is_none(),
            "untyped text is not cleanup evidence"
        );
    }
    #[test]
    fn paste_chord_failure_never_preflights_or_changes_clipboard() {
        let probes = Cell::new(0);
        let result = prepare_with::<usize>(
            "🦀",
            Method::Auto,
            ClipboardPolicy::Replace,
            false,
            Some(ClipboardPermit::granted()),
            &mut || Ok(()),
            || Ok(None),
            || bail!("paste chord missing"),
            |_| {
                probes.set(probes.get() + 1);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(probes.get(), 0);
    }
    #[test]
    fn fatal_layout_probe_error_is_not_unrepresentability() {
        let probes = Cell::new(0);
        let result = prepare_with::<usize>(
            "abc",
            Method::Auto,
            ClipboardPolicy::Replace,
            false,
            Some(ClipboardPermit::granted()),
            &mut || Ok(()),
            || bail!("layout snapshot lost"),
            || {
                probes.set(probes.get() + 1);
                Ok(1)
            },
            |_| Ok(()),
        );
        assert!(result.is_err());
        assert_eq!(probes.get(), 0);
    }
    #[test]
    fn eligibility_without_human_permission_cannot_publish() {
        assert!(
            plan("🦀", Method::Auto, ClipboardPolicy::Replace, false, false).is_err()
        );
        assert!(
            plan("abc", Method::Paste, ClipboardPolicy::Replace, false, true).is_err()
        );
        assert!(plan("abc", Method::Auto, ClipboardPolicy::Replace, false, true).is_ok());
    }
    #[test]
    fn controls_require_deliberate_forced_paste_and_nul_is_never_allowed() {
        for method in [Method::Auto, Method::Keyboard, Method::Native] {
            for text in ["x\ny", "\t", "\u{85}", "\u{7f}"] {
                assert!(plan(text, method, ClipboardPolicy::Replace, true, true).is_err());
            }
        }
        assert!(plan(
            "x\ny\t",
            Method::Paste,
            ClipboardPolicy::Replace,
            true,
            false
        )
        .is_ok());
        assert!(
            plan("\0", Method::Paste, ClipboardPolicy::Replace, true, false).is_err()
        );
    }
    #[test]
    fn target_cancel_guard_prevents_every_effect() {
        let plan =
            plan("abc", Method::Paste, ClipboardPolicy::Replace, true, true).unwrap();
        let calls = Cell::new(0);
        let mut last = None;
        let result = execute_with(
            &plan,
            &mut || bail!("target lost or cancelled"),
            &mut |e| last = Some(e.clone()),
            |_| Ok(()),
            |_, _| {
                calls.set(calls.get() + 1);
                Ok(())
            },
            |_, _, _| {
                calls.set(calls.get() + 1);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(calls.get(), 0);
        let last = last.unwrap();
        assert_eq!(last.clipboard, "unchanged");
        assert!(!last.current_unit_uncertain);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod revalidation_tests {
    use super::*;
    #[test]
    fn layout_revalidation_precedes_publication() {
        let plan = TextPlan {
            keys: (),
            clipboard: Some("private".into()),
            unit_kind: "chord",
        };
        let mut last = None;
        let result = execute_with(
            &plan,
            &mut || Ok(()),
            &mut |e| last = Some(e.clone()),
            |_| bail!("layout changed"),
            |_, _| panic!("must not publish"),
            |_, _, _| panic!("must not dispatch"),
        );
        assert!(result.is_err());
        let last = last.unwrap();
        assert_eq!(last.clipboard, "unchanged");
        assert_eq!(last.completed_native_units, 0);
        assert!(!last.current_unit_uncertain);
    }
}
