//! Owned Linux keyboard transactions. Text extends the observed full layout;
//! shortcuts resolve against the very same map published by their injector.
use anyhow::{Context, ensure};
use std::{io::Write, os::fd::{AsFd,AsRawFd}, sync::{Arc,atomic::{AtomicBool,Ordering}}, time::{Duration,Instant}};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, protocol::{wl_callback,wl_keyboard,wl_registry, wl_seat}};
use xkbcommon::xkb;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{zwp_virtual_keyboard_manager_v1 as manager, zwp_virtual_keyboard_v1 as keyboard};

#[derive(Default)]
struct State { seats: Vec<wl_seat::WlSeat>, manager: Option<manager::ZwpVirtualKeyboardManagerV1>, map:Option<xkb::Keymap>, group:Option<u32>, error:Option<String>, keyboard_bound:bool, map_revision:u64 }
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(s:&mut Self,r:&wl_registry::WlRegistry,e:wl_registry::Event,_:&(),_:&Connection,q:&QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = e {
            match interface.as_str() {
                "wl_seat" => s.seats.push(r.bind(name,version.min(7),q,())),
                "zwp_virtual_keyboard_manager_v1" => s.manager = Some(r.bind(name,1,q,())),
                _ => {}
            }
        }
    }
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(s:&mut Self,seat:&wl_seat::WlSeat,event:wl_seat::Event,_:&(),_:&Connection,q:&QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities {capabilities:WEnum::Value(c)}=event {
            if c.contains(wl_seat::Capability::Keyboard) && !s.keyboard_bound {
                seat.get_keyboard(q,()); s.keyboard_bound=true;
            }
        }
    }
}
impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(s:&mut Self,_:&wl_keyboard::WlKeyboard,event:wl_keyboard::Event,_:&(),_:&Connection,_:&QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Keymap {format,fd,size}=>{
                s.map_revision=s.map_revision.wrapping_add(1);
                let result=if format==WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) { super::shortcuts::read_map(&std::fs::File::from(fd),size) } else { Err(anyhow::anyhow!("Unsupported keymap format")) };
                match result { Ok(map)=>{s.map=Some(map);s.error=None;},Err(error)=>{s.map=None;s.error=Some(error.to_string());} }
            }
            wl_keyboard::Event::Modifiers {group,..}=>s.group=Some(group),
            _=>{}
        }
    }
}
impl Dispatch<wl_callback::WlCallback,Arc<AtomicBool>> for State {
    fn event(_:&mut Self,_:&wl_callback::WlCallback,_:wl_callback::Event,done:&Arc<AtomicBool>,_:&Connection,_:&QueueHandle<Self>) {
        done.store(true,Ordering::Relaxed);
    }
}
wayland_client::delegate_noop!(State: ignore manager::ZwpVirtualKeyboardManagerV1);
wayland_client::delegate_noop!(State: ignore keyboard::ZwpVirtualKeyboardV1);

#[cfg(test)]
pub(super) fn keymap(text:&str) -> (String, Vec<u32>) {
    let mut chars = Vec::new();
    let codes = text.chars().map(|ch| {
        let index = chars.iter().position(|c| *c == ch).unwrap_or_else(|| { chars.push(ch); chars.len()-1 });
        index as u32 + 9
    }).collect();
    let mut codes_source = String::new();
    let mut symbols = String::new();
    for (i,ch) in chars.iter().enumerate() {
        codes_source.push_str(&format!("<K{i:03}> = {};",i+9));
        let sym = match ch { '\n' | '\r' => "Return".into(), '\t' => "Tab".into(), c => format!("U{:04X}",*c as u32) };
        symbols.push_str(&format!("key <K{i:03}> {{ type=\"ONE_LEVEL\", [ {sym} ] }};"));
    }
    (format!("xkb_keymap {{ xkb_keycodes \"neoism_text\" {{ minimum=9; maximum={}; {codes_source} }}; xkb_types \"neoism_text\" {{ type \"ONE_LEVEL\" {{ modifiers=None; map[None]=Level1; }}; }}; xkb_compatibility \"neoism_text\" {{}}; xkb_symbols \"neoism_text\" {{ {symbols} }}; }};",(chars.len()+8).max(9)),codes)
}
// Compile before publishing and send the canonical form that other clients
// (notably Enigo's strict parser) will see after compositor serialization.
#[cfg(test)]
pub(super) fn compiled_keymap(text:&str)->anyhow::Result<(String,Vec<u32>)> {
    use xkbcommon::xkb;
    let (source,codes)=keymap(text);
    let map=xkb::Keymap::new_from_string(&xkb::Context::new(xkb::CONTEXT_NO_FLAGS),source,xkb::KEYMAP_FORMAT_TEXT_V1,xkb::KEYMAP_COMPILE_NO_FLAGS).context("Generated text keymap failed native compilation")?;
    Ok((map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1),codes))
}
pub(super) fn keymap_file(source:&str)->anyhow::Result<std::fs::File> {
    use std::{os::fd::FromRawFd,io::Seek};
    let fd=unsafe { libc::memfd_create(c"neoism-text".as_ptr(),libc::MFD_CLOEXEC) };
    ensure!(fd>=0,"Cannot allocate text keymap FD");
    let mut file=unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(source.as_bytes())?;
    file.write_all(&[0])?;
    file.rewind()?;
    Ok(file)
}
// Ordinary printable evdev positions (+8 for XKB). Chromium converts these
// through its fixed DomCode table BEFORE consulting the uploaded XKB map.
// Appending above max_keycode produces valid XKB that Chromium silently drops.
const LITERAL_KEYCODES: &[u32] = &[
    10,11,12,13,14,15,16,17,18,19,20,21,
    24,25,26,27,28,29,30,31,32,33,34,35,
    38,39,40,41,42,43,44,45,46,47,48,49,
    51,52,53,54,55,56,57,58,59,60,61,65,
];
fn literal_keycodes(original:&xkb::Keymap)->Vec<u32> {
    LITERAL_KEYCODES.iter().copied().filter(|code|original.key_get_name(xkb::Keycode::new(*code)).is_some()).collect()
}
fn text_chunks(text:&str,capacity:usize)->anyhow::Result<Vec<&str>> {
    ensure!(capacity>0,"Desktop layout has no supported literal-text key positions");
    ensure!(text.chars().count()<=512,"Text exceeds 512 Unicode characters");
    ensure!(!text.chars().any(char::is_control),"Text cannot contain control characters; use explicit key actions for Return/Tab instead of implicitly submitting text");
    ensure!(text.chars().all(|ch|u32::from(ch)<=0xffff),"Native Wayland keyboard text cannot safely deliver supplementary Unicode to Chromium; use explicit clipboard paste instead");
    let mut chunks=Vec::new();
    let mut chars=Vec::new();
    let mut start=0;
    for (offset,ch) in text.char_indices() {
        if !chars.contains(&ch) {
            if chars.len()==capacity {
                chunks.push(&text[start..offset]);start=offset;chars.clear();
            }
            chars.push(ch);
        }
    }
    if start<text.len() {chunks.push(&text[start..]);}
    Ok(chunks)
}
/// Temporarily remap recognized printable positions, retaining the rest of the
/// desktop map. The exact baseline is restored before any subsequent action.
/// All edits operate on libxkbcommon's canonical serialization, not user grammar.
pub(super) fn text_overlay(original:&xkb::Keymap,text:&str)->anyhow::Result<(String,Vec<u32>)> {
    validate_baseline(original)?;
    let slots=literal_keycodes(original);
    ensure!(text_chunks(text,slots.len())?.len()<=1,"Text needs multiple bounded keymap chunks");
    let mut source=original.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let mut chars=Vec::new();
    let codes=text.chars().map(|ch| {
        let index=chars.iter().position(|c|*c==ch).unwrap_or_else(||{chars.push(ch);chars.len()-1});
        slots[index]
    }).collect::<Vec<_>>();
    let mut symbols=String::new();
    for (index,ch) in chars.iter().enumerate() {
        let name=original.key_get_name(xkb::Keycode::new(slots[index])).context("Missing literal key name")?;
        symbols.push_str(&format!("\n    replace key <{name}> {{"));
        for group in 1..=original.num_layouts() {
            symbols.push_str(&format!(" type[Group{group}]=\"NEOISM_LITERAL\", symbols[Group{group}]=[ U{:04X} ], actions[Group{group}]=[ NoAction() ],",*ch as u32));
        }
        symbols.pop();
        symbols.push_str(" };");
    }
    append_section(&mut source,"xkb_types","\n    type \"NEOISM_LITERAL\" { modifiers=None; map[None]=Level1; };")?;
    append_section(&mut source,"xkb_symbols",&symbols)?;
    let start=source.find("\nxkb_symbols ").context("Missing canonical symbols")?+1;
    let end=start+source[start..].find("{\n").context("Missing canonical symbols body")?;
    source.replace_range(start..end,"xkb_symbols \"neoism_overlay\" ");
    let map=compile(source)?;
    // No retained XKB modifier action or group-specific definition may alter
    // literal characters, even on a custom multi-layout desktop map. Application
    // key handlers remain outside this guarantee; delivery is unverified.
    for group in 0..original.num_layouts() {
        let mut state=xkb::State::new(&map);
        state.update_mask(0,0,0,0,0,group);
        for (ch,code) in chars.iter().zip(&slots) {
            let code=xkb::Keycode::new(*code);
            state.update_key(code,xkb::KeyDirection::Down);
            ensure!(state.key_get_utf8(code)==ch.to_string() && state.serialize_mods(xkb::STATE_MODS_EFFECTIVE)==0 && state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE)==group,"Literal keymap changes text or modifier/layout state");
            state.update_key(code,xkb::KeyDirection::Up);
        }
    }
    Ok((map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1),codes))
}
fn append_section(source:&mut String,section:&str,extra:&str)->anyhow::Result<()> {
    let start=source.find(&format!("\n{section} ")).context("Missing canonical XKB section")?;
    let end=start+source[start..].find("\n};").context("Missing canonical XKB section end")?;
    source.insert_str(end,extra); Ok(())
}
pub(super) fn compile(source:String)->anyhow::Result<xkb::Keymap> {
    xkb::Keymap::new_from_string(&xkb::Context::new(xkb::CONTEXT_NO_FLAGS),source,xkb::KEYMAP_FORMAT_TEXT_V1,xkb::KEYMAP_COMPILE_NO_FLAGS).context("Cannot compile keyboard transaction map")
}
pub(super) fn validate_baseline(map:&xkb::Keymap)->anyhow::Result<()> {
    let source=map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    ensure!(map.max_keycode().raw()<=u16::MAX as u32,"Desktop keymap exceeds supported keycode range");
    let legacy_text_only=source.contains("<K000>") && (map.min_keycode().raw()..=map.max_keycode().raw())
        .filter_map(|code|map.key_get_name(xkb::Keycode::new(code)))
        .all(|name|name.len()==4 && name.starts_with('K') && name.as_bytes()[1..].iter().all(u8::is_ascii_digit));
    ensure!(!source.contains("\"neoism_text\"") && !source.contains("\"neoism_overlay\"") && !legacy_text_only,
        "Seat advertises a temporary Neoism keymap, not a restorable desktop layout");
    Ok(())
}

/// A small transport seam tests real transaction ordering with native XKB maps;
/// only the Wayland wire/ACKs are substituted, never keymap parsing/resolution.
pub(super) trait Wire {
    fn map(&mut self,source:&str)->anyhow::Result<()>;
    fn restore(&mut self,source:&str)->anyhow::Result<()>;
    fn key(&mut self,code:u32,down:bool)->anyhow::Result<()>;
    fn mods(&mut self,depressed:u32,latched:u32,locked:u32,group:u32)->anyhow::Result<()>;
    fn sync(&mut self)->anyhow::Result<()>;
    fn destroy(&mut self)->anyhow::Result<()>;
}
struct NativeWire<'a> {
    keyboard:keyboard::ZwpVirtualKeyboardV1,queue:&'a mut wayland_client::EventQueue<State>,state:&'a mut State, connection:Connection, start:std::time::Instant, destroyed:bool,
    // Keep all published FDs alive until cleanup ACK/disconnect.
    files:Vec<std::fs::File>,
    restoring:Option<String>,
    retiring:Option<keyboard::ZwpVirtualKeyboardV1>,
}
impl Wire for NativeWire<'_> {
    fn map(&mut self,source:&str)->anyhow::Result<()> {
        self.files.push(keymap_file(source)?);
        self.keyboard.keymap(1,self.files.last().unwrap().as_fd(),(source.len()+1) as u32);
        Ok(())
    }
    fn restore(&mut self,source:&str)->anyhow::Result<()> {
        self.files.push(keymap_file(source)?);
        // Hyprland's IME grab caches keymaps by device identity. Updating
        // the text device in place leaves Fcitx forwarding its old map.
        // A fresh baseline-only device refreshes that cache without a key.
        let replacement=self.state.manager.as_ref().context("Virtual keyboard manager disappeared")?
            .create_virtual_keyboard(&self.state.seats[0],&self.queue.handle(),());
        self.retiring=Some(std::mem::replace(&mut self.keyboard,replacement));
        self.keyboard.keymap(1,self.files.last().unwrap().as_fd(),(source.len()+1) as u32);
        self.retiring.as_ref().unwrap().destroy();
        self.retiring=None;
        self.restoring=Some(source.to_owned());
        Ok(())
    }
    fn key(&mut self,code:u32,down:bool)->anyhow::Result<()> {self.keyboard.key(self.start.elapsed().as_millis() as u32,code-8,u32::from(down));Ok(())}
    fn mods(&mut self,d:u32,l:u32,k:u32,g:u32)->anyhow::Result<()> {self.keyboard.modifiers(d,l,k,g);Ok(())}
    fn sync(&mut self)->anyhow::Result<()> {
        sync(&self.connection,self.queue,self.state,||Ok(()))?;
        if let Some(expected)=self.restoring.as_deref().filter(|_|self.destroyed) {
            // Our roundtrip cannot acknowledge Fcitx's separate connection.
            // Re-snapshot the seat after removal and allow its forwarded map
            // to settle, without retrying any input or changing other devices.
            let observer=self.state.seats[0].get_keyboard(&self.queue.handle(),());
            let result=wait_for_baseline(&self.connection,self.queue,self.state,Some(expected),Duration::from_millis(50),||Ok(()));
            observer.release();
            self.connection.flush()?;
            result.context("Input may have been delivered; restored desktop keymap was not observed")?;
        }
        Ok(())
    }
    fn destroy(&mut self)->anyhow::Result<()> {
        let retired=catch_native(||{
            if let Some(keyboard)=&self.retiring {keyboard.destroy();self.retiring=None;}
            Ok(())
        });
        let current=catch_native(||{
            if !self.destroyed {self.keyboard.destroy();self.destroyed=true;}
            Ok(())
        });
        retired.and(current)
    }
}
impl Drop for NativeWire<'_> {
    fn drop(&mut self) {
        let _=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{self.destroy()?;self.connection.flush()?;Ok::<_,anyhow::Error>(())}));
    }
}
struct Transaction<'a,W:Wire> {wire:&'a mut W,original:&'a str,group:u32,held:Vec<u32>,finished:bool,map_changed:bool}
impl<W:Wire> Transaction<'_,W> {
    fn cleanup(&mut self,ack:bool)->anyhow::Result<()> {
        let mut error=None;
        let mut attempt=|result:anyhow::Result<()>|if let Err(e)=result {error=Some(e);};
        for code in self.held.drain(..).rev() {attempt(catch_native(||self.wire.key(code,false)));}
        // Map FIRST, then its group/masks. The old code sent masks for the text
        // map before changing maps. ACK both restoration and removal; flush is
        // not an ordering barrier against a subsequent client connection.
        attempt(catch_native(||if self.map_changed {self.wire.restore(self.original)} else {self.wire.map(self.original)}));
        attempt(catch_native(||self.wire.mods(0,0,0,self.group)));
        if ack {attempt(catch_native(||self.wire.sync()));}
        attempt(catch_native(||self.wire.destroy()));
        if ack {attempt(catch_native(||self.wire.sync()));}
        self.finished=true;
        match error {Some(e)=>Err(e),None=>Ok(())}
    }
}
impl<W:Wire> Drop for Transaction<'_,W> {fn drop(&mut self) {if !self.finished {let _=self.cleanup(false);}}}
fn catch_native<T>(f:impl FnOnce()->anyhow::Result<T>)->anyhow::Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_|Err(anyhow::anyhow!("Keyboard transaction panicked; cleanup attempted")))
}
pub(super) fn transaction<W:Wire>(wire:&mut W,original:&str,source:&str,group:u32,codes:&[u32],chord:bool,mut check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    let map=compile(source.to_owned())?;
    let mut xstate=xkb::State::new(&map); xstate.update_mask(0,0,0,0,0,group);
    let mut tx=Transaction {wire,original,group,held:Vec::new(),finished:false,map_changed:source!=original};
    let result=catch_native(||{
        check()?;
        tx.wire.map(source)?; tx.wire.mods(0,0,0,group)?; tx.wire.sync()?;
        for code in codes {
            check()?;
            tx.held.push(*code);
            xstate.update_key(xkb::Keycode::new(*code),xkb::KeyDirection::Down);
            tx.wire.mods(xstate.serialize_mods(xkb::STATE_MODS_DEPRESSED),xstate.serialize_mods(xkb::STATE_MODS_LATCHED),xstate.serialize_mods(xkb::STATE_MODS_LOCKED),xstate.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE))?;
            tx.wire.key(*code,true)?;
            if !chord {tx.wire.key(*code,false)?;tx.held.pop();xstate.update_key(xkb::Keycode::new(*code),xkb::KeyDirection::Up);}
            tx.wire.sync()?;
        }
        // Paired releases ignore cancellation. No symbol lookup after modifiers.
        while let Some(code)=tx.held.last().copied() {
            tx.wire.key(code,false)?;tx.held.pop();
            xstate.update_key(xkb::Keycode::new(code),xkb::KeyDirection::Up);
            tx.wire.mods(xstate.serialize_mods(xkb::STATE_MODS_DEPRESSED),xstate.serialize_mods(xkb::STATE_MODS_LATCHED),xstate.serialize_mods(xkb::STATE_MODS_LOCKED),xstate.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE))?;
        }
        check()?; Ok(())
    });
    let cleanup=tx.cleanup(true); // Also on error/cancel/panic, not just success.
    match (result,cleanup) {
        (Ok(()),Ok(()))=>Ok(()),
        (Err(e),Ok(()))=>Err(e),
        (Ok(()),Err(e))=>Err(e.context("Keyboard restoration/removal not acknowledged")),
        (Err(e),Err(cleanup))=>Err(anyhow::anyhow!("{e:#}; cleanup failed: {cleanup:#}")),
    }
}
fn pump(conn:&Connection,queue:&mut wayland_client::EventQueue<State>,state:&mut State,deadline:Instant)->anyhow::Result<()> {
    queue.dispatch_pending(state)?;
    let blocked=match conn.flush() {
        Ok(())=>false,
        Err(wayland_client::backend::WaylandError::Io(e)) if e.kind()==std::io::ErrorKind::WouldBlock=>true,
        Err(e)=>return Err(e.into()),
    };
    ensure!(Instant::now()<deadline,"Wayland keyboard observation timed out");
    if let Some(read)=queue.prepare_read() {
        let mut fd=libc::pollfd {fd:read.connection_fd().as_raw_fd(),events:libc::POLLIN|if blocked {libc::POLLOUT} else {0},revents:0};
        let timeout=deadline.saturating_duration_since(Instant::now()).as_millis().min(10) as i32;
        // SAFETY: one initialized pollfd borrowing the live connection.
        let result=unsafe {libc::poll(&mut fd,1,timeout)};
        if result<0 {
            let error=std::io::Error::last_os_error();
            if error.kind()!=std::io::ErrorKind::Interrupted {return Err(error.into());}
        } else if result>0 {
            ensure!(fd.revents&(libc::POLLERR|libc::POLLHUP|libc::POLLNVAL)==0,"Wayland keyboard connection closed");
            if fd.revents&libc::POLLIN!=0 {read.read()?;}
        }
    }
    queue.dispatch_pending(state)?;
    Ok(())
}
fn sync(conn:&Connection,queue:&mut wayland_client::EventQueue<State>,state:&mut State,mut check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    let done=Arc::new(AtomicBool::new(false));
    conn.display().sync(&queue.handle(),done.clone());
    let deadline=Instant::now()+Duration::from_millis(500);
    while !done.load(Ordering::Relaxed) {check()?;pump(conn,queue,state,deadline)?;}
    Ok(())
}
fn wait_for_baseline(conn:&Connection,queue:&mut wayland_client::EventQueue<State>,state:&mut State,expected:Option<&str>,quiet:Duration,mut check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    let deadline=Instant::now()+Duration::from_millis(500);
    let initial_revision=state.map_revision;
    let mut revision=initial_revision;
    let mut clean_since=None;
    loop {
        check()?;
        pump(conn,queue,state,deadline)?;
        if revision!=state.map_revision {revision=state.map_revision;clean_since=None;}
        let observed=state.map.as_ref().context("No compositor keyboard map").and_then(|map| {
            validate_baseline(map)?;
            ensure!(expected.is_none_or(|source|map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)==source),"Observed keymap differs from the captured desktop baseline");
            Ok(())
        });
        let now=Instant::now();
        if observed.is_ok() && revision!=initial_revision {
            if now.duration_since(*clean_since.get_or_insert(now))>=quiet {return Ok(());}
        } else {clean_since=None;}
        if now>=deadline {return observed.and_then(|_|Err(anyhow::anyhow!("Desktop keymap did not settle")));}
    }
}

fn run_chunks(prepared:&[(String,Vec<u32>)],mut check:impl FnMut()->anyhow::Result<()>,mut run:impl FnMut(&str,&[u32],&mut dyn FnMut()->anyhow::Result<()>)->anyhow::Result<()>)->anyhow::Result<()> {
    for (index,(source,codes)) in prepared.iter().enumerate() {
        check().with_context(||if index==0 {"No keyboard events dispatched yet".into()} else {format!("Input may already have been delivered; stopped before chunk {}",index+1)})?;
        run(source,codes,&mut check).with_context(||format!("Keyboard transaction {} stopped; input may have been partially delivered",index+1))?;
        check().with_context(||format!("Input may already have been delivered; stopped after chunk {}",index+1))?;
    }
    Ok(())
}
fn perform(text:Option<&str>,keys:&[enigo::Key],mut check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    check()?;
    let conn=Connection::connect_to_env()?;
    let mut queue=conn.new_event_queue::<State>();
    conn.display().get_registry(&queue.handle(),());
    let mut state=State::default();
    // No input device exists yet. Observe the baseline on the SAME connection
    // that will own map publication, input, restoration and removal.
    for _ in 0..4 {sync(&conn,&mut queue,&mut state,&mut check)?;}
    ensure!(state.seats.len()==1,"Keyboard input requires one unambiguous Wayland seat");
    if let Some(error)=&state.error {anyhow::bail!("{error}");}
    if state.map.as_ref().is_some_and(|map|validate_baseline(map).is_err()) {
        wait_for_baseline(&conn,&mut queue,&mut state,None,Duration::ZERO,&mut check)
            .context("No input sent; desktop keymap preflight failed")?;
    }
    check()?;
    let original=state.map.as_ref().context("No compositor keyboard map")?;
    validate_baseline(original).context("No input sent; desktop keymap preflight failed")?;
    let group=match state.group {Some(g)=>g,None if original.num_layouts()==1=>0,None=>anyhow::bail!("Compositor did not provide active layout group")};
    ensure!(group<original.num_layouts(),"Invalid active layout group");
    // Compile every chunk before the first input event. Each chunk owns and
    // restores its device so the IME's identity cache cannot reuse an old map.
    let prepared=match text {
        Some(text)=>text_chunks(text,literal_keycodes(original).len())?.into_iter()
            .map(|chunk|{check()?;text_overlay(original,chunk)}).collect::<anyhow::Result<Vec<_>>>()?,
        None=>vec![(original.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1),super::shortcuts::resolve_in_map(original,group,keys)?.into_iter().map(u32::from).collect())],
    };
    let original=original.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    run_chunks(&prepared,&mut check,|source,codes,check| {
        let keyboard=state.manager.as_ref().context("Virtual keyboard protocol unavailable")?.create_virtual_keyboard(&state.seats[0],&queue.handle(),());
        let mut wire=NativeWire {keyboard,queue:&mut queue,state:&mut state,connection:conn.clone(),start:std::time::Instant::now(),destroyed:false,files:Vec::new(),restoring:None,retiring:None};
        let result=transaction(&mut wire,&original,source,group,codes,text.is_none(),check);
        let observed=wire.state.map.as_ref().context("No observed keymap after keyboard removal").and_then(validate_baseline)
            .context("Input may have been delivered; desktop keymap restoration could not be verified. Observe before retrying");
        result.and(observed)
    })
}
pub(super) fn send(text:&str,check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    if text.is_empty() {return Ok(());}
    text_chunks(text,LITERAL_KEYCODES.len())?;
    perform(Some(text),&[],check)
}
pub(super) fn send_keys(keys:&[enigo::Key],check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {perform(None,keys,check)}
#[cfg(test)] mod tests {
    use super::*;
    use xkbcommon::xkb;
    #[test] fn unresponsive_compositor_times_out_without_desktop_input() {
        let (client,_server)=std::os::unix::net::UnixStream::pair().unwrap();
        let conn=Connection::from_socket(client).unwrap();
        let mut queue=conn.new_event_queue::<State>();
        let start=Instant::now();
        let error=sync(&conn,&mut queue,&mut State::default(),||Ok(())).unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed()<Duration::from_secs(2));
    }
    #[test] fn observation_checks_cancellation_before_waiting() {
        let (client,_server)=std::os::unix::net::UnixStream::pair().unwrap();
        let conn=Connection::from_socket(client).unwrap();
        let mut queue=conn.new_event_queue::<State>();
        let error=wait_for_baseline(&conn,&mut queue,&mut State::default(),None,Duration::ZERO,||anyhow::bail!("cancelled")).unwrap_err();
        assert_eq!(error.to_string(),"cancelled");
    }
    #[test] fn maximum_code_is_valid_and_repeated_text_needs_no_map_updates() {
        let text=(0..512).map(|i|char::from_u32(0x400+i).unwrap()).collect::<String>();
        let (source,codes)=keymap(&text);
        let map=xkb::Keymap::new_from_string(&xkb::Context::new(xkb::CONTEXT_NO_FLAGS),source,xkb::KEYMAP_FORMAT_TEXT_V1,xkb::KEYMAP_COMPILE_NO_FLAGS).unwrap();
        let state=xkb::State::new(&map);
        let last=*codes.last().unwrap();
        assert_eq!(last,map.max_keycode().raw());
        assert_eq!(state.key_get_utf8(xkb::Keycode::new(last)),text.chars().last().unwrap().to_string());
        // Enigo 0.6.1 key_to_keycode uses min..max, excluding this real key.
        assert!(!(map.min_keycode().raw()..map.max_keycode().raw()).contains(&last));
        let (source,codes)=keymap("λλλλ");
        assert_eq!(codes,vec![9;4]);
        assert_eq!(source.matches("key <K000>").count(),1);
    }
    #[test] fn actual_native_map_preserves_text_and_has_no_modifier_state() {
        for text in ["Google: AAaa!!?? λλλ😀😀\t\n", "é中ß@#$%^&*()", "z", "abc\r\n"] {
            let (source,codes)=keymap(text);
            let map=xkb::Keymap::new_from_string(&xkb::Context::new(xkb::CONTEXT_NO_FLAGS),source,xkb::KEYMAP_FORMAT_TEXT_V1,xkb::KEYMAP_COMPILE_NO_FLAGS).unwrap();
            let mut state=xkb::State::new(&map);
            for (ch,code) in text.chars().zip(codes) {
                let code=xkb::Keycode::new(code);
                state.update_key(code,xkb::KeyDirection::Down);
                match ch {
                    '\n'|'\r'=>assert_eq!(state.key_get_one_sym(code).raw(),0xff0d),
                    '\t'=>assert_eq!(state.key_get_one_sym(code).raw(),0xff09),
                    c=>assert_eq!(state.key_get_utf8(code),c.to_string()),
                }
                state.update_key(code,xkb::KeyDirection::Up);
                assert_eq!(state.serialize_mods(xkb::STATE_MODS_EFFECTIVE),0);
            }
        }
    }
}

#[cfg(test)]
#[path="linux_keyboard_tests.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path="linux_keyboard_live_tests.rs"]
mod live_tests;
