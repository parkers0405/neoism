//! Original-layout Linux keyboard sessions. Planning never creates a device.
#[path="layout_plan.rs"]
mod layout_plan;
pub(super) use layout_plan::KeyPlan;
use anyhow::{Context, ensure};
use std::{io::Write, os::fd::{AsFd,AsRawFd}, sync::{Arc,atomic::{AtomicBool,Ordering}}, time::{Duration,Instant}};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, protocol::{wl_callback,wl_keyboard,wl_registry, wl_seat}};
use xkbcommon::xkb;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{zwp_virtual_keyboard_manager_v1 as manager, zwp_virtual_keyboard_v1 as keyboard};

#[derive(Default)]
struct State { seats: Vec<wl_seat::WlSeat>, manager: Option<manager::ZwpVirtualKeyboardManagerV1>, map:Option<xkb::Keymap>, group:Option<u32>, error:Option<String>, keyboard_bound:bool, map_revision:u64, observed_mods:Option<u32>, group_revision:u64, raw_map:Option<String>, canonical_map:Option<String>,
    #[cfg(test)] compiled_maps:usize,
}
impl State {
    fn record_map_source(&mut self,result:anyhow::Result<String>) {
        // Freshness counts notifications, not compilations. Even a cache hit
        // comes from a newly read, bounded, UTF-8/NUL-validated compositor FD.
        self.map_revision=self.map_revision.wrapping_add(1);
        let prepared=result.and_then(|source| {
            if self.raw_map.as_deref()==Some(source.as_str()) && self.map.is_some() && self.canonical_map.is_some() && self.error.is_none() {return Ok(());}
            #[cfg(test)] {self.compiled_maps+=1;}
            let map=compile(source.clone())?;
            let canonical=map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
            validate_baseline_source(&map,&canonical)?;
            self.raw_map=Some(source);
            self.canonical_map=Some(canonical);
            self.map=Some(map);
            self.error=None;
            Ok(())
        });
        if let Err(error)=prepared {
            self.raw_map=None;self.canonical_map=None;self.map=None;
            self.error=Some(error.to_string());
        }
    }
}
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
                super::latency::count("keyboard_observers",1);
            }
        }
    }
}
impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(s:&mut Self,_:&wl_keyboard::WlKeyboard,event:wl_keyboard::Event,_:&(),_:&Connection,_:&QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Keymap {format,fd,size}=>{
                let result=if format==WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) { super::shortcuts::read_map_source(&std::fs::File::from(fd),size) } else { Err(anyhow::anyhow!("Unsupported keymap format")) };
                s.record_map_source(result);
            }
            wl_keyboard::Event::Modifiers {group,mods_depressed,mods_latched,mods_locked,..}=>{s.group_revision=s.group_revision.wrapping_add(1);s.group=Some(group);s.observed_mods=Some(mods_depressed|mods_latched|mods_locked);},
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
#[cfg(test)]
const LITERAL_KEYCODES: &[u32] = &[
    10,11,12,13,14,15,16,17,18,19,20,21,
    24,25,26,27,28,29,30,31,32,33,34,35,
    38,39,40,41,42,43,44,45,46,47,48,49,
    51,52,53,54,55,56,57,58,59,60,61,65,
];
#[cfg(test)]
fn literal_keycodes(original:&xkb::Keymap)->Vec<u32> {
    LITERAL_KEYCODES.iter().copied().filter(|code|original.key_get_name(xkb::Keycode::new(*code)).is_some()).collect()
}
#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
fn append_section(source:&mut String,section:&str,extra:&str)->anyhow::Result<()> {
    let start=source.find(&format!("\n{section} ")).context("Missing canonical XKB section")?;
    let end=start+source[start..].find("\n};").context("Missing canonical XKB section end")?;
    source.insert_str(end,extra); Ok(())
}
pub(super) fn compile(source:String)->anyhow::Result<xkb::Keymap> {
    xkb::Keymap::new_from_string(&xkb::Context::new(xkb::CONTEXT_NO_FLAGS),source,xkb::KEYMAP_FORMAT_TEXT_V1,xkb::KEYMAP_COMPILE_NO_FLAGS).context("Cannot compile keyboard transaction map")
}
pub(super) fn validate_baseline(map:&xkb::Keymap)->anyhow::Result<()> {
    validate_baseline_source(map,&map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1))
}
fn validate_baseline_source(map:&xkb::Keymap,source:&str)->anyhow::Result<()> {
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
#[cfg(test)]
pub(super) trait Wire {
    fn map(&mut self,source:&str)->anyhow::Result<()>;
    fn restore(&mut self,source:&str)->anyhow::Result<()>;
    fn key(&mut self,code:u32,down:bool)->anyhow::Result<()>;
    fn mods(&mut self,depressed:u32,latched:u32,locked:u32,group:u32)->anyhow::Result<()>;
    fn sync(&mut self)->anyhow::Result<()>;
    fn destroy(&mut self)->anyhow::Result<()>;
}
#[cfg(test)]
struct NativeWire<'a> {
    keyboard:keyboard::ZwpVirtualKeyboardV1,queue:&'a mut wayland_client::EventQueue<State>,state:&'a mut State, connection:Connection, start:std::time::Instant, destroyed:bool,
    // Keep all published FDs alive until cleanup ACK/disconnect.
    files:Vec<std::fs::File>,
    restoring:Option<String>,
    retiring:Option<keyboard::ZwpVirtualKeyboardV1>,
}
#[cfg(test)]
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
#[cfg(test)]
impl Drop for NativeWire<'_> {
    fn drop(&mut self) {
        let _=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{self.destroy()?;self.connection.flush()?;Ok::<_,anyhow::Error>(())}));
    }
}
#[cfg(test)]
struct Transaction<'a,W:Wire> {wire:&'a mut W,original:&'a str,group:u32,held:Vec<u32>,finished:bool,map_changed:bool}
#[cfg(test)]
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
#[cfg(test)]
impl<W:Wire> Drop for Transaction<'_,W> {fn drop(&mut self) {if !self.finished {let _=self.cleanup(false);}}}
fn catch_native<T>(f:impl FnOnce()->anyhow::Result<T>)->anyhow::Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_|Err(anyhow::anyhow!("Keyboard transaction panicked; cleanup attempted")))
}
#[cfg(test)]
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
    pump_until(conn,queue,state,deadline,||false)
}
fn pump_until(conn:&Connection,queue:&mut wayland_client::EventQueue<State>,state:&mut State,deadline:Instant,done:impl Fn()->bool)->anyhow::Result<()> {
    queue.dispatch_pending(state)?;
    let blocked=match conn.flush() {
        Ok(())=>false,
        Err(wayland_client::backend::WaylandError::Io(e)) if e.kind()==std::io::ErrorKind::WouldBlock=>true,
        Err(e)=>return Err(e.into()),
    };
    ensure!(Instant::now()<deadline,"Wayland keyboard observation timed out");
    if done() {return Ok(());}
    if let Some(read)=queue.prepare_read() {
        let mut fd=libc::pollfd {fd:read.connection_fd().as_raw_fd(),events:libc::POLLIN|if blocked {libc::POLLOUT} else {0},revents:0};
        let timeout=deadline.saturating_duration_since(Instant::now()).as_millis().min(10) as i32;
        let result=keyboard_poll(&mut fd,timeout);
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
#[cfg(test)]
thread_local! { static BLOCKING_POLLS:std::cell::Cell<usize>=const {std::cell::Cell::new(0)}; }
fn keyboard_poll(fd:&mut libc::pollfd,timeout:i32)->i32 {
    #[cfg(test)]
    if timeout>0 {BLOCKING_POLLS.with(|count|count.set(count.get()+1));}
    // SAFETY: one initialized pollfd borrowing the live connection.
    super::latency::measure("keyboard_poll",||unsafe {libc::poll(fd,1,timeout)})
}
fn sync(conn:&Connection,queue:&mut wayland_client::EventQueue<State>,state:&mut State,mut check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    super::latency::measure("keyboard_sync",|| {
        let done=Arc::new(AtomicBool::new(false));
        conn.display().sync(&queue.handle(),done.clone());
        let deadline=Instant::now()+Duration::from_millis(500);
        while !done.load(Ordering::Relaxed) {check()?;pump_until(conn,queue,state,deadline,||done.load(Ordering::Relaxed))?;}
        Ok(())
    })
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

#[cfg(test)]
fn run_chunks(prepared:&[(String,Vec<u32>)],mut check:impl FnMut()->anyhow::Result<()>,mut run:impl FnMut(&str,&[u32],&mut dyn FnMut()->anyhow::Result<()>)->anyhow::Result<()>)->anyhow::Result<()> {
    for (index,(source,codes)) in prepared.iter().enumerate() {
        check().with_context(||if index==0 {"No keyboard events dispatched yet".into()} else {format!("Input may already have been delivered; stopped before chunk {}",index+1)})?;
        run(source,codes,&mut check).with_context(||format!("Keyboard transaction {} stopped; input may have been partially delivered",index+1))?;
        check().with_context(||format!("Input may already have been delivered; stopped after chunk {}",index+1))?;
    }
    Ok(())
}
/// Facts concern native queue/ACK boundaries, not application consumption.
#[derive(Debug,Clone,Copy,Default,PartialEq,Eq)]
pub(super) struct KeyboardFailureFacts {
    pub(super) completed_units:usize,
    pub(super) current_unit_uncertain:bool,
    pub(super) cleanup_failed:bool,
}
#[derive(Debug)]
struct KeyboardError {facts:KeyboardFailureFacts,source:anyhow::Error}
impl std::fmt::Display for KeyboardError {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {write!(f,"{}",self.source)}
}
impl std::error::Error for KeyboardError {
    fn source(&self)->Option<&(dyn std::error::Error+'static)> {Some(self.source.as_ref())}
}
pub(super) fn failure_facts(error:&anyhow::Error)->Option<KeyboardFailureFacts> {
    error.downcast_ref::<KeyboardError>().map(|error|error.facts)
}
fn keyboard_error(source:anyhow::Error,facts:KeyboardFailureFacts)->anyhow::Error {KeyboardError {source,facts}.into()}
// Shared stroke engine: fake transports exercise the exact per-unit ordering.
trait StrokeWire {
    fn guard(&mut self,check:&mut dyn FnMut()->anyhow::Result<()>)->anyhow::Result<()>;
    fn stroke(&mut self,code:u32,down:bool,state:&xkb::State)->anyhow::Result<()>;
    fn ack(&mut self,check:&mut dyn FnMut()->anyhow::Result<()>)->anyhow::Result<()> {self.guard(check)}
    fn completed(&mut self) {}
}
fn dispatch_plan(wire:&mut impl StrokeWire,map:&xkb::Keymap,plan:&KeyPlan,check:&mut dyn FnMut()->anyhow::Result<()>,progress:&mut dyn FnMut(usize))->anyhow::Result<()> {
    let mut state=xkb::State::new(map); state.update_mask(0,0,0,0,0,plan.group);
    for unit in &plan.units {
        for code in unit {
            wire.guard(check)?;
            state.update_key(xkb::Keycode::new(*code),xkb::KeyDirection::Down);
            wire.stroke(*code,true,&state)?;
        }
        for code in unit.iter().rev() {
            wire.guard(check)?;
            state.update_key(xkb::Keycode::new(*code),xkb::KeyDirection::Up);
            wire.stroke(*code,false,&state)?;
        }
        wire.ack(check)?; // ACK releases separately from later snapshot/guard failures.
        wire.completed();
        progress(1); // Delta: one fully released, compositor-acknowledged unit.
    }
    Ok(())
}
/// Cheap cancellation/deadline checks for observation waits, and an uncached
/// authoritative foreground check immediately before every injection request.
pub(super) struct Checks<'a> {
    pub(super) wait:&'a mut dyn FnMut()->anyhow::Result<()>,
    pub(super) full:&'a mut dyn FnMut()->anyhow::Result<()>,
}
/// One observed map and connection for the entire preflighted batch.
pub(super) struct KeyboardSession {
    conn:Connection, queue:wayland_client::EventQueue<State>, state:State,
    original:xkb::Keymap, source:String, group:u32,
    keyboard:Option<keyboard::ZwpVirtualKeyboardV1>, file:Option<std::fs::File>,
    held:Vec<u32>, start:Instant, failed:bool, finished:bool, pending_neutral:bool, facts:KeyboardFailureFacts,
}
impl KeyboardSession {
    /// Read-only observation: pass the cheap cancellation/deadline check.
    pub(super) fn open(check:&mut dyn FnMut()->anyhow::Result<()>) -> anyhow::Result<Self> {
        check()?;
        let conn=Connection::connect_to_env()?;
        let mut queue=conn.new_event_queue::<State>();
        conn.display().get_registry(&queue.handle(),());
        let mut state=State::default();
        for _ in 0..4 {sync(&conn,&mut queue,&mut state,&mut *check)?;}
        ensure!(state.seats.len()==1,"Keyboard input requires one unambiguous Wayland seat");
        ensure!(state.manager.is_some(),"Virtual keyboard protocol unavailable");
        if let Some(error)=&state.error {anyhow::bail!("{error}");}
        let map=state.map.as_ref().context("No compositor keyboard map")?;
        validate_baseline(map)?;
        let group=match state.group {Some(g)=>g,None if map.num_layouts()==1=>0,None=>anyhow::bail!("Compositor did not provide active layout group")};
        ensure!(group<map.num_layouts(),"Invalid active layout group");
        ensure!(state.observed_mods.unwrap_or(0)==0,"Observed active modifiers make keyboard baseline unsafe");
        let source=state.canonical_map.clone().context("No validated compositor map source")?;
        let original=map.clone();
        check()?;
        Ok(Self {conn,queue,state,original,source,group,keyboard:None,file:None,held:Vec::new(),start:Instant::now(),failed:false,finished:false,pending_neutral:false,facts:KeyboardFailureFacts::default()})
    }
    pub(super) fn plan_text(&self,text:&str)->anyhow::Result<Option<KeyPlan>> {ensure!(!self.failed && !self.finished,"Keyboard session is no longer active");layout_plan::text(&self.original,self.group,text)}
    pub(super) fn plan_keys(&self,keys:&[enigo::Key])->anyhow::Result<KeyPlan> {ensure!(!self.failed && !self.finished,"Keyboard session is no longer active");layout_plan::keys(&self.original,self.group,keys)}
    /// Read-only runtime preflight before another transport publishes anything.
    /// This never creates a virtual keyboard or replans an existing plan.
    pub(super) fn revalidate(&mut self,plan:&KeyPlan,guard:&mut dyn FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
        catch_native(|| {
            ensure!(!self.failed && !self.finished,"Keyboard session is no longer active");
            ensure!(plan.source==self.source && plan.group==self.group,"Plan belongs to a different layout snapshot");
            self.guard(guard)
        }).map_err(|error|keyboard_error(error,KeyboardFailureFacts::default()))
    }
    fn guard(&mut self,check:&mut dyn FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
        check()?;
        // Rebind an observer: a roundtrip alone cannot turn a cached keymap into
        // a fresh snapshot on compositors that only announce maps at binding.
        let revision=self.state.map_revision;
        let group_revision=self.state.group_revision;
        let observer=self.state.seats[0].get_keyboard(&self.queue.handle(),());
        super::latency::count("keyboard_observers",1);
        let observed=catch_native(||sync(&self.conn,&mut self.queue,&mut self.state,&mut *check));
        observer.release();
        observed?;
        ensure!(self.state.map_revision!=revision,"Compositor did not refresh the keyboard snapshot");
        ensure!(self.original.num_layouts()==1 || self.state.group_revision!=group_revision,"Compositor did not refresh the active layout group");
        ensure!(self.state.error.is_none(),"Keyboard observation failed");
        ensure!(self.state.canonical_map.as_deref()==Some(self.source.as_str()),"Desktop layout changed; input stopped without replanning");
        ensure!(self.state.group.unwrap_or(self.group)==self.group,"Desktop layout group changed; input stopped without replanning");
        if self.held.is_empty() {
            if self.pending_neutral && self.state.observed_mods.is_some_and(|mods|mods!=0) {
                // Fcitx forwards modifiers on another connection. Our release ACK
                // does not ACK that forwarding. Read only, with cancellation and
                // a hard bound; never clear or guess physical modifier state.
                let deadline=Instant::now()+Duration::from_millis(500);
                while self.state.observed_mods.is_some_and(|mods|mods!=0) {
                    check()?;
                    pump(&self.conn,&mut self.queue,&mut self.state,deadline)?;
                    ensure!(self.state.error.is_none() && self.state.canonical_map.as_deref()==Some(self.source.as_str()),"Desktop layout changed while awaiting neutral modifiers");
                    ensure!(self.state.group.unwrap_or(self.group)==self.group,"Desktop layout group changed while awaiting neutral modifiers");
                }
            }
            ensure!(self.state.observed_mods.unwrap_or(0)==0,"Observed active modifiers are unsafe");
            self.pending_neutral=false;
        }
        check()
    }
    fn masks(&self,s:&xkb::State) {
        self.keyboard.as_ref().unwrap().modifiers(s.serialize_mods(xkb::STATE_MODS_DEPRESSED),s.serialize_mods(xkb::STATE_MODS_LATCHED),s.serialize_mods(xkb::STATE_MODS_LOCKED),self.group);
    }
    fn cleanup(&mut self)->anyhow::Result<()> {
        super::latency::measure("keyboard_cleanup",|| {
            let mut error=None;
            if let Some(k)=self.keyboard.take() {
                for code in self.held.drain(..).rev() {if let Err(e)=catch_native(||{k.key(self.start.elapsed().as_millis() as u32,code-8,0);super::latency::count("keyboard_edges",1);Ok(())}) {error=Some(e);}}
                // Only this owned device is neutralized. Physical state is unknown.
                if let Err(e)=catch_native(||{k.modifiers(0,0,0,self.state.group.unwrap_or(self.group));Ok(())}) {error=Some(e);}
                if let Err(e)=catch_native(||sync(&self.conn,&mut self.queue,&mut self.state,||Ok(()))) {error=Some(e);}
                if let Err(e)=catch_native(||{k.destroy();Ok(())}) {error=Some(e);}
                if let Err(e)=catch_native(||sync(&self.conn,&mut self.queue,&mut self.state,||Ok(()))) {error=Some(e);}
            }
            self.file=None;
            match error {Some(e)=>Err(e),None=>Ok(())}
        })
    }
    /// Required before reporting successful input/batch completion. Drop is only
    /// a best-effort fallback; this exposes bounded native cleanup ACK failures.
    pub(super) fn finish(&mut self)->anyhow::Result<()> {
        if self.facts.cleanup_failed {
            return Err(keyboard_error(anyhow::anyhow!("Previous keyboard cleanup failed"),self.facts));
        }
        if self.finished {return Ok(());}
        let result=catch_native(||self.cleanup());
        self.finished=true;
        result.map_err(|error| {
            self.failed=true;
            self.facts.cleanup_failed=true;
            // Cleanup failure cannot invent an unacknowledged character.
            keyboard_error(error,self.facts)
        })
    }
    pub(super) fn execute(&mut self,plan:&KeyPlan,check:&mut dyn FnMut()->anyhow::Result<()>,progress:&mut dyn FnMut(usize))->anyhow::Result<()> {
        // Compatibility: callers with one closure retain all checks in waits.
        let shared=std::cell::RefCell::new(check);
        self.execute_with_checks(plan,&mut Checks {wait:&mut ||(shared.borrow_mut())(),full:&mut ||(shared.borrow_mut())()},progress)
    }
    pub(super) fn execute_with_checks(&mut self,plan:&KeyPlan,checks:&mut Checks<'_>,progress:&mut dyn FnMut(usize))->anyhow::Result<()> {
        if self.failed || self.finished {
            return Err(keyboard_error(anyhow::anyhow!("Keyboard session is no longer active"),KeyboardFailureFacts::default()));
        }
        self.facts=KeyboardFailureFacts::default();
        let result=catch_native(|| {
            self.revalidate(plan,checks.wait)?;
            if plan.units.is_empty() {return Ok(());}
            if self.keyboard.is_none() {
                self.file=Some(keymap_file(&self.source)?);
                (checks.full)()?;
                self.keyboard=Some(self.state.manager.as_ref().unwrap().create_virtual_keyboard(&self.state.seats[0],&self.queue.handle(),()));
                self.guard(checks.wait)?;
                (checks.full)()?;
                self.keyboard.as_ref().unwrap().keymap(1,self.file.as_ref().unwrap().as_fd(),(self.source.len()+1) as u32);
                self.guard(checks.wait)?;
                (checks.full)()?;
                self.keyboard.as_ref().unwrap().modifiers(0,0,0,self.group);
            }
            let map=self.original.clone();
            dispatch_plan(&mut CheckedStroke {session:self,checks},&map,plan,&mut ||Ok(()),progress)?;
            self.guard(checks.wait)?;
            Ok(())
        });
        if let Err(error)=result {
            self.failed=true;
            let source=match self.cleanup() {Ok(())=>error,Err(cleanup)=>{self.facts.cleanup_failed=true;anyhow::anyhow!("{error:#}; cleanup failed: {cleanup:#}")}};
            return Err(keyboard_error(source,self.facts));
        }
        Ok(())
    }
}
// The shared engine still re-observes the original map/group for every edge.
// No full foreground queries run inside that read-only observation loop.
struct CheckedStroke<'a,'b> {session:&'a mut KeyboardSession,checks:&'a mut Checks<'b>}
impl StrokeWire for CheckedStroke<'_,'_> {
    fn guard(&mut self,_:&mut dyn FnMut()->anyhow::Result<()>)->anyhow::Result<()> {self.session.guard(self.checks.wait)}
    fn ack(&mut self,_:&mut dyn FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
        sync(&self.session.conn,&mut self.session.queue,&mut self.session.state,&mut *self.checks.wait)
    }
    fn completed(&mut self) {self.session.facts.completed_units+=1;self.session.facts.current_unit_uncertain=false;}
    fn stroke(&mut self,code:u32,down:bool,state:&xkb::State)->anyhow::Result<()> {
        let session=&mut self.session;
        if down {
            (self.checks.full)()?;
            session.masks(state);
            session.pending_neutral|=state.serialize_mods(xkb::STATE_MODS_EFFECTIVE)!=0;
        }
        (self.checks.full)()?;
        if down {session.held.push(code);session.facts.current_unit_uncertain=true;}
        session.keyboard.as_ref().unwrap().key(session.start.elapsed().as_millis() as u32,code-8,u32::from(down));
        super::latency::count("keyboard_edges",1);
        if !down {
            session.held.pop();
            (self.checks.full)()?;
            session.masks(state);
        }
        Ok(())
    }
}
impl Drop for KeyboardSession {fn drop(&mut self) {let _=catch_native(||self.cleanup());}}
pub(super) fn send(text:&str,mut check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    ensure!(text.chars().count()<=512 && !text.chars().any(char::is_control),"Invalid native text");
    ensure!(!text.chars().any(|ch|u32::from(ch)>0xffff),"Native keyboard cannot safely deliver supplementary Unicode; select clipboard explicitly");
    if text.is_empty() {return Ok(());}
    let mut session=KeyboardSession::open(&mut check)?;
    let plan=session.plan_text(text)?.context("Text is not representable in the original active layout")?;
    session.execute(&plan,&mut check,&mut |_|{})?;
    session.finish()
}
pub(super) fn send_keys(keys:&[enigo::Key],mut check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    let mut session=KeyboardSession::open(&mut check)?;
    let plan=session.plan_keys(keys)?;
    session.execute(&plan,&mut check,&mut |_|{})?;
    session.finish()
}
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
