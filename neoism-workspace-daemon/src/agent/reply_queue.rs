//! Per-WebSocket agent replies. Overflow invalidates the entire stream so the
//! client reconnects and reconciles from a baseline instead of missing deltas.
use neoism_protocol::agent::AgentServerMessage;
use parking_lot::Mutex;
use std::{io, sync::Arc};
use tokio::sync::{mpsc, Notify};

const MAX_MESSAGES: usize = 256;
const MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Default)]
struct State {
    messages: usize,
    bytes: usize,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    closed: Notify,
}

struct Envelope {
    message: Option<AgentServerMessage>,
    bytes: usize,
    shared: Arc<Shared>,
}

impl Drop for Envelope {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock();
        state.messages -= 1;
        state.bytes -= self.bytes;
    }
}

#[derive(Clone)]
pub struct AgentReplySender {
    tx: mpsc::UnboundedSender<Envelope>,
    shared: Arc<Shared>,
}

pub struct AgentReplyReceiver {
    rx: mpsc::UnboundedReceiver<Envelope>,
    shared: Arc<Shared>,
}

pub fn channel() -> (AgentReplySender, AgentReplyReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared::default());
    (
        AgentReplySender {
            tx,
            shared: shared.clone(),
        },
        AgentReplyReceiver { rx, shared },
    )
}

struct CountBytes(usize);
impl io::Write for CountBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("agent reply size overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl AgentReplySender {
    pub fn is_closed(&self) -> bool {
        self.shared.state.lock().closed || self.tx.is_closed()
    }

    pub fn send(
        &self,
        message: AgentServerMessage,
    ) -> Result<(), mpsc::error::SendError<AgentServerMessage>> {
        if self.is_closed() {
            return Err(mpsc::error::SendError(message));
        }
        // Count JSON without allocating a serialized copy of large histories.
        let mut count = CountBytes(0);
        let counted = serde_json::to_writer(&mut count, &message).is_ok();
        let mut state = self.shared.state.lock();
        let bytes = state.bytes.checked_add(count.0);
        // One oversized response can make progress, but only in an empty queue.
        if state.closed
            || !counted
            || state.messages >= MAX_MESSAGES
            || bytes.is_none()
            || (state.messages != 0 && bytes.unwrap() > MAX_BYTES)
        {
            state.closed = true;
            drop(state);
            self.shared.closed.notify_waiters();
            return Err(mpsc::error::SendError(message));
        }
        state.messages += 1;
        state.bytes = bytes.unwrap();
        let result = self.tx.send(Envelope {
            message: Some(message),
            bytes: count.0,
            shared: self.shared.clone(),
        });
        // Never drop an envelope while holding its accounting mutex.
        drop(state);
        result
            .map_err(|mut error| mpsc::error::SendError(error.0.message.take().unwrap()))
    }
}

impl AgentReplyReceiver {
    /// Also usable while the socket is awaiting a stalled outbound write.
    pub async fn closed(&self) {
        loop {
            let notified = self.shared.closed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.shared.state.lock().closed {
                return;
            }
            notified.await;
        }
    }

    pub async fn recv(&mut self) -> Option<AgentServerMessage> {
        let shared = self.shared.clone();
        let notified = shared.closed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if shared.state.lock().closed {
            return None;
        }
        let mut envelope = tokio::select! {
            biased;
            _ = &mut notified => return None,
            envelope = self.rx.recv() => envelope?,
        };
        if shared.state.lock().closed {
            return None;
        }
        envelope.message.take()
    }

    #[cfg(test)]
    pub fn try_recv(&mut self) -> Result<AgentServerMessage, mpsc::error::TryRecvError> {
        if self.shared.state.lock().closed {
            return Err(mpsc::error::TryRecvError::Disconnected);
        }
        let mut envelope = self.rx.try_recv()?;
        Ok(envelope.message.take().unwrap())
    }
}

impl Drop for AgentReplyReceiver {
    fn drop(&mut self) {
        self.shared.state.lock().closed = true;
        self.shared.closed.notify_waiters();
        // rx's subsequent drop releases all queued envelopes and their budgets.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(size: usize) -> AgentServerMessage {
        AgentServerMessage::SessionEvent {
            session_id: "s".into(),
            kind: "test".into(),
            properties: serde_json::Value::String("x".repeat(size)),
        }
    }

    #[tokio::test]
    async fn ordering_and_drain_release_budgets() {
        let (tx, mut rx) = channel();
        tx.send(payload(10)).unwrap();
        tx.send(AgentServerMessage::Pong).unwrap();
        assert!(matches!(
            rx.recv().await,
            Some(AgentServerMessage::SessionEvent { .. })
        ));
        assert!(matches!(rx.try_recv(), Ok(AgentServerMessage::Pong)));
        let state = tx.shared.state.lock();
        assert_eq!((state.messages, state.bytes), (0, 0));
    }

    #[tokio::test]
    async fn message_overflow_fails_all_senders_and_receiver() {
        let (tx, mut rx) = channel();
        let other = tx.clone();
        for _ in 0..MAX_MESSAGES {
            tx.send(AgentServerMessage::Pong).unwrap();
        }
        assert!(other.send(AgentServerMessage::Pong).is_err());
        assert!(tx.is_closed());
        assert!(rx.recv().await.is_none());
        assert!(tx.send(AgentServerMessage::Pong).is_err());
        drop(rx);
        let state = tx.shared.state.lock();
        assert_eq!((state.messages, state.bytes), (0, 0));
    }

    #[tokio::test]
    async fn byte_overflow_and_single_oversized_progress() {
        let (tx, mut rx) = channel();
        tx.send(payload(MAX_BYTES)).unwrap(); // JSON overhead exceeds the budget.
        assert!(tx.shared.state.lock().bytes > MAX_BYTES);
        assert!(rx.recv().await.is_some());
        assert_eq!(tx.shared.state.lock().bytes, 0);
        tx.send(payload(MAX_BYTES / 2)).unwrap();
        assert!(tx.send(payload(MAX_BYTES / 2)).is_err());
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn oversized_response_cannot_accumulate() {
        let (tx, mut rx) = channel();
        tx.send(payload(MAX_BYTES)).unwrap();
        assert!(tx.send(AgentServerMessage::Pong).is_err());
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn waiting_receiver_wakes_on_failure_and_sender_drop() {
        let (tx, mut rx) = channel();
        let waiter = tokio::spawn(async move { rx.recv().await });
        tokio::task::yield_now().await;
        // Exercise failure notification without enqueueing any message.
        tx.shared.state.lock().closed = true;
        tx.shared.closed.notify_waiters();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        let (tx, mut rx) = channel();
        drop(tx);
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn overflow_interrupts_a_stalled_socket_write() {
        let (tx, rx) = channel();
        let waiter = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = rx.closed() => {},
                _ = std::future::pending::<()>() => unreachable!(),
            }
        });
        tokio::task::yield_now().await;
        for _ in 0..MAX_MESSAGES {
            tx.send(AgentServerMessage::Pong).unwrap();
        }
        assert!(tx.send(AgentServerMessage::Pong).is_err());
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tx.shared.state.lock().messages, 0);
    }

    #[tokio::test]
    async fn concurrent_producers_share_one_budget() {
        let (tx, rx) = channel();
        let mut producers = Vec::new();
        for _ in 0..8 {
            let tx = tx.clone();
            producers.push(tokio::spawn(async move {
                for _ in 0..MAX_MESSAGES {
                    if tx.send(AgentServerMessage::Pong).is_err() {
                        break;
                    }
                }
            }));
        }
        for producer in producers {
            producer.await.unwrap();
        }
        assert!(tx.is_closed());
        assert_eq!(tx.shared.state.lock().messages, MAX_MESSAGES);
        drop(rx);
        assert_eq!(tx.shared.state.lock().bytes, 0);
    }

    #[tokio::test]
    async fn canceled_recv_does_not_consume_or_poison_queue() {
        let (tx, mut rx) = channel();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), rx.recv())
                .await
                .is_err()
        );
        tx.send(AgentServerMessage::Pong).unwrap();
        assert!(matches!(rx.recv().await, Some(AgentServerMessage::Pong)));
        assert!(!tx.is_closed());
    }

    #[tokio::test]
    async fn receiver_drop_closes_senders_and_write_waiter_is_cancel_safe() {
        let (tx, rx) = channel();
        let wait = rx.closed();
        tokio::pin!(wait);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), &mut wait)
                .await
                .is_err()
        );
        tx.shared.state.lock().closed = true;
        tx.shared.closed.notify_waiters();
        tokio::time::timeout(std::time::Duration::from_secs(1), wait)
            .await
            .unwrap();
        let (tx, rx) = channel();
        drop(rx);
        assert!(tx.is_closed());
        assert!(tx.send(AgentServerMessage::Pong).is_err());
    }
}
