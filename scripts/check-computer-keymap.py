#!/usr/bin/env python3
"""Linux, no desktop connection: test generated maps with Enigo's EXACT private parser.

Locate the locked dependency via cargo metadata and compile its unmodified parser
in a disposable test crate. No vendored parser copy, no registry patches, no
mock parser. Also exercises libxkbcommon's compositor-style reserialization.
Run: python3 scripts/check-computer-keymap.py
Browser launcher uses --features browser-live for the separately opt-in module.
That mode excludes the parser copy only: serde_json's scalar PartialEq impls
make upstream Enigo's `metadata.len() != size.into()` ambiguous when compiled
in this same crate. The normal invocation keeps every historical parser test.
"""
import json
import pathlib
import subprocess
import sys
import tempfile

root = pathlib.Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--locked", "--offline", "--format-version", "1",
    "--filter-platform", "x86_64-unknown-linux-gnu",
], cwd=root))
enigo = next(p for p in metadata["packages"] if p["name"] == "enigo")
parser = pathlib.Path(enigo["manifest_path"]).parent / "src/linux/keymap2/mod.rs"
source = root / "neoism-agent/crates/neoism-agent-server/src/computer_use/linux_text.rs"
shortcuts = source.with_name("shortcuts.rs")
main_source = (source.parent.parent / "computer_use.rs").read_text()
key_parsers = main_source[main_source.index("fn modifier_key("):main_source.index("fn coordinates(")]
# Do not carry coordinates' cfg attributes onto the next generated module.
key_parsers = key_parsers[:key_parsers.rfind("}") + 1]
with tempfile.TemporaryDirectory(prefix="neoism-keymap-parser-") as directory:
    directory = pathlib.Path(directory)
    (directory / "src").mkdir()
    (directory / "Cargo.toml").write_text(f'''[package]
name = "neoism-keymap-parser-regression"
version = "0.0.0"
edition = "2024"
[dependencies]
anyhow = "1"
enigo = {{ version = "={enigo['version']}", default-features = false, features = ["wayland"] }}
nom = "8"
log = "0.4"
libc = "0.2"
serde_json = {{ version = "1", optional = true }}
xkbcommon = "0.9"
xkeysym = "0.2"
tempfile = "3"
rand = "0.8"
wayland-client = "0.31"
wayland-protocols = {{ version = "0.32", features = ["client", "unstable"] }}
wayland-protocols-wlr = {{ version = "0.3", features = ["client"] }}
wayland-protocols-misc = {{ version = "0.3", features = ["client"] }}
[features]
browser-live = ["dep:serde_json"]
''')
    (directory / "src/lib.rs").write_text(f'''#![allow(dead_code)]
pub use enigo::{{InputError,InputResult,Key}};
use anyhow::bail;
{key_parsers}
#[cfg(not(feature="browser-live"))]
#[path={json.dumps(str(parser))}] mod actual_enigo;
#[path={json.dumps(str(source))}] mod linux_text;
#[path={json.dumps(str(shortcuts))}] mod shortcuts;
#[derive(Clone, Debug, PartialEq)]
struct Display {{ id:String, x:i32, y:i32, width:u32, height:u32 }}
#[cfg(feature="browser-live")]
#[path={json.dumps(str(source.with_name('linux_pointer.rs')))}] mod linux_pointer;
#[cfg(feature="browser-live")]
#[path={json.dumps(str(source.with_name('linux_clipboard.rs')))}] mod linux_clipboard;
#[cfg(all(test, feature="browser-live"))] #[path={json.dumps(str(source.with_name('linux_browser_live_tests.rs')))}] mod linux_browser_live_tests;
#[cfg(all(test, not(feature="browser-live")))] mod regression {{
    use super::*;
    use xkbcommon::xkb;
    fn serialize(source:String)->String {{
        xkb::Keymap::new_from_string(&xkb::Context::new(xkb::CONTEXT_NO_FLAGS),source,xkb::KEYMAP_FORMAT_TEXT_V1,xkb::KEYMAP_COMPILE_NO_FLAGS).unwrap().get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
    }}
    fn load_fd(source:&str)->Result<actual_enigo::Keymap2,()> {{
        use std::io::Seek;
        let mut file=linux_text::keymap_file(source).unwrap();
        file.seek(std::io::SeekFrom::End(0)).unwrap();
        actual_enigo::Keymap2::new_from_fd(xkb::Context::new(xkb::CONTEXT_NO_FLAGS),xkb::KEYMAP_FORMAT_TEXT_V1,file.into(),(source.len()+1) as u32)
    }}
    #[test] fn old_unnamed_map_reproduces_dependency_unwrap_panic() {{
        let (source,_)=linux_text::keymap("https://www.google.com/search?q=neoism+browser");
        let old=serialize(source.replace("\\\"neoism_text\\\"", ""));
        assert!(std::panic::catch_unwind(||load_fd(&old).map(|_|())).is_err());
    }}
    #[test] fn url_enter_control_l_text_enter_and_cleanup_through_exact_fd_parser() {{
        linux_text::test_support::sequence(|source| {{
            assert!(load_fd(source).is_ok(),"exact Enigo FD loader rejected transaction map");
            assert!(load_fd(&serialize(source.to_owned())).is_ok(),"exact FD loader rejected compositor reserialization");
            Ok(())
        }});
    }}
    #[test] fn generated_and_compositor_serialized_maps_pass_exact_dependency_parser() {{
        let full=(0..512).map(|i|char::from_u32(0x400+i).unwrap()).collect::<String>();
        for text in ["https://www.google.com/search?q=neoism+browser", "Google: AAaa!!?? λλλ😀😀\\t\\n", "é中ß@#$%^&*()", "z", full.as_str()] {{
            let (generated,_)=linux_text::compiled_keymap(text).unwrap();
            for map in [generated.clone(),serialize(generated)] {{
                assert!(load_fd(&map).is_ok(),"actual Enigo FD loader rejected our map");
                let parsed=actual_enigo::ParsedKeymap::try_from(map.as_str()).expect("exact Enigo parser rejected our map");
                let roundtrip=parsed.to_string();
                actual_enigo::ParsedKeymap::try_from(roundtrip.as_str()).unwrap();
                serialize(roundtrip);
            }}
        }}
    }}
}}
''')
    subprocess.run(["cargo", "test", "--offline", "--manifest-path", str(directory / "Cargo.toml"), *sys.argv[1:]], cwd=root, check=True,
                   env=__import__("os").environ | {"CARGO_TARGET_DIR": str(root / "target/computer-keymap-regression")})
