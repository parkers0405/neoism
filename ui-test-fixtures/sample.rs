//! Inert Rust fixture for patch rendering tests.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchSample {
    label: &'static str,
    line_count: usize,
    state: PreviewState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewState {
    Applied,
    Archived,
}

impl PreviewState {
    fn label(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Archived => "archived",
        }
    }

    fn is_terminal(self) -> bool {
        matches!(self, Self::Archived)
    }

    fn next(self) -> Self {
        match self {
            Self::Applied => Self::Archived,
            Self::Archived => Self::Archived,
        }
    }
}

impl fmt::Display for PreviewState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

fn sample() -> PatchSample {
    PatchSample {
        label: "diff preview",
        line_count: 24,
        state: PreviewState::Applied,
    }
}

fn archive(mut sample: PatchSample) -> PatchSample {
    sample.state = sample.state.next();
    sample
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_has_a_label() {
        assert_eq!(sample().label, "diff preview");
        assert_eq!(sample().state, PreviewState::Applied);
        assert_eq!(sample().state.label(), "applied");
        assert!(!sample().state.is_terminal());
        assert!(archive(sample()).state.is_terminal());
        assert_eq!(archive(sample()).state.to_string(), "archived");
    }
}
