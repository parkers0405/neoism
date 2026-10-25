//! Owner-scoped asynchronous delivery. The host supplies execution and wakeup;
//! this module owns cancellation and rejects results after component teardown.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, mpsc};
use std::task::{Context, Poll, Waker};

use crate::{Component, ComponentContext, ComponentError, ComponentHandle, ComponentHost};

#[derive(Default)]
struct Cancellation { cancelled: AtomicBool, waker: Mutex<Option<Waker>> }

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<Cancellation>);

impl CancellationToken {
    pub fn new() -> Self { Self::default() }
    pub fn is_cancelled(&self) -> bool { self.0.cancelled.load(Ordering::Acquire) }
    pub fn cancel(&self) {
        if !self.0.cancelled.swap(true, Ordering::AcqRel) {
            let waker = self.0.waker.lock().unwrap_or_else(|error| error.into_inner()).take();
            if let Some(waker) = waker { waker.wake(); }
        }
    }
    pub fn bind<F: Future>(&self, future: F) -> CancelledFuture<F> {
        CancelledFuture { future: Box::pin(future), token: self.clone() }
    }
}

/// Cancellation drops the underlying future on its next poll; an executor is
/// woken when an owner is removed, so a pending future does not leak indefinitely.
pub struct CancelledFuture<F: Future> { future: Pin<Box<F>>, token: CancellationToken }

impl<F: Future> Future for CancelledFuture<F> {
    type Output = Option<F::Output>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.token.is_cancelled() { return Poll::Ready(None); }
        {
            let mut waker = self.token.0.waker.lock().unwrap_or_else(|error| error.into_inner());
            if waker.as_ref().is_none_or(|old| !old.will_wake(cx.waker())) { *waker = Some(cx.waker().clone()); }
        }
        if self.token.is_cancelled() { return Poll::Ready(None); }
        self.future.as_mut().poll(cx).map(Some)
    }
}

/// Execution is supplied by the application (Tokio, a native worker pool, etc.).
/// The core does not create threads, own an event loop, or poll idle tasks.
pub trait TaskExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>);
}

pub trait LocalTaskExecutor {
    fn spawn_local(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>);
}

trait Pending<Action: Clone + 'static>: Send {
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any + Send>;
    fn apply(self: Box<Self>, host: &mut ComponentHost<Action>) -> Result<Vec<Action>, ComponentError>;
}
struct Message<C: Component> { owner: ComponentHandle<C>, event: C::Event, cancellation: CancellationToken }
impl<C: Component> Pending<C::Action> for Message<C> {
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any + Send> { self }
    fn apply(self: Box<Self>, host: &mut ComponentHost<C::Action>) -> Result<Vec<C::Action>, ComponentError> {
        if self.cancellation.is_cancelled() { return Ok(Vec::new()); }
        host.send(self.owner, self.event)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryError { OwnerCancelled, HostClosed, QueueFull }

/// Cloneable weak event endpoint. A component's cleanup cancels it, and the host
/// generation check independently protects a reused owner slot.
pub struct EventSender<C: Component> {
    owner: ComponentHandle<C>,
    sender: mpsc::SyncSender<Box<dyn Pending<C::Action>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    cancellation: CancellationToken,
    waiting: Arc<Mutex<Vec<Waker>>>,
}

impl<C: Component> Clone for EventSender<C> {
    fn clone(&self) -> Self { Self { owner: self.owner, sender: self.sender.clone(),
        wake: self.wake.clone(), cancellation: self.cancellation.clone(), waiting: self.waiting.clone() } }
}

impl<C: Component> EventSender<C> {
    pub fn cancellation(&self) -> CancellationToken { self.cancellation.clone() }
    /// Delivery never blocks the UI or a worker. A saturated mailbox returns the
    /// event to the caller, which can coalesce or retry instead of losing it.
    pub fn try_send(&self, event: C::Event) -> Result<(), SendError<C::Event>> {
        if self.cancellation.is_cancelled() { return Err(SendError::Cancelled(event)); }
        let message: Box<dyn Pending<C::Action>> = Box::new(Message::<C> {
            owner: self.owner, event, cancellation: self.cancellation.clone(),
        });
        match self.sender.try_send(message) {
            Ok(()) => { (self.wake)(); Ok(()) }
            Err(mpsc::TrySendError::Full(message)) => {
                let message = message.into_any().downcast::<Message<C>>()
                    .unwrap_or_else(|_| unreachable!("sender message type is fixed"));
                Err(SendError::Full(message.event))
            }
            Err(mpsc::TrySendError::Disconnected(message)) => {
                let message = message.into_any().downcast::<Message<C>>()
                    .unwrap_or_else(|_| unreachable!("sender message type is fixed"));
                Err(SendError::Closed(message.event))
            }
        }
    }
    /// The queue is bounded. This method reports pressure rather than blocking;
    /// callers needing retry retain the returned PendingDelivery.
    pub fn send(&self, event: C::Event) -> Result<(), DeliveryError> {
        self.try_send(event).map_err(|error| match error {
            SendError::Cancelled(_) => DeliveryError::OwnerCancelled,
            SendError::Full(_) => DeliveryError::QueueFull,
            SendError::Closed(_) => DeliveryError::HostClosed,
        })
    }
    pub fn send_async(&self, event: C::Event) -> SendFuture<C> {
        SendFuture { sender: self.clone(), event: Some(Box::new(event)) }
    }
}

pub enum SendError<Event> {
    Cancelled(Event), Full(Event), Closed(Event),
}

/// Backpressure waits on a drain notification, not a timer or a busy loop.
pub struct SendFuture<C: Component> {
    sender: EventSender<C>,
    event: Option<Box<C::Event>>,
}
impl<C: Component> Unpin for SendFuture<C> {}
impl<C: Component> Future for SendFuture<C> {
    type Output = Result<(), DeliveryError>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(event) = this.event.take() else { return Poll::Ready(Ok(())); };
        // Register before testing capacity so a concurrent drain cannot race the
        // transition to Pending. The task token also wakes on owner teardown.
        {
            let mut waiting = this.sender.waiting.lock().unwrap_or_else(|error| error.into_inner());
            if !waiting.iter().any(|old| old.will_wake(cx.waker())) { waiting.push(cx.waker().clone()); }
        }
        {
            let mut waker = this.sender.cancellation.0.waker.lock().unwrap_or_else(|error| error.into_inner());
            *waker = Some(cx.waker().clone());
        }
        match this.sender.try_send(*event) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(SendError::Full(event)) => { this.event = Some(Box::new(event)); Poll::Pending }
            Err(SendError::Cancelled(_)) => Poll::Ready(Err(DeliveryError::OwnerCancelled)),
            Err(SendError::Closed(_)) => Poll::Ready(Err(DeliveryError::HostClosed)),
        }
    }
}

#[derive(Clone)]
pub struct EventQueue<Action: Clone + 'static> {
    sender: mpsc::SyncSender<Box<dyn Pending<Action>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    waiting: Arc<Mutex<Vec<Waker>>>,
}
impl<Action: Clone + 'static> EventQueue<Action> {
    pub fn sender<C: Component<Action = Action>>(&self, owner: ComponentHandle<C>) -> EventSender<C> {
        EventSender { owner, sender: self.sender.clone(), wake: self.wake.clone(),
            cancellation: CancellationToken::new(), waiting: self.waiting.clone() }
    }
}

/// Main-thread mailbox paired with a host's redraw/wakeup mechanism.
pub struct Mailbox<Action: Clone + 'static> {
    sender: mpsc::SyncSender<Box<dyn Pending<Action>>>,
    receiver: Option<mpsc::Receiver<Box<dyn Pending<Action>>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    waiting: Arc<Mutex<Vec<Waker>>>,
}

impl<Action: Clone + 'static> Mailbox<Action> {
    pub fn new(capacity: usize, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        Self { sender, receiver: Some(receiver), wake: Arc::new(wake), waiting: Arc::default() }
    }
    pub fn sender<C: Component<Action = Action>>(&self, owner: ComponentHandle<C>) -> EventSender<C> {
        EventSender { owner, sender: self.sender.clone(), wake: self.wake.clone(), cancellation: CancellationToken::new(), waiting: self.waiting.clone() }
    }
    /// A finite drain prevents a producer from starving input/presentation.
    pub fn drain(&mut self, host: &mut ComponentHost<Action>, limit: usize) -> MailboxDelivery<Action> {
        let mut delivery = MailboxDelivery { actions: Vec::new(), stale: 0, processed: 0, errors: Vec::new() };
        for _ in 0..limit {
            let Some(message) = self.receiver.as_ref().and_then(|receiver| receiver.try_recv().ok()) else { break; };
            delivery.processed += 1;
            match message.apply(host) {
                Ok(actions) => delivery.actions.extend(actions),
                Err(ComponentError::StaleHandle | ComponentError::WrongHost) => delivery.stale += 1,
                Err(error) => delivery.errors.push(error),
            }
        }
        if delivery.processed != 0 {
            let waiting = std::mem::take(&mut *self.waiting.lock().unwrap_or_else(|error| error.into_inner()));
            for waker in waiting { waker.wake(); }
        }
        if delivery.processed == limit && limit != 0 { (self.wake)(); }
        delivery
    }
}

impl<Action: Clone + 'static> Drop for Mailbox<Action> {
    fn drop(&mut self) {
        drop(self.receiver.take());
        let waiting = std::mem::take(&mut *self.waiting.lock().unwrap_or_else(|error| error.into_inner()));
        for waker in waiting { waker.wake(); }
    }
}

pub struct MailboxDelivery<Action> {
    pub actions: Vec<Action>, pub processed: usize, pub stale: usize, pub errors: Vec<ComponentError>,
}

impl<Action: Clone + 'static> ComponentContext<'_, Action> {
    /// Subscription endpoints and tasks are tied to the current owner, not the
    /// lifetime of a copied weak handle or an external worker.
    pub fn event_sender<C: Component<Action = Action>>(&mut self, mailbox: &Mailbox<Action>) -> Result<EventSender<C>, ComponentError> {
        let sender = mailbox.sender(self.handle::<C>()?);
        let token = sender.cancellation(); self.own(move || token.cancel());
        Ok(sender)
    }
    pub fn spawn<C, F>(&mut self, mailbox: &Mailbox<Action>, executor: &impl TaskExecutor, future: F) -> Result<CancellationToken, ComponentError>
    where C: Component<Action = Action>, F: Future<Output = C::Event> + Send + 'static {
        let sender = self.event_sender::<C>(mailbox)?;
        let token = sender.cancellation();
        let task_token = token.clone();
        executor.spawn(Box::pin(async move {
            if let Some(event) = task_token.bind(future).await { let _ = sender.send_async(event).await; }
        }));
        Ok(token)
    }
    pub fn spawn_local<C, F>(&mut self, mailbox: &Mailbox<Action>, executor: &impl LocalTaskExecutor, future: F) -> Result<CancellationToken, ComponentError>
    where C: Component<Action = Action>, F: Future<Output = C::Event> + 'static {
        let sender = self.event_sender::<C>(mailbox)?;
        let token = sender.cancellation();
        let task_token = token.clone();
        executor.spawn_local(Box::pin(async move {
            if let Some(event) = task_token.bind(future).await { let _ = sender.send_async(event).await; }
        }));
        Ok(token)
    }
}
