use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const RANDOM_SUFFIX_LEN: usize = 14;
const BASE62: &[u8; 62] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdKind {
    Account,
    Artifact,
    Audit,
    Entry,
    Event,
    Message,
    Part,
    Permission,
    Pty,
    Question,
    Session,
    Tool,
    User,
    Workspace,
}

impl IdKind {
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Account => "act",
            Self::Artifact => "art",
            Self::Audit => "aud",
            Self::Entry => "ent",
            Self::Event => "evt",
            Self::Message => "msg",
            Self::Part => "prt",
            Self::Permission => "per",
            Self::Pty => "pty",
            Self::Question => "que",
            Self::Session => "ses",
            Self::Tool => "tool",
            Self::User => "usr",
            Self::Workspace => "wrk",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Error)]
pub enum IdError {
    #[error("ID {id} does not start with {prefix}")]
    InvalidPrefix { id: String, prefix: &'static str },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Id(String);

impl Id {
    pub fn new(kind: IdKind, direction: IdDirection) -> Self {
        Self(create(kind.prefix(), direction, None))
    }

    pub fn ascending(kind: IdKind) -> Self {
        Self::new(kind, IdDirection::Ascending)
    }

    pub fn descending(kind: IdKind) -> Self {
        Self::new(kind, IdDirection::Descending)
    }

    pub fn parse(kind: IdKind, value: impl Into<String>) -> Result<Self, IdError> {
        let value = value.into();
        let prefix = kind.prefix();
        if !value.starts_with(prefix) {
            return Err(IdError::InvalidPrefix { id: value, prefix });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<Id> for String {
    fn from(value: Id) -> Self {
        value.0
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Default)]
struct MonotonicState {
    last_timestamp: u64,
    counter: u16,
}

static STATE: OnceLock<Mutex<MonotonicState>> = OnceLock::new();

fn timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn next_time_value(timestamp: u64, direction: IdDirection) -> u64 {
    let mut state = STATE
        .get_or_init(|| Mutex::new(MonotonicState::default()))
        .lock()
        .expect("id monotonic state poisoned");

    if state.last_timestamp != timestamp {
        state.last_timestamp = timestamp;
        state.counter = 0;
    }
    state.counter = state.counter.wrapping_add(1);

    let encoded = timestamp
        .saturating_mul(0x1000)
        .saturating_add(u64::from(state.counter));
    match direction {
        IdDirection::Ascending => encoded & 0x0000_ffff_ffff_ffff,
        IdDirection::Descending => !encoded & 0x0000_ffff_ffff_ffff,
    }
}

fn random_base62(length: usize) -> String {
    let mut bytes = vec![0u8; length];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
        .into_iter()
        .map(|byte| BASE62[usize::from(byte) % BASE62.len()] as char)
        .collect()
}

pub fn create(prefix: &str, direction: IdDirection, timestamp: Option<u64>) -> String {
    let time = next_time_value(timestamp.unwrap_or_else(timestamp_millis), direction);
    format!("{prefix}_{time:012x}{}", random_base62(RANDOM_SUFFIX_LEN))
}

pub fn timestamp(id: &str) -> Option<u64> {
    let (_, suffix) = id.split_once('_')?;
    let time_hex = suffix.get(..12)?;
    u64::from_str_radix(time_hex, 16)
        .ok()
        .map(|value| value / 0x1000)
}

pub type AccountId = Id;
pub type EntryId = Id;
pub type EventId = Id;
pub type MessageId = Id;
pub type PartId = Id;
pub type PermissionId = Id;
pub type PtyId = Id;
pub type QuestionId = Id;
pub type SessionId = Id;
pub type ToolId = Id;
pub type UserId = Id;
pub type WorkspaceId = Id;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_expected_prefixed_shape() {
        let timestamp = 1_700_000_000_000;
        let id = create("ses", IdDirection::Ascending, Some(timestamp));
        assert!(id.starts_with("ses_"));
        assert_eq!(id.len(), "ses_".len() + 26);
        let encoded = (timestamp * 0x1000 + 1) & 0x0000_ffff_ffff_ffff;
        assert_eq!(crate::id::timestamp(&id), Some(encoded / 0x1000));
    }

    #[test]
    fn validates_prefix() {
        let err = Id::parse(IdKind::Session, "msg_abc").unwrap_err();
        assert!(err.to_string().contains("ses"));
    }
}
