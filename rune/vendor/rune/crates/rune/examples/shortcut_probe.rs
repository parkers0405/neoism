//! Native acceptance probe. Starts windowless; stdin EOF shuts down cleanly.
//! --probe-conflict expects another copy to own the accelerator.
//! --script scripts/global-shortcut.json --tree /path/to/tree.json tests the static handler.
use rune::{App, AppSpec, ShortcutError, Tx, text};
use std::io::Write;

const ACCELERATOR: &str = "cmd+ctrl+alt+shift+k";
struct State { count: u32, thread: std::thread::ThreadId }
fn fired(state: &mut State, _: &mut Tx) {
    assert_eq!(state.thread, std::thread::current().id(), "handler must run on the state/app thread");
    state.count += 1;
    println!("FIRED {}", state.count);
    std::io::stdout().flush().unwrap();
}
fn main() {
    let app = App::new(AppSpec::new("Shortcut probe", (100.0, 100.0),
        || State { count: 0, thread: std::thread::current().id() },
        |state, _| text(format!("count={}", state.count))))
        .global_shortcut(ACCELERATOR, fired)
        .windowless().accessory();
    let handle = app.handle();
    let check = handle.clone();
    let conflict = std::env::args().any(|s| s == "--probe-conflict");
    handle.update(move |_, tx| {
        let errors = check.take_shortcut_errors();
        if conflict {
            assert_eq!(errors, vec![ShortcutError::Conflict]);
            println!("CONFLICT");
            tx.quit();
        } else {
            assert!(errors.is_empty(), "{errors:?}");
            // Exercise runtime registration and unregistration on the app thread too.
            let id = check.register_shortcut("ctrl+alt+shift+f11".parse().unwrap(), |_, _| {}).unwrap();
            check.unregister_shortcut(id).unwrap();
            println!("READY");
        }
        std::io::stdout().flush().unwrap();
    });
    if !std::env::args().any(|a| a == "--tree") {
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            handle.quit();
        });
    }
    app.run();
}
