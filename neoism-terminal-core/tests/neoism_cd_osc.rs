use neoism_terminal_core::handler::Processor;
use neoism_terminal_core::{Crosswords, TerminalEffect, TerminalId};

fn effects(bytes: &[u8]) -> Vec<TerminalEffect> {
    let mut terminal = Crosswords::new(
        (8, 40),
        neoism_terminal_core::ansi::CursorShape::Block,
        TerminalId::new(1),
        100,
    );
    Processor::<neoism_terminal_core::handler::StdSyncHandler>::new()
        .advance(&mut terminal, bytes);
    terminal.drain_effects().collect()
}

#[test]
fn neoism_cd_osc_decodes_absolute_path() {
    let got = effects(b"\x1b]777;neoism;cd;L3RtcC9hIGI=\x07");
    assert!(got.iter().any(|effect| matches!(
        effect,
        TerminalEffect::ChangeTerminalDirectory { path } if path == std::path::Path::new("/tmp/a b")
    )));
}

#[test]
fn neoism_cd_osc_rejects_malformed_or_control_paths() {
    for bytes in [
        b"\x1b]777;neoism;cd;%%%\x07".as_slice(),
        b"\x1b]777;neoism;cd;L3RtcC9hCmI=\x07".as_slice(),
        b"\x1b]777;neoism;cd\x07".as_slice(),
    ] {
        assert!(!effects(bytes).iter().any(|effect| matches!(
            effect,
            TerminalEffect::ChangeTerminalDirectory { .. }
        )));
    }
}
