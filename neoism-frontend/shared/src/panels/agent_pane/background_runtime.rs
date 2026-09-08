//! Process-local job authority, independent of transcript and execution revisions.
use std::collections::{BTreeSet, HashSet};

#[derive(Clone, Debug, Default)]
pub struct BackgroundTaskAuthority {
    epoch: Option<String>,
    revision: u64,
    retired_epochs: HashSet<String>,
    job_ids: BTreeSet<String>,
}

impl BackgroundTaskAuthority {
    pub fn is_authoritative(&self) -> bool {
        self.epoch.is_some()
    }

    /// Both SSE full lists and REST snapshots use this revision domain. Empty
    /// lists are authoritative too. Once an epoch is replaced, a delayed fetch
    /// from that server must never switch us back to it.
    pub fn apply(&mut self, epoch: &str, revision: u64, tasks: &[(String, u64)]) -> bool {
        if epoch.is_empty()
            || self.retired_epochs.contains(epoch)
            || (self.epoch.as_deref() == Some(epoch) && revision <= self.revision)
        {
            return false;
        }
        if self.epoch.as_deref() != Some(epoch) {
            if let Some(old) = self.epoch.replace(epoch.to_owned()) {
                self.retired_epochs.insert(old);
            }
        }
        self.revision = revision;
        self.job_ids = tasks.iter().map(|(id, _)| id.clone()).collect();
        true
    }

    pub fn job_ids(&self) -> &BTreeSet<String> {
        &self.job_ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_authority_empty_reconnect_and_retired_epoch() {
        let mut state = BackgroundTaskAuthority::default();
        let jobs = [("job".into(), 100)];
        assert!(state.apply("a", 10, &jobs));
        assert!(state.apply("a", 11, &[]));
        assert!(state.is_authoritative());
        assert!(!state.apply("a", 10, &jobs));
        assert!(!state.apply("a", 11, &jobs));
        assert!(state.job_ids().is_empty());
        assert!(state.apply("b", 0, &[]));
        assert!(!state.apply("a", 12, &jobs));
        assert!(state.apply("b", 1, &jobs));
        assert_eq!(state.job_ids().len(), 1);
    }
}
