//! `Tx::input_time`: each handler sees its own input's arrival time, and only input handlers
//! see one.
use std::time::Duration;

use super::*;
use crate::{Cx, El, Env, Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode, Runtime, Tx, el};

#[test]
fn an_event_timestamp_is_the_instant_that_long_before_the_uptime_read() {
    let now = Instant::now() + Duration::from_secs(10);
    assert_eq!(since_boot(now, 1000.25, 1000.0), now - Duration::from_millis(250));
    assert_eq!(since_boot(now, 5.5, 2.0), now - Duration::from_millis(3500));
    assert_eq!(since_boot(now, 1000.0, 1000.0), now, "stamped as the uptime was read");
    assert_eq!(since_boot(now, 1000.0, 1000.5), now, "a stamp after the read is now");
    assert_eq!(since_boot(now, 1000.0, f64::NAN), now);
}

#[derive(Default)]
struct Seen {
    keys: Vec<(Key, Option<Instant>)>,
    presses: Vec<Option<Instant>>,
    polls: Vec<Option<Instant>>,
}

fn view(_: &Seen, _: &mut Cx<Seen>) -> El {
    el().size(100.0, 100.0)
        .child(el().key("button").size(40.0, 40.0).on_press_tx(|s: &mut Seen, tx: &mut Tx| s.presses.push(tx.input_time())))
}

fn app() -> Runtime<Seen> {
    let mut rt = Runtime::new(Seen::default(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, e, tx| { s.keys.push((e.key.clone(), tx.input_time())); true });
    rt.frame(0.0);
    rt
}

fn physical(code: PhysicalKeyCode, key: Key) -> (PhysicalInput, Option<KeyEvent>) {
    (PhysicalInput::Key { key: code, mods: Mods::default(), pressed: true, repeat: false }, Some(KeyEvent { key, mods: Mods::default() }))
}

#[test]
fn input_that_piles_up_between_frames_keeps_each_event_s_time() {
    let mut rt = app();
    let t0 = Instant::now();
    let [t1, t2, t3] = [1, 2, 3].map(|ms| t0 + Duration::from_millis(ms));
    {
        let _input = scope(Some(t1));
        let (event, key) = physical(PhysicalKeyCode::ArrowDown, Key::Down);
        rt.physical_then(event, key, None, None);
    }
    {
        let _input = scope(Some(t2));
        let (event, key) = physical(PhysicalKeyCode::ArrowUp, Key::Up);
        rt.physical_then(event, key, None, None);
    }
    {
        let _input = scope(Some(t3));
        rt.pointer_move(20.0, 20.0);
        rt.pointer_down(20.0, 20.0);
        rt.pointer_up(20.0, 20.0);
    }
    rt.frame(0.0);
    assert_eq!(rt.state.keys, vec![(Key::Down, Some(t1)), (Key::Up, Some(t2))]);
    assert_eq!(rt.state.presses, vec![Some(t3)]);
}

#[test]
fn handlers_no_input_caused_see_no_input_time() {
    let mut rt = app();
    rt.poll = Some(|s, _, tx| { s.polls.push(tx.input_time()); false });
    {
        // A frame run while a host dispatches input still polls without its time.
        let _input = scope(Some(Instant::now()));
        rt.frame(0.0);
    }
    assert_eq!(rt.state.polls, vec![None]);
    rt.update(|s, tx| s.polls.push(tx.input_time()));
    assert_eq!(rt.state.polls, vec![None, None], "an update");
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.keys, vec![(Key::Down, None)], "input a program feeds the runtime itself");
    assert_eq!(current(), None, "every scope restored what it held");
}
