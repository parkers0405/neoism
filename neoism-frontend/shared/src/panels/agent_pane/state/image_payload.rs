//! Immutable attachment URLs with content identities computed once at ingestion.
use sha2::{Digest, Sha256};
use std::{
    hash::{Hash, Hasher},
    ops::Deref,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
};

/// Cheaply cloned original URL with a SHA-256 identity computed at ingestion.
/// Private storage prevents mutations from leaving a stale render/measure key.
#[derive(Clone, Debug)]
pub struct AgentImagePayload(Arc<ImagePayload>);

#[derive(Debug)]
struct ImagePayload {
    url: String,
    identity: [u8; 32],
    // Shared by composer/card/modal clones and reclaimed with the payload.
    decode_failed: AtomicBool,
    renderer_id: OnceLock<Option<u32>>,
}

impl AgentImagePayload {
    pub(crate) fn decode_failed(&self) -> bool {
        self.0.decode_failed.load(Ordering::Relaxed)
    }
    pub(crate) fn mark_decode_failed(&self) {
        self.0.decode_failed.store(true, Ordering::Relaxed);
    }
    pub(crate) fn renderer_id(&self, allocate: impl FnOnce([u8; 32]) -> Option<u32>) -> Option<u32> {
        *self.0.renderer_id.get_or_init(|| allocate(self.0.identity))
    }
    pub fn cache_identity(&self) -> [u8; 32] {
        self.0.identity
    }
    pub fn as_str(&self) -> &str {
        &self.0.url
    }
}

impl From<String> for AgentImagePayload {
    fn from(url: String) -> Self {
        let identity = Sha256::digest(url.as_bytes()).into();
        Self(Arc::new(ImagePayload {
            url,
            identity,
            decode_failed: AtomicBool::new(false),
            renderer_id: OnceLock::new(),
        }))
    }
}
impl From<&str> for AgentImagePayload {
    fn from(url: &str) -> Self {
        url.to_owned().into()
    }
}
impl Deref for AgentImagePayload {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}
impl PartialEq for AgentImagePayload {
    fn eq(&self, other: &Self) -> bool {
        self.0.identity == other.0.identity
    }
}
impl Eq for AgentImagePayload {}
impl Hash for AgentImagePayload {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.identity.hash(state);
    }
}
// Keep prompt/file wire parts as URL strings, not wrapper objects.
impl serde::Serialize for AgentImagePayload {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_original_payload_and_identity_is_content_based() {
        let original = format!("data:image/png;base64,{}", "A".repeat(1024 * 1024));
        let payload = AgentImagePayload::from(original.clone());
        let cloned = payload.clone();
        assert!(Arc::ptr_eq(&payload.0, &cloned.0));
        assert_eq!(payload.as_str(), original);
        assert_eq!(payload, AgentImagePayload::from(original.clone()));
        assert_ne!(payload, AgentImagePayload::from(format!("{original}B")));
        assert_eq!(serde_json::to_value(&payload).unwrap(), original);
    }

    #[test]
    fn warmed_images_and_preview_clones_do_not_reenter_the_id_registry() {
        let payload = AgentImagePayload::from("data:image/png;base64,AA==");
        let clone = payload.clone();
        assert_eq!(payload.renderer_id(|_| Some(42)), Some(42));
        for _ in 0..1_000 {
            assert_eq!(clone.renderer_id(|_| panic!("warm paint must not lock the registry")), Some(42));
        }
        let replacement = AgentImagePayload::from("data:image/png;base64,AQ==");
        assert_eq!(replacement.renderer_id(|_| Some(43)), Some(43));
    }

    #[test]
    fn hashing_only_visits_the_fixed_size_identity() {
        #[derive(Default)]
        struct CountingHasher(usize);
        impl Hasher for CountingHasher {
            fn finish(&self) -> u64 {
                self.0 as u64
            }
            fn write(&mut self, bytes: &[u8]) {
                self.0 += bytes.len();
            }
        }
        let payload = AgentImagePayload::from("A".repeat(1024 * 1024));
        let mut hasher = CountingHasher::default();
        payload.hash(&mut hasher);
        assert!(hasher.0 <= 40, "hashed {} bytes", hasher.0);
        let image = super::super::NeoismAgentImage {
            filename: "shot.png".into(),
            mime: "image/png".into(),
            url: payload,
        };
        let mut hasher = CountingHasher::default();
        image.hash(&mut hasher);
        assert!(
            hasher.0 < 128,
            "derived image Hash visited {} bytes",
            hasher.0
        );
    }

    #[test]
    fn composer_snapshots_share_the_attached_bytes() {
        let mut pane = super::super::NeoismAgentPane::default();
        assert!(pane.attach_clipboard_image("shot.png", "image/png", b"original bytes"));
        let first = pane.input_images();
        let second = pane.input_images();
        assert!(Arc::ptr_eq(&first[0].url.0, &second[0].url.0));
        assert_eq!(
            first[0].url.cache_identity(),
            second[0].url.cache_identity()
        );
    }
}
