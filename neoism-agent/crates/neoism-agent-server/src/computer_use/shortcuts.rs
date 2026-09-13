//! Resolve complete chords before pressing anything. Text injection is separate.
use anyhow::ensure;
#[cfg(target_os = "linux")]
use anyhow::Context;
use enigo::{Direction, Key, Keyboard};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum NativeKey {
    Raw(u16),
    #[cfg(target_os = "windows")]
    Virtual(Key),
}
impl NativeKey {
    fn send(self, input: &mut impl Keyboard, direction: Direction) -> anyhow::Result<()> {
        match self {
            Self::Raw(code) => input.raw(code, direction)?,
            #[cfg(target_os = "windows")]
            Self::Virtual(key) => input.key(key, direction)?,
        }
        Ok(())
    }
}

/// One list containing modifiers first and the primary key last. Resolution
/// cannot inject events; all failures here precede any modifier presses.
pub(super) fn resolve(keys: &[Key]) -> anyhow::Result<Vec<NativeKey>> {
    #[cfg(target_os = "linux")]
    {
        let (map, group) = linux::current_keymap()?;
        keys.iter().map(|key| linux::resolve_key(&map, group, *key).map(NativeKey::Raw)).collect()
    }
    #[cfg(target_os = "macos")]
    { keys.iter().map(|key| macos::resolve_key(*key).map(NativeKey::Raw)).collect() }
    #[cfg(target_os = "windows")]
    {
        // named_key converts letters/digits to VK codes. Named modifier/control
        // keys are also fixed VKs, never Enigo's Unicode fallback.
        ensure!(keys.iter().all(|k| !matches!(k, Key::Unicode(_))), "Unicode is not a Windows shortcut key");
        Ok(keys.iter().copied().map(NativeKey::Virtual).collect())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    { anyhow::bail!("No native shortcut backend") }
}

#[cfg(target_os="linux")]
pub(super) fn read_map(file:&std::fs::File,size:u32)->anyhow::Result<xkbcommon::xkb::Keymap> { linux::read_keymap(file,size) }
#[cfg(target_os="linux")]
pub(super) fn resolve_in_map(map:&xkbcommon::xkb::Keymap,group:u32,keys:&[Key])->anyhow::Result<Vec<u16>> {
    keys.iter().map(|key|linux::resolve_key(map,group,*key)).collect()
}

struct Held<'a, K: Keyboard> { input: &'a mut K, keys: Vec<NativeKey> }
impl<K: Keyboard> Held<'_, K> {
    fn release(&mut self) -> anyhow::Result<()> {
        let mut error = None;
        for key in self.keys.drain(..).rev() {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(||key.send(self.input,Direction::Release))) {
                Ok(Ok(()))=>{},
                Ok(Err(e))=>error=Some(e),
                Err(_)=>error=Some(anyhow::anyhow!("Native key release panicked; remaining releases still attempted")),
            }
        }
        match error { Some(e) => Err(e), None => Ok(()) }
    }
}
impl<K: Keyboard> Drop for Held<'_, K> {
    fn drop(&mut self) { let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||self.release())); }
}
pub(super) fn send(input: &mut impl Keyboard, keys: &[NativeKey], mut check: impl FnMut() -> anyhow::Result<()>) -> anyhow::Result<()> {
    let mut held = Held { input, keys: Vec::new() };
    for &key in keys {
        check()?;
        held.keys.push(key); // Release even a partially successful press.
        key.send(held.input, Direction::Press)?;
    }
    held.release()
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::os::unix::fs::FileExt;
    use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, protocol::{wl_keyboard, wl_registry, wl_seat}};
    use xkbcommon::xkb;

    #[derive(Default)]
    struct Probe {
        seats: usize,
        keyboard_bound: bool,
        map: Option<xkb::Keymap>,
        group: Option<u32>,
        error: Option<String>,
    }
    impl Probe {
        fn record_keymap(&mut self, result: anyhow::Result<xkb::Keymap>) {
            match result {
                Ok(map) => { self.map = Some(map); self.error = None; }
                Err(error) => { self.map = None; self.error = Some(error.to_string()); }
            }
        }
    }
    impl Dispatch<wl_registry::WlRegistry, ()> for Probe {
        fn event(s: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, q: &QueueHandle<Self>) {
            if let wl_registry::Event::Global { name, interface, version } = event {
                if interface == "wl_seat" {
                    s.seats += 1;
                    if s.seats == 1 {
                        registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(7), q, ());
                    }
                }
            }
        }
    }
    impl Dispatch<wl_seat::WlSeat, ()> for Probe {
        fn event(s: &mut Self, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _: &Connection, q: &QueueHandle<Self>) {
            if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = event {
                if c.contains(wl_seat::Capability::Keyboard) && !s.keyboard_bound {
                    seat.get_keyboard(q, ());
                    s.keyboard_bound = true;
                }
            }
        }
    }
    impl Dispatch<wl_keyboard::WlKeyboard, ()> for Probe {
        fn event(s: &mut Self, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            match event {
                wl_keyboard::Event::Keymap { format, fd, size } => {
                    let result = (|| -> anyhow::Result<xkb::Keymap> {
                        ensure!(format == WEnum::Value(wl_keyboard::KeymapFormat::XkbV1), "Unsupported Wayland keymap format");
                        read_keymap(&std::fs::File::from(fd), size)
                    })();
                    s.record_keymap(result);
                }
                wl_keyboard::Event::Modifiers { group, .. } => s.group = Some(group),
                _ => {}
            }
        }
    }
    pub(super) fn read_keymap(file: &std::fs::File, size: u32) -> anyhow::Result<xkb::Keymap> {
        ensure!(size > 0 && size <= 4 * 1024 * 1024, "Invalid/oversized Wayland keymap");
        let mut bytes = vec![0; size as usize];
        // SCM_RIGHTS duplicates share the open-file offset. Compositors may
        // forward a virtual keyboard's FD after its writer left it at EOF.
        // pread reads offset zero without changing any other client's cursor.
        file.read_exact_at(&mut bytes, 0).context("Truncated/unreadable Wayland keymap FD")?;
        while bytes.last() == Some(&0) { bytes.pop(); }
        ensure!(!bytes.is_empty() && !bytes.contains(&0), "Empty or embedded-NUL Wayland keymap");
        let source = String::from_utf8(bytes).context("Wayland keymap is not UTF-8")?;
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        xkb::Keymap::new_from_string(&context, source, xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS)
            .context("Invalid XKB keymap in compositor FD (read completely from offset zero)")
    }
    pub(super) fn current_keymap() -> anyhow::Result<(xkb::Keymap, u32)> {
        let conn = Connection::connect_to_env()?;
        let mut queue = conn.new_event_queue::<Probe>();
        conn.display().get_registry(&queue.handle(), ());
        let mut probe = Probe::default();
        for _ in 0..4 { queue.roundtrip(&mut probe)?; }
        if let Some(error) = probe.error { anyhow::bail!(error); }
        ensure!(probe.seats == 1, "Cannot safely select a shortcut keyboard on a multi-seat desktop");
        let map = probe.map.context("No compositor keyboard keymap; cannot resolve shortcuts")?;
        // Wayland need not send modifiers/group to a client without keyboard
        // focus. A single-layout keymap is unambiguous; never guess otherwise.
        let group = match probe.group {
            Some(group) => group,
            None if map.num_layouts() == 1 => 0,
            None => anyhow::bail!("Compositor did not provide the active keyboard layout group"),
        };
        ensure!(group < map.num_layouts(), "Invalid keyboard layout group");
        Ok((map, group))
    }
    pub(super) fn resolve_key(map: &xkb::Keymap, group: u32, key: Key) -> anyhow::Result<u16> {
        let symbol: xkb::Keysym = key.into();
        // Level zero ignores Shift/Caps/Control, while retaining the layout group.
        // Enigo raw() accepts XKB codes and subtracts eight on the Wayland wire.
        for code in map.min_keycode().raw().max(8)..=map.max_keycode().raw().min(u16::MAX as u32) {
            if map.key_get_syms_by_level(xkb::Keycode::new(code), group, 0).contains(&symbol) {
                return Ok(code as u16);
            }
        }
        anyhow::bail!("Shortcut {key:?} has no unmodified key in the active layout; use text for literal characters")
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[derive(Default)]
        struct KeyboardLog { events: Vec<(u16, Direction)> }
        impl Keyboard for KeyboardLog {
            fn fast_text(&mut self, _: &str) -> enigo::InputResult<Option<()>> { panic!("shortcuts must not inject text") }
            fn key(&mut self, _: Key, _: Direction) -> enigo::InputResult<()> { panic!("shortcuts must not re-resolve symbols after modifiers") }
            fn raw(&mut self, code: u16, direction: Direction) -> enigo::InputResult<()> {
                self.events.push((code, direction));
                Ok(())
            }
        }
        #[test]
        fn chord_press_release_codes_are_identical_and_cleanup_survives_cancellation() {
            let map = keymap("us");
            for keys in [vec![Key::Shift, Key::Unicode('a')], vec![Key::Shift, Key::Unicode('1')], vec![Key::Control, Key::Shift, Key::Unicode('a')]] {
                let codes = keys.iter().map(|k| resolve_key(&map, 0, *k).unwrap()).collect::<Vec<_>>();
                let native = codes.iter().copied().map(NativeKey::Raw).collect::<Vec<_>>();
                let mut log = KeyboardLog::default();
                send(&mut log, &native, || Ok(())).unwrap();
                let expected = codes.iter().map(|c| (*c, Direction::Press)).chain(codes.iter().rev().map(|c| (*c, Direction::Release))).collect::<Vec<_>>();
                assert_eq!(log.events, expected);
                let mut log = KeyboardLog::default();
                let mut calls = 0;
                assert!(send(&mut log, &native, || {
                    calls += 1;
                    ensure!(calls < native.len(), "cancelled before primary key");
                    Ok(())
                }).is_err());
                assert_eq!(log.events.len(), (native.len() - 1) * 2);
                assert_eq!(log.events.last(), Some(&(codes[0], Direction::Release)));
            }
        }
        #[test] fn release_panic_does_not_skip_remaining_modifier_releases() {
            struct Panics(Vec<(u16,Direction)>);
            impl Keyboard for Panics {
                fn fast_text(&mut self,_:&str)->enigo::InputResult<Option<()>> {unreachable!()}
                fn key(&mut self,_:Key,_:Direction)->enigo::InputResult<()> {unreachable!()}
                fn raw(&mut self,code:u16,direction:Direction)->enigo::InputResult<()> {
                    self.0.push((code,direction));
                    if code==2 {panic!("partial native key event");}
                    Ok(())
                }
            }
            let mut input=Panics(Vec::new());
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(||send(&mut input,&[NativeKey::Raw(1),NativeKey::Raw(2)],||Ok(())))).is_err());
            assert_eq!(input.0,vec![(1,Direction::Press),(2,Direction::Press),(2,Direction::Release),(1,Direction::Release)]);
        }
        fn keymap_file(bytes: &[u8]) -> std::fs::File {
            use std::io::Write;
            let path = std::env::temp_dir().join(format!("neoism-keymap-{:032x}", rand::random::<u128>()));
            let mut file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();
            std::fs::remove_file(path).unwrap(); // Anonymous after open; no test litter.
            file.write_all(bytes).unwrap(); // Exactly like Enigo's writer: at EOF.
            file
        }
        #[test]
        fn keymap_fd_at_eof_parses_without_changing_shared_offset() {
            use std::io::{Read, Seek, SeekFrom};
            let source = keymap("us").get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
            for nul in [false, true] {
                let mut bytes = source.as_bytes().to_vec();
                if nul { bytes.push(0); }
                let mut file = keymap_file(&bytes);
                let mut old_read = String::new();
                file.read_to_string(&mut old_read).unwrap();
                assert!(old_read.is_empty(), "old offset-relative reader sees EOF and fails XKB parsing");
                for offset in [bytes.len() as u64, 17] {
                    file.seek(SeekFrom::Start(offset)).unwrap();
                    let map = read_keymap(&file.try_clone().unwrap(), bytes.len() as u32).unwrap();
                    assert_eq!(file.stream_position().unwrap(), offset, "pread must not alter the compositor's shared cursor");
                    assert_eq!(resolve_key(&map, 0, Key::Unicode('a')).unwrap(), 38);
                    let super_key = super::super::super::named_key("Super_L").unwrap();
                    assert_eq!(resolve_key(&map, 0, super_key).unwrap(), 133);
                }
            }
        }
        #[test]
        fn invalid_or_truncated_keymaps_are_errors_not_silent_fallbacks() {
            let file = keymap_file(b"not an xkb keymap");
            assert!(read_keymap(&file, 0).is_err());
            assert!(read_keymap(&file, 4 * 1024 * 1024 + 1).is_err());
            assert!(read_keymap(&file, 100).is_err());
            assert!(read_keymap(&file, 17).is_err());
            assert!(read_keymap(&keymap_file(b"\0\0"), 2).is_err());
            assert!(read_keymap(&keymap_file(b"a\0b"), 3).is_err());
        }
        #[test]
        fn replacement_keymap_clears_old_error_without_using_stale_maps() {
            let mut probe = Probe::default();
            probe.record_keymap(read_keymap(&keymap_file(b""), 1));
            assert!(probe.map.is_none() && probe.error.is_some());
            let source = keymap("us").get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
            probe.record_keymap(read_keymap(&keymap_file(source.as_bytes()), source.len() as u32));
            assert!(probe.map.is_some() && probe.error.is_none());
            probe.record_keymap(read_keymap(&keymap_file(b""), 1));
            assert!(probe.map.is_none() && probe.error.is_some());
        }
        #[test]
        #[ignore = "read-only live Wayland keymap probe; requires a desktop connection"]
        fn live_wayland_keymap_read_only() {
            let (map, group) = current_keymap().expect("read-only compositor keymap");
            println!("Parsed compositor keymap: {} layouts, group {group}", map.num_layouts());
            for key in [Key::Return, Key::Control, Key::Shift, Key::Meta, Key::Unicode('a'), Key::Unicode('l')] {
                println!("{key:?}: {}", resolve_key(&map, group, key).unwrap());
            }
        }
        fn keymap(layout: &str) -> xkb::Keymap {
            xkb::Keymap::new_from_names(&xkb::Context::new(xkb::CONTEXT_NO_FLAGS), "", "", layout, "", None, xkb::KEYMAP_COMPILE_NO_FLAGS).unwrap()
        }
        #[test]
        fn shifted_chords_use_original_keycodes_and_symbols() {
            let map = keymap("us");
            for (letter, control, expected) in [('a', false, 'A'), ('1', false, '!'), ('a', true, 'A')] {
                let key = resolve_key(&map, 0, Key::Unicode(letter)).unwrap();
                let shift = resolve_key(&map, 0, Key::Shift).unwrap();
                let mut state = xkb::State::new(&map);
                if control { state.update_key(xkb::Keycode::new(resolve_key(&map, 0, Key::Control).unwrap().into()), xkb::KeyDirection::Down); }
                state.update_key(xkb::Keycode::new(shift.into()), xkb::KeyDirection::Down);
                assert_eq!(state.key_get_one_sym(xkb::Keycode::new(key.into())), xkb::utf32_to_keysym(expected as u32));
                assert_eq!(resolve_key(&map, 0, Key::Unicode(letter)).unwrap(), key);
            }
        }
        #[test]
        fn non_us_layout_is_resolved_not_assumed() {
            let us = keymap("us");
            let de = keymap("de");
            assert_ne!(resolve_key(&us, 0, Key::Unicode('y')).unwrap(), resolve_key(&de, 0, Key::Unicode('y')).unwrap());
            assert!(resolve_key(&us, 0, Key::Unicode('λ')).is_err());
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::ffi::c_void;
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn TISCopyCurrentKeyboardLayoutInputSource() -> *const c_void;
        fn TISGetInputSourceProperty(source: *const c_void, key: *const c_void) -> *const c_void;
        static kTISPropertyUnicodeKeyLayoutData: *const c_void;
        fn LMGetKbdType() -> u8;
        fn UCKeyTranslate(layout: *const u8, code: u16, action: u16, modifiers: u32, keyboard: u32, options: u32, dead: *mut u32, max: usize, actual: *mut usize, text: *mut u16) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
        fn CFDataGetBytePtr(value: *const c_void) -> *const u8;
    }
    struct Source(*const c_void);
    impl Drop for Source { fn drop(&mut self) { if !self.0.is_null() { unsafe { CFRelease(self.0); } } } }
    pub(super) fn resolve_key(key: Key) -> anyhow::Result<u16> {
        let Key::Unicode(ch) = key else {
            return u16::try_from(key).map_err(|_| anyhow::anyhow!("Unsupported native macOS key"));
        };
        let source = Source(unsafe { TISCopyCurrentKeyboardLayoutInputSource() });
        ensure!(!source.0.is_null(), "No macOS keyboard layout");
        let data = unsafe { TISGetInputSourceProperty(source.0, kTISPropertyUnicodeKeyLayoutData) };
        ensure!(!data.is_null(), "No Unicode layout data; cannot resolve shortcut");
        let layout = unsafe { CFDataGetBytePtr(data) };
        ensure!(!layout.is_null(), "Empty macOS layout data");
        for code in 0..128 {
            let mut dead = 0;
            let mut actual = 0;
            let mut text = [0u16; 8];
            let status = unsafe { UCKeyTranslate(layout, code, 3, 0, LMGetKbdType().into(), 1, &mut dead, text.len(), &mut actual, text.as_mut_ptr()) };
            if status == 0 && actual == 1 && u32::from(text[0]) == ch as u32 { return Ok(code); }
        }
        anyhow::bail!("Shortcut {ch} has no unmodified key in the current macOS layout")
    }
}
