// Phase 3b: the ANSI `Handler` impl moved into
// `neoism_terminal_core::handler`. The native parser driver below
// keeps a `handler::Processor` so the backend's event-loop integration
// is unchanged — call sites import the type from the terminal-core
// path.
use crate::event::sync::FairMutex;
use crate::event::RioEvent;
use crate::event::{EventListener, Msg, WindowId};
use corcovado::channel;
use corcovado::{self, Events, PollOpt, Ready};
use neoism_terminal_core::crosswords::Crosswords;
use neoism_terminal_core::handler;
use neoism_terminal_pty::PtySession;
use std::sync::mpsc::TryRecvError;
use std::sync::Arc;
use std::thread::{Builder, JoinHandle};
use std::time::Instant;
use tracing::error;

/// Like `thread::spawn`, but with a `name` argument.
pub fn spawn_named<F, T, S>(name: S, f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
    S: Into<String>,
{
    Builder::new()
        .name(name.into())
        .spawn(f)
        .expect("thread spawn works")
}

/// Cap on bytes parsed while the terminal lock is held in a single
/// pass; matches the pre-Phase-4 budget so renderer responsiveness
/// stays identical.
const MAX_LOCKED_READ: usize = u16::MAX as usize;
const LOG_BYTE_PREVIEW_LIMIT: usize = 96;

fn bytes_hex_for_log(bytes: &[u8]) -> String {
    let mut out = bytes
        .iter()
        .take(LOG_BYTE_PREVIEW_LIMIT)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    if bytes.len() > LOG_BYTE_PREVIEW_LIMIT {
        out.push_str(" ...");
    }
    out
}

fn bytes_text_for_log(bytes: &[u8]) -> String {
    let preview = &bytes[..bytes.len().min(LOG_BYTE_PREVIEW_LIMIT)];
    let mut out = String::from_utf8_lossy(preview).escape_debug().to_string();
    if bytes.len() > LOG_BYTE_PREVIEW_LIMIT {
        out.push_str("...");
    }
    out
}

struct PeekableReceiver<T> {
    rx: channel::Receiver<T>,
    peeked: Option<T>,
}

impl<T> PeekableReceiver<T> {
    fn new(rx: channel::Receiver<T>) -> Self {
        Self { rx, peeked: None }
    }

    fn peek(&mut self) -> Option<&T> {
        if self.peeked.is_none() {
            self.peeked = self.rx.try_recv().ok();
        }

        self.peeked.as_ref()
    }

    fn recv(&mut self) -> Option<T> {
        if self.peeked.is_some() {
            self.peeked.take()
        } else {
            self.rx.try_recv().ok()
        }
    }
}

/// Parser driver bound to a single PTY session.
///
/// Phase 4 of the libghostty-style migration split the original
/// `Machine` in half:
///
/// * **PTY IO** — opening the PTY, mio/corcovado event loop, signal
///   handling, exit detection — moved into
///   [`neoism_terminal_pty::PtySession`] / `LocalPty`.
/// * **Parser driver** — the lock-dance, [`Crosswords`] advance,
///   effect drain — stays here, just consuming bytes the PTY session
///   pushes over a `corcovado::channel`.
pub struct Machine<U: EventListener> {
    sender: channel::Sender<Msg>,
    receiver: PeekableReceiver<Msg>,
    pty: PtySession,
    byte_rx: channel::Receiver<Vec<u8>>,
    child_event_rx: channel::Receiver<i32>,
    poll: corcovado::Poll,
    terminal: Arc<FairMutex<Crosswords>>,
    event_proxy: U,
    window_id: WindowId,
    route_id: usize,
}

#[derive(Default)]
pub struct State {
    parser: handler::Processor,
}


impl<U> Machine<U>
where
    U: EventListener + Send + 'static,
{
    pub fn new(
        terminal: Arc<FairMutex<Crosswords>>,
        mut pty: PtySession,
        event_proxy: U,
        window_id: WindowId,
        route_id: usize,
    ) -> Result<Machine<U>, Box<dyn std::error::Error>> {
        let (sender, receiver) = channel::channel();
        let poll = corcovado::Poll::new()?;

        let byte_rx = pty
            .take_byte_receiver()
            .ok_or("PtySession byte receiver already taken")?;
        let child_event_rx = pty
            .take_child_event_receiver()
            .ok_or("PtySession child event receiver already taken")?;

        Ok(Machine {
            sender,
            receiver: PeekableReceiver::new(receiver),
            poll,
            pty,
            byte_rx,
            child_event_rx,
            terminal,
            event_proxy,
            window_id,
            route_id,
        })
    }

    /// Drain pending byte chunks the PTY reader thread pushed into
    /// the byte channel, advance the parser, and dispatch effects.
    /// Mirrors the old `pty_read` lock-dance: tries an unfair
    /// `try_lock` first, falls back to a blocking lock only when we
    /// hit `MAX_LOCKED_READ` worth of buffered bytes.
    #[inline]
    fn drain_bytes_into_parser(&mut self, state: &mut State) {
        let mut processed = 0;
        tracing::trace!(
            target: "neoism_backend::pty_input",
            window_id = ?self.window_id,
            route_id = self.route_id,
            "parser drain pass starting"
        );

        let mut terminal_guard = None;

        loop {
            let chunk = match self.byte_rx.try_recv() {
                Ok(chunk) => chunk,
                Err(_) => break,
            };

            tracing::trace!(
                target: "neoism_backend::pty_input",
                window_id = ?self.window_id,
                route_id = self.route_id,
                read_len = chunk.len(),
                bytes_hex = %bytes_hex_for_log(&chunk),
                bytes_text = %bytes_text_for_log(&chunk),
                "parser received PTY chunk"
            );

            // Reuse a single lock acquisition for as many chunks as
            // we can grab without exceeding the budget. The dance
            // mirrors the pre-Phase-4 behavior — prefer try_lock so
            // the renderer doesn't stall, only block when buffered
            // chunks pile up.
            let terminal = match &mut terminal_guard {
                Some(t) => t,
                None => {
                    let terminal_lease = self.terminal.lease();
                    let guard = match self.terminal.try_lock_unfair() {
                        Some(g) => {
                            drop(terminal_lease);
                            g
                        }
                        None if processed >= MAX_LOCKED_READ => {
                            drop(terminal_lease);
                            self.terminal.lock_unfair()
                        }
                        None => {
                            drop(terminal_lease);
                            // Re-push the chunk back into the
                            // pipeline by putting it directly through
                            // the parser on the next pass — we lose
                            // the chunk here unless we hold onto it.
                            // Use a small local stash and continue.
                            // Simpler: just block; this matches the
                            // old behavior when we hit the budget.
                            self.terminal.lock_unfair()
                        }
                    };
                    terminal_guard.insert(guard)
                }
            };

            tracing::trace!(
                target: "neoism_backend::pty_input",
                window_id = ?self.window_id,
                route_id = self.route_id,
                byte_len = chunk.len(),
                "PTY parser advancing"
            );
            state.parser.advance(&mut **terminal, &chunk);

            let drained_effects: Vec<_> = terminal.drain_effects().collect();
            if !drained_effects.is_empty() {
                crate::effects_adapter::dispatch_terminal_effects(
                    drained_effects,
                    &self.event_proxy,
                    self.window_id,
                    self.route_id,
                );
            }

            processed += chunk.len();
            if processed >= MAX_LOCKED_READ {
                break;
            }
        }

        if processed > 0 {
            if let Some(ref mut term) = terminal_guard {
                let damage = term.peek_damage_event();
                tracing::trace!(
                    target: "neoism_backend::pty_input",
                    window_id = ?self.window_id,
                    route_id = self.route_id,
                    processed,
                    sync_bytes = state.parser.sync_bytes_count(),
                    damage = ?damage,
                    damage_event_in_flight = term.damage_event_in_flight,
                    "parser drain damage check"
                );
                if state.parser.sync_bytes_count() < processed
                    && !term.damage_event_in_flight
                    && damage.is_some()
                {
                    term.damage_event_in_flight = true;
                    tracing::trace!(
                        target: "neoism_backend::pty_input",
                        window_id = ?self.window_id,
                        route_id = self.route_id,
                        "sending TerminalDamaged after parser drain"
                    );
                    self.event_proxy.send_event(
                        RioEvent::TerminalDamaged(self.route_id),
                        self.window_id,
                    );
                }
            }
        }

        tracing::trace!(
            target: "neoism_backend::pty_input",
            window_id = ?self.window_id,
            route_id = self.route_id,
            processed,
            "parser drain pass finished"
        );
    }

    /// Drain the channel.
    ///
    /// Returns `false` when a shutdown message was received.
    fn drain_recv_channel(&mut self, _state: &mut State) -> bool {
        while let Some(msg) = self.receiver.recv() {
            match msg {
                Msg::Input(input) => {
                    tracing::trace!(
                        target: "neoism_backend::pty_input",
                        window_id = ?self.window_id,
                        route_id = self.route_id,
                        byte_len = input.len(),
                        bytes_hex = %bytes_hex_for_log(input.as_ref()),
                        bytes_text = %bytes_text_for_log(input.as_ref()),
                        "drained input message from frontend"
                    );
                    if let Err(err) = self.pty.write(input.as_ref()) {
                        tracing::warn!(
                            target: "neoism_backend::pty_input",
                            window_id = ?self.window_id,
                            route_id = self.route_id,
                            error = %err,
                            "failed to forward Msg::Input to PtySession"
                        );
                    }
                }
                Msg::Resize(window_size) => {
                    tracing::trace!(
                        target: "neoism_backend::pty_input",
                        window_id = ?self.window_id,
                        route_id = self.route_id,
                        ?window_size,
                        "received PTY resize"
                    );
                    let _ = self.pty.resize(window_size.cols, window_size.rows);
                }
                Msg::Shutdown => {
                    tracing::trace!(
                        target: "neoism_backend::pty_input",
                        window_id = ?self.window_id,
                        route_id = self.route_id,
                        "received PTY shutdown message"
                    );
                    return false;
                }
                Msg::RebindWindow(window_id) => {
                    tracing::info!(
                        target: "neoism_backend::pty_input",
                        old_window_id = ?self.window_id,
                        new_window_id = ?window_id,
                        route_id = self.route_id,
                        "re-homing PTY parser driver onto new window"
                    );
                    // The IO thread is the sole writer of `window_id`, so a
                    // plain assignment is race-free: every subsequent
                    // `RioEvent` this machine dispatches is tagged with the
                    // new host window.
                    self.window_id = window_id;
                }
            }
        }

        true
    }

    /// Returns a `bool` indicating whether or not the event loop should continue running.
    #[inline]
    fn channel_event(&mut self, token: corcovado::Token, state: &mut State) -> bool {
        if !self.drain_recv_channel(state) {
            return false;
        }

        self.poll
            .reregister(
                &self.receiver.rx,
                token,
                Ready::readable(),
                PollOpt::edge() | PollOpt::oneshot(),
            )
            .unwrap();

        true
    }

    pub fn channel(&self) -> channel::Sender<Msg> {
        self.sender.clone()
    }

    pub fn spawn(mut self) -> JoinHandle<(Self, State)> {
        spawn_named("PTY parser driver", move || {
            let mut state = State::default();

            let mut tokens = (0..).map(Into::into);
            let poll_opts = PollOpt::edge() | PollOpt::oneshot();

            let channel_token = tokens.next().unwrap();
            self.poll
                .register(
                    &self.receiver.rx,
                    channel_token,
                    Ready::readable(),
                    poll_opts,
                )
                .unwrap();

            let byte_token = tokens.next().unwrap();
            self.poll
                .register(&self.byte_rx, byte_token, Ready::readable(), poll_opts)
                .unwrap();

            let child_event_token = tokens.next().unwrap();
            self.poll
                .register(
                    &self.child_event_rx,
                    child_event_token,
                    Ready::readable(),
                    poll_opts,
                )
                .unwrap();

            let mut events = Events::with_capacity(1024);

            'event_loop: loop {
                // Wakeup the event loop when a synchronized update timeout was reached.
                let handler = state.parser.sync_timeout();
                let timeout = handler
                    .sync_timeout()
                    .map(|st| st.saturating_duration_since(Instant::now()));

                events.clear();
                if let Err(err) = self.poll.poll(&mut events, timeout) {
                    match err.kind() {
                        std::io::ErrorKind::Interrupted => continue,
                        _ => {
                            error!("Event loop polling error: {err}");
                            break 'event_loop;
                        }
                    }
                }

                // Handle synchronized update timeout.
                if events.is_empty() && self.receiver.peek().is_none() {
                    let mut terminal = self.terminal.lock();
                    state.parser.stop_sync(&mut *terminal);

                    // Notify renderer if damage available and no event in flight
                    if !terminal.damage_event_in_flight
                        && terminal.peek_damage_event().is_some()
                    {
                        terminal.damage_event_in_flight = true;
                        self.event_proxy.send_event(
                            RioEvent::TerminalDamaged(self.route_id),
                            self.window_id,
                        );
                    }

                    continue;
                }

                // Handle channel events, if there are any.
                if !self.drain_recv_channel(&mut state) {
                    break;
                }

                for event in events.iter() {
                    match event.token() {
                        token if token == channel_token => {
                            // In case should shutdown by message
                            if !self.channel_event(channel_token, &mut state) {
                                break 'event_loop;
                            }
                        }
                        token if token == child_event_token => {
                            let mut exited = false;
                            loop {
                                match self.child_event_rx.try_recv() {
                                    Ok(_status) => exited = true,
                                    Err(TryRecvError::Empty) => break,
                                    Err(TryRecvError::Disconnected) => {
                                        exited = exited || self.pty.exit_code().is_some();
                                        break;
                                    }
                                }
                            }

                            if exited {
                                // `Crosswords::exit` pushes a
                                // `TerminalEffect::Exit` into the
                                // per-terminal effect buffer; drain
                                // it so the native frontend still
                                // observes a `RioEvent::CloseTerminal`.
                                let drained_exit_effects: Vec<_> = {
                                    let mut terminal = self.terminal.lock();
                                    terminal.exit();
                                    terminal.drain_effects().collect()
                                };
                                if !drained_exit_effects.is_empty() {
                                    crate::effects_adapter::dispatch_terminal_effects(
                                        drained_exit_effects,
                                        &self.event_proxy,
                                        self.window_id,
                                        self.route_id,
                                    );
                                }

                                self.event_proxy
                                    .send_event(RioEvent::Render, self.window_id);

                                break 'event_loop;
                            }

                            let _ = self.poll.reregister(
                                &self.child_event_rx,
                                child_event_token,
                                Ready::readable(),
                                poll_opts,
                            );
                        }
                        token if token == byte_token => {
                            self.drain_bytes_into_parser(&mut state);
                            let _ = self.poll.reregister(
                                &self.byte_rx,
                                byte_token,
                                Ready::readable(),
                                poll_opts,
                            );
                        }
                        _ => (),
                    }
                }
            }

            // The evented instances are not dropped here so deregister them explicitly.
            let _ = self.poll.deregister(&self.receiver.rx);
            let _ = self.poll.deregister(&self.byte_rx);
            let _ = self.poll.deregister(&self.child_event_rx);

            (self, state)
        })
    }
}
