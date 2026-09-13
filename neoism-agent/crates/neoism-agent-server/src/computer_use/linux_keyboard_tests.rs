//! Deterministic IME fixture: queued events apply only on sync. Like Hyprland's
//! IME grab, map forwarding is cached by device identity, not map revision.
//! Parsing, key resolution, modifier state and production transaction are real.
//! The standalone dependency harness supplies Enigo's exact FD loader as parser.
use super::*;
use enigo::Key;

enum Pending { Map(String), Restore(String), Mods(u32,u32,u32,u32), Key(u32,bool), Destroy }
// Fcitx's return connection runs one sync behind the injector connection.
// A compositor ACK must not implicitly acknowledge these requests too.
enum Returned { Map(String), Mods(u32,u32,u32,u32), Key(String,bool) }
struct Injector { id:u64, map:String, state:xkb::State }
struct Compositor<'a> {
    baseline:String, current:String, state:xkb::State, pending:Vec<Pending>, returned:Vec<Returned>,
    injector:Option<Injector>, next_id:u64, ime_device:Option<u64>, ime_map:Option<String>, ime_state:xkb::State,
    calls:Vec<&'static str>, output:Vec<(String,bool)>,
    parser:&'a mut dyn FnMut(&str)->anyhow::Result<()>, failure:u8, fail_sync_at:usize, sync_count:usize,
}
impl<'a> Compositor<'a> {
    fn new(source:&str,parser:&'a mut dyn FnMut(&str)->anyhow::Result<()>)->Self {
        Self {baseline:source.into(),current:source.into(),state:xkb::State::new(&compile(source.into()).unwrap()),pending:Vec::new(),returned:Vec::new(),injector:None,next_id:1,ime_device:None,ime_map:None,ime_state:xkb::State::new(&compile(source.into()).unwrap()),calls:Vec::new(),output:Vec::new(),parser,failure:0,fail_sync_at:0,sync_count:0}
    }
    fn create_injector(&mut self,source:String)->anyhow::Result<()> {
        let state=xkb::State::new(&compile(source.clone())?);
        self.injector=Some(Injector {id:self.next_id,map:source,state});
        self.next_id+=1;
        Ok(())
    }
    // Hyprland forwards keyboard data on routed modifiers/keys, and only
    // when the originating device identity differs from the grab's cache.
    fn route_keyboard(&mut self)->anyhow::Result<()> {
        let injector=self.injector.as_ref().expect("event on destroyed injector");
        if self.ime_device!=Some(injector.id) {
            self.ime_state=xkb::State::new(&compile(injector.map.clone())?);
            self.ime_device=Some(injector.id);
            self.ime_map=Some(injector.map.clone());
            self.returned.push(Returned::Map(injector.map.clone()));
        }
        Ok(())
    }
    fn assert_restored(&self) {
        assert_eq!(self.current,self.baseline);
        assert_eq!(self.ime_map.as_deref(),Some(self.baseline.as_str()));
        assert!(self.injector.is_none(),"owned injector must be destroyed");
        assert!(self.pending.is_empty(),"returning with queued cleanup is not restoration");
        assert!(self.returned.is_empty(),"Fcitx return updates must also be drained");
        assert_eq!(self.state.serialize_mods(xkb::STATE_MODS_EFFECTIVE),0);
        assert_eq!(&self.calls[self.calls.len()-5..],&["map","mods","sync","destroy","sync"]);
    }
}
impl Wire for Compositor<'_> {
    fn map(&mut self,source:&str)->anyhow::Result<()> {
        (self.parser)(source)?;
        self.calls.push("map");self.pending.push(Pending::Map(source.into()));Ok(())
    }
    fn restore(&mut self,source:&str)->anyhow::Result<()> {
        (self.parser)(source)?;
        self.calls.push("map");self.pending.push(Pending::Restore(source.into()));Ok(())
    }
    fn mods(&mut self,d:u32,l:u32,k:u32,g:u32)->anyhow::Result<()> {
        self.calls.push("mods");self.pending.push(Pending::Mods(d,l,k,g));Ok(())
    }
    fn key(&mut self,code:u32,down:bool)->anyhow::Result<()> {
        self.calls.push(if down {"down"} else {"up"});self.pending.push(Pending::Key(code,down));
        if down && self.failure!=0 {
            let failure=std::mem::take(&mut self.failure);
            if failure==2 {panic!("in-flight key panic");}
            anyhow::bail!("partially accepted key error");
        }
        Ok(())
    }
    fn destroy(&mut self)->anyhow::Result<()> {
        // Only our own device is acknowledged at this point. Fcitx's
        // baseline return may still be queued on its separate connection.
        let injector=self.injector.as_ref().expect("destroy requires a live injector");
        assert_eq!(injector.map,self.baseline);
        assert_eq!(injector.state.serialize_mods(xkb::STATE_MODS_EFFECTIVE),0);
        self.calls.push("destroy");self.pending.push(Pending::Destroy);Ok(())
    }
    fn sync(&mut self)->anyhow::Result<()> {
        self.calls.push("sync"); self.sync_count+=1;
        // Only return requests queued by an EARLIER sync run here.
        for event in std::mem::take(&mut self.returned) {
            match event {
                Returned::Map(source)=>{
                    self.state=xkb::State::new(&compile(source.clone())?);self.current=source;
                },
                Returned::Mods(d,l,k,g)=>{self.state.update_mask(d,l,k,0,0,g);},
                Returned::Key(text,control)=>self.output.push((text,control)),
            }
        }
        for event in std::mem::take(&mut self.pending) {
            match event {
                Pending::Map(source)=>{
                    if let Some(injector)=&mut self.injector {
                        injector.state=xkb::State::new(&compile(source.clone())?);
                        injector.map=source;
                    } else {
                        // Each transaction owns a new protocol object, even
                        // though the fixture's Wire is reused by sequence().
                        self.create_injector(source)?;
                    }
                },
                Pending::Restore(source)=>{
                    // Create + baseline map + destroy previous. None of those
                    // operations forwards replacement data to the IME grab.
                    self.create_injector(source)?;
                },
                Pending::Mods(d,l,k,g)=>{
                    self.injector.as_mut().expect("modifiers need an injector").state.update_mask(d,l,k,0,0,g);
                    self.route_keyboard()?;
                    self.ime_state.update_mask(d,l,k,0,0,g);
                    self.returned.push(Returned::Mods(d,l,k,g));
                },
                Pending::Key(code,down)=>{
                    self.route_keyboard()?;
                    let code=xkb::Keycode::new(code);
                    if down {
                        let sym=self.ime_state.key_get_one_sym(code);
                        let control=self.ime_state.mod_name_is_active(xkb::MOD_NAME_CTRL,xkb::STATE_MODS_EFFECTIVE);
                        let text=match sym.raw() {0xff0d=>"<Return>".into(),0xffe3=>"<Control>".into(),_=>xkb::keysym_to_utf8(sym)};
                        self.returned.push(Returned::Key(text,control));
                    }
                },
                // Removing an injector does NOT reset the grab cache or the
                // surviving Fcitx VK's map. Only a later routed identity does.
                Pending::Destroy=>{self.injector=None;},
            }
        }
        ensure!(self.sync_count!=self.fail_sync_at,"simulated missing compositor ACK");
        Ok(())
    }
}
fn desktop(layout:&str)->xkb::Keymap {
    xkb::Keymap::new_from_names(&xkb::Context::new(xkb::CONTEXT_NO_FLAGS),"","",layout,"",None,xkb::KEYMAP_COMPILE_NO_FLAGS).unwrap()
}
fn chord(map:&xkb::Keymap,keys:&[Key])->Vec<u32> {
    super::super::shortcuts::resolve_in_map(map,0,keys).unwrap().into_iter().map(u32::from).collect()
}

pub(crate) fn sequence(mut parser:impl FnMut(&str)->anyhow::Result<()>) {
    for layout in ["us","de","fr"] {
        let base=desktop(layout);
        let original=base.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
        let mut wire=Compositor::new(&original,&mut parser);
        for url in ["https://www.google.com/search?q=Neoism+AA!!", "https://example.com/?q=λ❤é&&x=RepeatRepeat"] {
            let (overlay,codes)=text_overlay(&base,url).unwrap();
            let overlay_map=compile(overlay.clone()).unwrap();
            // Modifier and control keys stay intact during literal typing.
            // Printable keys are temporarily remapped; subsequent shortcuts
            // must use the restored baseline, never the literal map.
            for keys in [&[Key::Return][..],&[Key::Control][..],&[Key::Shift][..],&[Key::Meta][..]] {
                assert_eq!(chord(&base,keys),chord(&overlay_map,keys));
            }
            assert!(validate_baseline(&overlay_map).is_err(),"must not snapshot our overlay as original");
            let from=wire.output.len();
            transaction(&mut wire,&original,&overlay,0,&codes,false,||Ok(())).unwrap();
            assert_eq!(wire.output[from..].iter().map(|(s,_)|s.as_str()).collect::<String>(),url);
            assert!(wire.output[from..].iter().all(|(_,ctrl)|!*ctrl));
            wire.assert_restored();
            let enter=chord(&base,&[Key::Return]);
            transaction(&mut wire,&original,&original,0,&enter,true,||Ok(())).unwrap();
            assert_eq!(wire.output.last(),Some(&("<Return>".into(),false)));
            wire.assert_restored();
            let control_l=chord(&base,&[Key::Control,Key::Unicode('l')]);
            transaction(&mut wire,&original,&original,0,&control_l,true,||Ok(())).unwrap();
            assert_eq!(wire.output.last(),Some(&("l".into(),true)));
            wire.assert_restored();
        }
        let (overlay,codes)=text_overlay(&base,"abcλ").unwrap();
        for failure in [0,1,2] {
            wire.failure=failure;
            let mut checks=0;
            let from=wire.output.len();
            let result=transaction(&mut wire,&original,&overlay,0,&codes,false,||{
                checks+=1;
                ensure!(failure!=0 || checks<3,"cancelled after first character");Ok(())
            });
            assert!(result.is_err());wire.assert_restored();
            assert_eq!(wire.output.len()-from,1,"no subsequent text after failure/cancel");
            let enter=chord(&base,&[Key::Return]);
            transaction(&mut wire,&original,&original,0,&enter,true,||Ok(())).unwrap();
            assert_eq!(wire.output.last(),Some(&("<Return>".into(),false)));
            wire.assert_restored();
        }
        let control_l=chord(&base,&[Key::Control,Key::Unicode('l')]);
        let mut checks=0;
        assert!(transaction(&mut wire,&original,&original,0,&control_l,true,||{
            checks+=1;ensure!(checks<3,"cancel after Control down");Ok(())
        }).is_err());
        wire.assert_restored();
        transaction(&mut wire,&original,&original,0,&control_l,true,||Ok(())).unwrap();
        assert_eq!(wire.output.last(),Some(&("l".into(),true)));wire.assert_restored();
        let (overlay,codes)=text_overlay(&base,"x").unwrap();
        wire.fail_sync_at=wire.sync_count+3; // map ACK, key ACK, then restore ACK fails
        assert!(transaction(&mut wire,&original,&overlay,0,&codes,false,||Ok(())).unwrap_err().to_string().contains("not acknowledged"));
        wire.assert_restored(); // destroy and its ACK still attempted
    }
}
#[test] fn ime_identity_cache_requires_a_fresh_restore_device() {
    let base=desktop("us");
    let original=base.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let (overlay,_)=text_overlay(&base,"abc").unwrap();
    let mut parser=|source:&str| {compile(source.into())?;Ok(())};
    let mut wire=Compositor::new(&original,&mut parser);
    wire.map(&overlay).unwrap();wire.mods(0,0,0,0).unwrap();wire.sync().unwrap();
    let old_id=wire.injector.as_ref().unwrap().id;
    assert_eq!(wire.current,original,"injector ACK does not drain Fcitx's new return requests");
    wire.map(&original).unwrap();wire.mods(0,0,0,0).unwrap();wire.sync().unwrap();
    assert_eq!(wire.injector.as_ref().unwrap().id,old_id);
    assert_eq!(wire.injector.as_ref().unwrap().map,original);
    assert_eq!(wire.ime_map.as_deref(),Some(overlay.as_str()));
    assert_eq!(wire.current,overlay,"same-device restore is invisible to the IME");
    wire.restore(&original).unwrap();wire.mods(0,0,0,0).unwrap();wire.sync().unwrap();
    assert_ne!(wire.injector.as_ref().unwrap().id,old_id);
    assert_eq!(wire.ime_map.as_deref(),Some(original.as_str()));
    assert_eq!(wire.current,overlay,"replacement ACK still precedes Fcitx's return");
    wire.destroy().unwrap();wire.sync().unwrap();wire.assert_restored();
    assert!(wire.output.is_empty(),"restoration must never manufacture a keypress");
    assert!(!wire.calls.contains(&"down"),"no keydown even before asynchronous delivery");
}

#[test] fn replacement_without_modifiers_does_not_restore_ime_or_surviving_vk() {
    let base=desktop("us");
    let original=base.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let (overlay,_)=text_overlay(&base,"λ").unwrap();
    let mut parser=|source:&str| {compile(source.into())?;Ok(())};
    let mut wire=Compositor::new(&original,&mut parser);
    wire.map(&overlay).unwrap();wire.mods(0,0,0,0).unwrap();wire.sync().unwrap();
    wire.sync().unwrap();
    let cached_id=wire.ime_device;
    wire.restore(&original).unwrap();wire.sync().unwrap();
    let replacement_id=wire.injector.as_ref().unwrap().id;
    assert_ne!(Some(replacement_id),cached_id);
    assert_eq!(wire.injector.as_ref().unwrap().map,original);
    assert_eq!(wire.ime_device,cached_id,"replacement map alone cannot refresh grab identity");
    assert_eq!(wire.ime_map.as_deref(),Some(overlay.as_str()));
    wire.destroy().unwrap();wire.sync().unwrap();wire.sync().unwrap();
    assert_eq!(wire.current,overlay,"destroy must not invent a physical-map fallback");
    assert_eq!(wire.ime_map.as_deref(),Some(overlay.as_str()));
    assert!(wire.pending.is_empty() && wire.returned.is_empty());
    assert!(wire.output.is_empty() && !wire.calls.contains(&"down"));

    // A subsequent transaction is a different device, so its ordinary
    // baseline + modifiers can now refresh the otherwise persistent cache.
    wire.map(&original).unwrap();wire.mods(0,0,0,0).unwrap();wire.sync().unwrap();
    assert_ne!(wire.injector.as_ref().unwrap().id,replacement_id);
    assert_ne!(wire.ime_device,cached_id);
    assert_eq!(wire.current,overlay);
    wire.destroy().unwrap();wire.sync().unwrap();wire.assert_restored();
    assert!(wire.output.is_empty() && !wire.calls.contains(&"down"));
}

#[test] fn routed_key_refreshes_identity_but_map_publication_alone_does_not() {
    let base=desktop("us");
    let original=base.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let (overlay,codes)=text_overlay(&base,"λ").unwrap();
    let mut parser=|source:&str| {compile(source.into())?;Ok(())};
    let mut wire=Compositor::new(&original,&mut parser);
    wire.map(&overlay).unwrap();wire.sync().unwrap();
    assert!(wire.ime_device.is_none() && wire.ime_map.is_none());
    assert_eq!(wire.current,original);
    // This is the intended input, not a restoration key. A routed key is
    // also an IME identity-selection event even with no preceding modifiers.
    wire.key(codes[0],true).unwrap();wire.key(codes[0],false).unwrap();wire.sync().unwrap();
    assert_eq!(wire.ime_device,Some(wire.injector.as_ref().unwrap().id));
    assert_eq!(wire.ime_map.as_deref(),Some(overlay.as_str()));
    assert!(wire.output.is_empty(),"return connection is still queued");
    wire.sync().unwrap();
    assert_eq!(wire.output,vec![("λ".into(),false)]);
    let restore_start=wire.calls.len();
    wire.restore(&original).unwrap();wire.mods(0,0,0,0).unwrap();wire.sync().unwrap();
    wire.destroy().unwrap();wire.sync().unwrap();wire.assert_restored();
    assert!(!wire.calls[restore_start..].contains(&"down"));
    assert_eq!(wire.output,vec![("λ".into(),false)],"restore adds no input");
}

#[test] fn focus_loss_mid_unicode_text_stops_and_restores_keyboard() {
    let base=desktop("us");
    let original=base.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let (overlay,codes)=text_overlay(&base,"λ❤abc").unwrap();
    let mut parser=|source:&str| {compile(source.into())?;Ok(())};
    let mut wire=Compositor::new(&original,&mut parser);
    let mut checks=0;
    let result=transaction(&mut wire,&original,&overlay,0,&codes,false,|| {
        checks+=1;
        // Map publication check, first character check, then foreground lost
        // before character two. Production calls windows::validate here.
        ensure!(checks<3,"Target is not foreground (Neoism replaced Helium)");
        Ok(())
    });
    assert!(result.unwrap_err().to_string().contains("not foreground"));
    assert_eq!(wire.output,vec![("λ".into(),false)]);
    wire.assert_restored();
}
#[test] fn native_text_enter_control_l_text_enter_and_error_restoration() {sequence(|source|{compile(source.into())?;Ok(())});}
#[test] fn old_text_only_map_cannot_be_a_restore_baseline() {
    let (source,_)=compiled_keymap("https://www.google.com/search?q=neoism").unwrap();
    let unnamed=compile(source.replace("\"neoism_text\"", "")).unwrap();
    assert!(validate_baseline(&unnamed).is_err(),"legacy unnamed text maps are also not valid baselines");
    let map=compile(source).unwrap();
    assert!(super::super::shortcuts::resolve_in_map(&map,0,&[Key::Return]).is_err());
    assert!(validate_baseline(&map).unwrap_err().to_string().contains("temporary Neoism keymap"));
}
#[test] fn unicode_chunks_use_only_browser_recognized_positions() {
    for layout in ["us","de","fr","us,de"] {
        let base=desktop(layout);
        let chars=(0..512).map(|i|char::from_u32(0x400+i).unwrap()).collect::<String>();
        let chunks=text_chunks(&chars,literal_keycodes(&base).len()).unwrap();
        assert!(chunks.len()>1);
        assert_eq!(chunks.concat(),chars);
        for chunk in chunks {
            let (source,codes)=text_overlay(&base,chunk).unwrap();
            let map=compile(source).unwrap();
            assert_eq!(map.max_keycode(),base.max_keycode());
            for group in 0..base.num_layouts() {
                let mut state=xkb::State::new(&map);
                state.update_mask(0,0,0,0,0,group);
                for (ch,code) in chunk.chars().zip(&codes) {
                    assert!(LITERAL_KEYCODES.contains(code));
                    assert_eq!(state.key_get_utf8(xkb::Keycode::new(*code)),ch.to_string());
                }
            }
        }
        assert!(text_overlay(&base,&chars).is_err(),"one map must not exceed the recognized pool");
    }
}
#[test] fn text_preflight_rejects_implicit_control_keys_and_preserves_unicode_boundaries() {
    for text in ["abc\n", "abc\r", "abc\t", "\0", "\u{7f}", "\u{85}"] {
        assert!(text_chunks(text,48).is_err());
    }
    assert!(text_chunks(&"a".repeat(513),48).is_err());
    assert!(text_chunks("a",0).is_err());
    let calls=std::cell::Cell::new(0);
    let error=send("prefix 🦀",||{calls.set(calls.get()+1);Ok(())}).unwrap_err();
    assert!(error.to_string().contains("supplementary Unicode"));
    assert_eq!(calls.get(),0,"unsupported text must fail before connecting or dispatching its prefix");
    let text="aéé中❤e\u{301}क्\u{200d}षאבג";
    let chunks=text_chunks(text,2).unwrap();
    assert_eq!(chunks.concat(),text);
    assert!(chunks.iter().all(|chunk|chunk.chars().collect::<std::collections::HashSet<_>>().len()<=2));
    assert_eq!(text_chunks("aaaaaaaa",1).unwrap(),vec!["aaaaaaaa"]);
}
#[test] fn cancellation_after_a_chunk_reports_partial_input_without_starting_the_next() {
    let base=desktop("us");
    let original=base.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let prepared=vec![text_overlay(&base,"first").unwrap(),text_overlay(&base,"second").unwrap()];
    let completed=std::cell::Cell::new(0);
    let mut parser=|source:&str|{compile(source.into())?;Ok(())};
    let mut wire=Compositor::new(&original,&mut parser);
    let error=run_chunks(&prepared,||{
        ensure!(completed.get()==0,"cancelled");Ok(())
    },|source,codes,check| {
        transaction(&mut wire,&original,source,0,codes,false,check)?;
        completed.set(completed.get()+1);Ok(())
    }).unwrap_err();
    assert!(format!("{error:#}").contains("Input may already have been delivered"));
    assert!(format!("{error:#}").contains("cancelled"));
    assert_eq!(completed.get(),1);
    assert_eq!(wire.output.iter().map(|(text,_)|text.as_str()).collect::<String>(),"first");
    wire.assert_restored();
}
#[test] fn literal_overlay_neutralizes_custom_printable_modifier_actions() {
    let mut source=desktop("us,de").get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    append_section(&mut source,"xkb_symbols","\n replace key <AE01> { symbols[Group1]=[ Control_L ], actions[Group1]=[ SetMods(modifiers=Control) ], symbols[Group2]=[ Control_L ], actions[Group2]=[ SetMods(modifiers=Control) ] }; modifier_map Control { <AE01> };").unwrap();
    let base=compile(source).unwrap();
    let baseline=base.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let (source,codes)=text_overlay(&base,"λ").unwrap();
    assert_eq!(codes,vec![10]);
    let map=compile(source.clone()).unwrap();
    for group in 0..2 {
        let mut state=xkb::State::new(&map);
        state.update_mask(0,0,0,0,0,group);
        state.update_key(xkb::Keycode::new(10),xkb::KeyDirection::Down);
        assert_eq!(state.key_get_utf8(xkb::Keycode::new(10)),"λ");
        assert_eq!(state.serialize_mods(xkb::STATE_MODS_EFFECTIVE),0);
        assert_eq!(map.key_get_syms_by_level(xkb::Keycode::new(24),group,0),base.key_get_syms_by_level(xkb::Keycode::new(24),group,0));
    }
    let mut parser=|source:&str|{compile(source.into())?;Ok(())};
    let mut wire=Compositor::new(&baseline,&mut parser);
    transaction(&mut wire,&baseline,&source,0,&codes,false,||Ok(())).unwrap();
    wire.assert_restored();
}

#[test] fn chunked_transactions_restore_before_followup_shortcuts() {
    let base=desktop("us");
    let original=base.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let text=(0..120).map(|i|char::from_u32(0x400+i).unwrap()).collect::<String>();
    // This is the same full preflight used by perform: prepare all maps first.
    let prepared=text_chunks(&text,literal_keycodes(&base).len()).unwrap().into_iter()
        .map(|chunk|text_overlay(&base,chunk)).collect::<anyhow::Result<Vec<_>>>().unwrap();
    let mut parser=|source:&str|{compile(source.into())?;Ok(())};
    let mut wire=Compositor::new(&original,&mut parser);
    for (source,codes) in prepared {
        transaction(&mut wire,&original,&source,0,&codes,false,||Ok(())).unwrap();
        wire.assert_restored();
    }
    assert_eq!(wire.output.iter().map(|(text,_)|text.as_str()).collect::<String>(),text);
    let control_l=chord(&base,&[Key::Control,Key::Unicode('l')]);
    transaction(&mut wire,&original,&original,0,&control_l,true,||Ok(())).unwrap();
    assert_eq!(wire.output.last(),Some(&("l".into(),true)));
    wire.assert_restored();
}
