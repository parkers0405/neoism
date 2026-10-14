// Standalone fixture for testing the agent UI's file-write and edit events.
// Not imported by the application.
pub fn ui_test_label(step: u32) -> String {
    let status = if step == 0 { "pending" } else { "ready" };
    format!("UI edit test: step {step} [{status}]")
}

#[cfg(test)]
mod tests {
    use super::ui_test_label;

    #[test]
    fn includes_step_number() {
        assert!(ui_test_label(2).contains("step 2"));
    }

    #[test]
    fn zero_step_is_pending() {
        assert_eq!(ui_test_label(0), "UI edit test: step 0 [pending]");
    }

    #[test]
    fn started_step_is_ready() {
        assert_eq!(ui_test_label(2), "UI edit test: step 2 [ready]");
    }
}
