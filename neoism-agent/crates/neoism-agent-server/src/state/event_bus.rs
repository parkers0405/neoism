//! Byte- and count-bounded broadcast for agent events.
//!
//! Receivers own cursors, never payload references. Overflow (including an event
//! larger than the entire budget) advances those cursors via `Lagged`, just as
//! Tokio broadcast does. Byte limits apply to estimated retained payload memory,
//! not caller-owned received clones or ring/subscriber bookkeeping.

use std::collections::{BTreeMap, VecDeque};
use std::mem::size_of;
use std::sync::{Arc, Mutex};

use neoism_agent_core::EventPayload;
use serde_json::Value;
use tokio::sync::Notify;
use tokio::sync::broadcast::error::{RecvError, SendError, TryRecvError};

pub(crate) struct EventBus {
    shared: Arc<Shared>,
}

pub struct EventReceiver {
    shared: Arc<Shared>,
    id: u64,
}

struct Shared {
    state: Mutex<State>,
    changed: Notify,
}

struct State {
    events: VecDeque<Entry>,
    bytes: usize,
    max_events: usize,
    max_bytes: usize,
    next_sequence: u64,
    next_receiver: u64,
    receivers: BTreeMap<u64, u64>,
    senders: usize,
}

struct Entry {
    sequence: u64,
    bytes: usize,
    payload: EventPayload,
}

impl EventBus {
    /// Zero limits are allowed: every published event then produces lag.
    pub(crate) fn new(max_events: usize, max_bytes: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    events: VecDeque::new(),
                    bytes: 0,
                    max_events,
                    max_bytes,
                    next_sequence: 0,
                    next_receiver: 0,
                    receivers: BTreeMap::new(),
                    senders: 1,
                }),
                changed: Notify::new(),
            }),
        }
    }

    /// Subscribe to future sends only, including when older events remain unread.
    pub(crate) fn subscribe(&self) -> EventReceiver {
        let mut state = self.shared.state.lock().unwrap();
        let id = state.next_receiver;
        state.next_receiver = id.checked_add(1).expect("event receiver ID exhausted");
        let sequence = state.next_sequence;
        state.receivers.insert(id, sequence);
        EventReceiver {
            shared: Arc::clone(&self.shared),
            id,
        }
    }

    /// Returns the subscriber count, or the original payload if none exist.
    /// Success does not guarantee delivery: overflow is reported to receivers.
    pub(crate) fn send(
        &self,
        mut payload: EventPayload,
    ) -> Result<usize, SendError<EventPayload>> {
        let mut state = self.shared.state.lock().unwrap();
        let count = state.receivers.len();
        if count == 0 {
            return Err(SendError(payload));
        }
        let sequence = state.next_sequence;
        state.next_sequence = sequence.checked_add(1).expect("event sequence exhausted");
        let bytes = payload_bytes(&payload);
        if state.max_events == 0 || bytes > state.max_bytes {
            // A gap cannot occur inside the ring: discard its prefix as well.
            // Even stalled subscribers retain no reference to the rejected event.
            state.events.clear();
            state.bytes = 0;
        } else {
            // The estimate assumes compact maps. Rebuild before retaining:
            // preserve_order's IndexMap may hide arbitrarily reserved capacity.
            // Moving members preserves string/array capacities and hence bytes.
            // Already-rejected events and sends without subscribers skip this.
            compact_objects(&mut payload.properties);
            while state.events.len() >= state.max_events
                || state.bytes > state.max_bytes - bytes
            {
                state.pop_front();
            }
            state.bytes += bytes;
            state.events.push_back(Entry {
                sequence,
                bytes,
                payload,
            });
        }
        drop(state);
        self.shared.changed.notify_waiters();
        Ok(count)
    }
}

impl Clone for EventBus {
    fn clone(&self) -> Self {
        self.shared.state.lock().unwrap().senders += 1;
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for EventBus {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.senders -= 1;
        let closed = state.senders == 0;
        drop(state);
        if closed {
            self.shared.changed.notify_waiters();
        }
    }
}

impl EventReceiver {
    /// Cancellation-safe: no cursor is advanced until an outcome is returned.
    pub async fn recv(&mut self) -> Result<EventPayload, RecvError> {
        let shared = Arc::clone(&self.shared);
        loop {
            // Register BEFORE checking the ring. notify_waiters alone does not
            // retain permits, so enabling after try_recv would lose wakeups.
            let changed = shared.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match self.try_recv() {
                Ok(payload) => return Ok(payload),
                Err(TryRecvError::Lagged(n)) => return Err(RecvError::Lagged(n)),
                Err(TryRecvError::Closed) => return Err(RecvError::Closed),
                Err(TryRecvError::Empty) => changed.await,
            }
        }
    }

    pub fn try_recv(&mut self) -> Result<EventPayload, TryRecvError> {
        let mut state = self.shared.state.lock().unwrap();
        let cursor = state.receivers[&self.id];
        let oldest = state
            .events
            .front()
            .map_or(state.next_sequence, |e| e.sequence);
        if cursor < oldest {
            state.receivers.insert(self.id, oldest);
            state.reclaim();
            return Err(TryRecvError::Lagged(oldest - cursor));
        }
        if cursor < state.next_sequence {
            let index = usize::try_from(cursor - oldest).expect("event index overflow");
            let payload = state.events[index].payload.clone();
            state.receivers.insert(self.id, cursor + 1);
            state.reclaim();
            return Ok(payload);
        }
        if state.senders == 0 {
            Err(TryRecvError::Closed)
        } else {
            Err(TryRecvError::Empty)
        }
    }
}

impl Drop for EventReceiver {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.receivers.remove(&self.id);
        state.reclaim();
    }
}

impl State {
    fn pop_front(&mut self) {
        if let Some(entry) = self.events.pop_front() {
            self.bytes -= entry.bytes;
        }
    }

    fn reclaim(&mut self) {
        let consumed = self
            .receivers
            .values()
            .copied()
            .min()
            .unwrap_or(self.next_sequence);
        while self.events.front().is_some_and(|e| e.sequence < consumed) {
            self.pop_front();
        }
    }
}

fn payload_bytes(event: &EventPayload) -> usize {
    size_of::<EventPayload>()
        // Id hides its String capacity; IDs are generated at a fixed length.
        .saturating_add(event.id.as_str().len())
        .saturating_add(event.kind.capacity())
        .saturating_add(value_heap_bytes(&event.properties))
}

/// Discard hidden object capacity, recursively, without cloning payload data.
/// Each fresh map reserves only its live member count, preserving iteration order.
/// Arrays need no rebuild because their reserved capacity is measured directly.
/// This costs O(live JSON nodes) plus one fresh allocation per nonempty object;
/// old and new map storage can briefly coexist during send, but not in the ring.
fn compact_objects(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let old = std::mem::take(map);
            let mut compact = serde_json::Map::with_capacity(old.len());
            for (key, mut value) in old {
                compact_objects(&mut value);
                compact.insert(key, value);
            }
            *map = compact;
        }
        Value::Array(values) => {
            for value in values {
                compact_objects(value);
            }
        }
        _ => {}
    }
}

/// Count reserved array/string storage, not JSON encoding length. Object storage
/// is opaque in serde_json; conservatively charge one worst-case B-tree node per
/// member (11 key/value slots plus 12 child pointers). This also covers compact
/// IndexMaps: admitted payloads MUST pass through compact_objects before retention.
fn value_heap_bytes(value: &Value) -> usize {
    match value {
        Value::String(s) => s.capacity(),
        Value::Array(values) => values.iter().fold(
            values.capacity().saturating_mul(size_of::<Value>()),
            |sum, value| sum.saturating_add(value_heap_bytes(value)),
        ),
        Value::Object(values) => {
            let node = 11usize
                .saturating_mul(size_of::<(String, Value)>())
                .saturating_add(12 * size_of::<usize>())
                .saturating_add(4 * size_of::<usize>());
            values
                .iter()
                .fold(values.len().saturating_mul(node), |sum, (key, value)| {
                    sum.saturating_add(key.capacity())
                        .saturating_add(value_heap_bytes(value))
                })
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn event(n: u64) -> EventPayload {
        EventPayload::new("test", json!(n))
    }

    fn number(event: EventPayload) -> u64 {
        event.properties.as_u64().unwrap()
    }

    #[test]
    fn exact_count_limit_and_future_only_subscription() {
        let bus = EventBus::new(2, usize::MAX);
        let mut slow = bus.subscribe();
        bus.send(event(0)).unwrap();
        let mut late = bus.subscribe();
        bus.send(event(1)).unwrap();
        assert_eq!(number(late.try_recv().unwrap()), 1);
        assert_eq!(number(slow.try_recv().unwrap()), 0);
        bus.send(event(2)).unwrap();
        bus.send(event(3)).unwrap();
        assert!(matches!(slow.try_recv(), Err(TryRecvError::Lagged(1))));
        assert_eq!(number(slow.try_recv().unwrap()), 2);
        assert_eq!(number(slow.try_recv().unwrap()), 3);
        assert_eq!(number(late.try_recv().unwrap()), 2);
        assert_eq!(number(late.try_recv().unwrap()), 3);
        let state = bus.shared.state.lock().unwrap();
        assert!(state.events.is_empty());
        assert_eq!(state.bytes, 0);
    }

    #[test]
    fn byte_overflow_does_not_lag_fast_receiver() {
        let budget = payload_bytes(&event(0)) * 2;
        let bus = EventBus::new(100, budget);
        let mut slow = bus.subscribe();
        let mut fast = bus.subscribe();
        for n in 0..10 {
            bus.send(event(n)).unwrap();
            assert_eq!(number(fast.try_recv().unwrap()), n);
            let state = bus.shared.state.lock().unwrap();
            assert!(state.bytes <= budget);
            assert_eq!(state.events.len(), (n as usize + 1).min(2));
        }
        assert!(matches!(slow.try_recv(), Err(TryRecvError::Lagged(8))));
        assert_eq!(number(slow.try_recv().unwrap()), 8);
        assert_eq!(number(slow.try_recv().unwrap()), 9);
        assert_eq!(bus.shared.state.lock().unwrap().bytes, 0);
    }

    #[test]
    fn oversize_is_not_retained_and_gap_is_reported() {
        let bus = EventBus::new(10, payload_bytes(&event(0)) * 2);
        let mut receiver = bus.subscribe();
        bus.send(event(0)).unwrap();
        bus.send(EventPayload::new("large", json!("x".repeat(8192))))
            .unwrap();
        assert_eq!(bus.shared.state.lock().unwrap().bytes, 0);
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Lagged(2))));
        bus.send(event(2)).unwrap();
        assert_eq!(number(receiver.try_recv().unwrap()), 2);
    }

    #[test]
    fn receiver_drop_reclaims_and_last_sender_closes_after_drain() {
        let bus = EventBus::new(10, usize::MAX);
        let clone = bus.clone();
        let slow = bus.subscribe();
        let mut fast = bus.subscribe();
        bus.send(event(0)).unwrap();
        fast.try_recv().unwrap();
        drop(slow);
        assert_eq!(bus.shared.state.lock().unwrap().bytes, 0);
        drop(bus);
        assert!(matches!(fast.try_recv(), Err(TryRecvError::Empty)));
        clone.send(event(1)).unwrap();
        drop(clone);
        assert_eq!(number(fast.try_recv().unwrap()), 1);
        assert!(matches!(fast.try_recv(), Err(TryRecvError::Closed)));
    }

    #[test]
    fn no_subscribers_and_zero_budget() {
        let bus = EventBus::new(0, 0);
        assert!(bus.send(event(0)).is_err());
        let mut receiver = bus.subscribe();
        bus.send(event(1)).unwrap();
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Lagged(1))));
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    }

    #[tokio::test]
    async fn wakeup_cancellation_and_close() {
        let bus = EventBus::new(10, usize::MAX);
        let mut receiver = bus.subscribe();
        // Poll recv to Pending, then cancel it without consuming anything.
        tokio::select! {
            biased;
            _ = receiver.recv() => panic!("unexpected event"),
            _ = std::future::ready(()) => {}
        }
        bus.send(event(0)).unwrap();
        assert_eq!(number(receiver.recv().await.unwrap()), 0);
        let sender = tokio::spawn(async move {
            tokio::task::yield_now().await;
            bus.send(event(1)).unwrap();
            tokio::task::yield_now().await;
            drop(bus);
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            assert_eq!(number(receiver.recv().await.unwrap()), 1);
            assert!(matches!(receiver.recv().await, Err(RecvError::Closed)));
        })
        .await
        .unwrap();
        sender.await.unwrap();
    }

    #[tokio::test]
    async fn all_waiters_wake_on_overflow_and_empty_close() {
        let bus = EventBus::new(1, 0);
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();
        let publish = async {
            tokio::task::yield_now().await;
            bus.send(event(0)).unwrap();
        };
        let (a, b, ()) = tokio::join!(first.recv(), second.recv(), publish);
        assert!(matches!(a, Err(RecvError::Lagged(1))));
        assert!(matches!(b, Err(RecvError::Lagged(1))));
        let close = async move {
            tokio::task::yield_now().await;
            drop(bus);
        };
        let (a, b, ()) = tokio::join!(first.recv(), second.recv(), close);
        assert!(matches!(a, Err(RecvError::Closed)));
        assert!(matches!(b, Err(RecvError::Closed)));
    }

    #[test]
    fn sparse_object_storage_is_rebuilt_before_retention() {
        let mut sparse = serde_json::Map::with_capacity(100_000);
        sparse.insert("z-last".into(), json!(1));
        sparse.insert("a-first".into(), json!(2));
        let original_slot = &sparse["z-last"] as *const Value as usize;
        let expected_order: Vec<_> = sparse.keys().cloned().collect();
        let expected_json = serde_json::to_string(&sparse).unwrap();
        // Nest below an array and another object to exercise recursive compaction.
        let mut payload = EventPayload::new("sparse", json!({"items": []}));
        payload.properties["items"] = Value::Array(vec![Value::Object(sparse)]);
        let budget = payload_bytes(&payload);
        let bus = EventBus::new(4, budget);
        let mut receiver = bus.subscribe();
        bus.send(payload).unwrap();
        {
            let state = bus.shared.state.lock().unwrap();
            assert_eq!(state.bytes, budget);
            let retained = state.events[0].payload.properties["items"][0]
                .as_object()
                .unwrap();
            // Fresh storage was allocated while the old map was still alive;
            // merely charging len() while retaining the sparse map fails this.
            assert_ne!(&retained["z-last"] as *const Value as usize, original_slot);
            assert_eq!(retained.keys().cloned().collect::<Vec<_>>(), expected_order);
            assert_eq!(serde_json::to_string(retained).unwrap(), expected_json);
        }
        assert_eq!(
            receiver.try_recv().unwrap().properties["items"][0]["z-last"],
            1
        );
        assert_eq!(bus.shared.state.lock().unwrap().bytes, 0);
    }

    #[test]
    fn estimator_counts_reserved_capacity_recursively() {
        let mut text = String::with_capacity(4096);
        text.push('x');
        let mut values = Vec::with_capacity(32);
        values.push(Value::String(text));
        assert!(
            value_heap_bytes(&Value::Array(values)) >= 4096 + 32 * size_of::<Value>()
        );
        assert!(value_heap_bytes(&json!({"nested": {"text": "abc"}})) > 6);
    }
}
