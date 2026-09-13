//! Historical overlay/IME fixtures plus original-layout planner and stroke tests.
//! Overlay transactions below are cfg(test)-only; production never remaps keys.
//! Parsing, key resolution and modifier simulation use real native XKB maps.
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

// Original-layout production planner regressions (overlay tests above historical).
fn original_layout(layout:&str)->xkb::Keymap {
    xkb::Keymap::new_from_names(&xkb::Context::new(xkb::CONTEXT_NO_FLAGS),"","",layout,"",None,xkb::KEYMAP_COMPILE_NO_FLAGS).unwrap()
}
fn assert_original_text(layout:&str,group:u32,text:&str) {
    let map=original_layout(layout);
    let source=map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let plan=layout_plan::text(&map,group,text).unwrap().unwrap();
    assert_eq!(plan.unit_count(),text.chars().count());
    assert_eq!(plan.units_kind,"unicode_scalar");
    assert_eq!(plan.source,source);
    assert!(!plan.source.contains("neoism_overlay"));
    let mut state=xkb::State::new(&map); state.update_mask(0,0,0,0,0,group);
    for (ch,unit) in text.chars().zip(&plan.units) {
        for code in unit {state.update_key(xkb::Keycode::new(*code),xkb::KeyDirection::Down);}
        assert_eq!(state.key_get_utf8(xkb::Keycode::new(*unit.last().unwrap())),ch.to_string());
        for code in unit.iter().rev() {state.update_key(xkb::Keycode::new(*code),xkb::KeyDirection::Up);}
        assert_eq!(state.serialize_mods(xkb::STATE_MODS_EFFECTIVE),0);
        assert_eq!(state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),group);
    }
}
#[test] fn original_us_de_fr_case_punctuation_and_level3() {
    assert_original_text("us",0,"Hello AaZz 0123456789 !@#$%^&*()_+-=[]{};:'\",.<>/?\\|`");
    assert_original_text("de",0,"Hallo ÄäÖöÜüß @€{}[]\\|~");
    assert_original_text("fr",0,"Bonjour AaZz éèàç @€{}[]\\|~");
}
#[test] fn original_group_only_and_dead_keys_not_composed() {
    let map=original_layout("us,de");
    assert!(layout_plan::text(&map,0,"ä").unwrap().is_none());
    assert_original_text("us,de",1,"äÄ@€");
    assert!(layout_plan::text(&original_layout("us"),0,"café").unwrap().is_none());
    assert!(layout_plan::text(&original_layout("de"),0,"ê").unwrap().is_none());
}
#[test] fn original_full_preflight_validates_suffix_and_unicode_policy() {
    let map=original_layout("us");
    for text in ["prefixλ","prefix😀","e\u{301}","a\u{200d}"] {assert!(layout_plan::text(&map,0,text).unwrap().is_none());}
    for text in ["prefix\0","prefix\n","prefix\t","prefix\u{85}","😀\n"] {assert!(layout_plan::text(&map,0,text).is_err());}
    assert_eq!(layout_plan::text(&map,0,&"a".repeat(512)).unwrap().unwrap().unit_count(),512);
    assert!(layout_plan::text(&map,0,&"a".repeat(513)).is_err());
}
#[test] fn original_custom_modifier_poison_rejected() {
    let map=original_layout("de");
    let mut source=map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let state=xkb::State::new(&map);
    let mut replacements=String::new();
    for code in map.min_keycode().raw()..=map.max_keycode().raw() {
        let key=xkb::Keycode::new(code);
        if state.key_get_one_sym(key).raw()==0xfe03 {
            replacements.push_str(&format!("\n replace key <{}> {{ [ ISO_Level3_Shift ], actions=[ SetMods(modifiers=Control) ] }};",map.key_get_name(key).unwrap()));
        }
    }
    append_section(&mut source,"xkb_symbols", &replacements).unwrap();
    let poison=compile(source).unwrap();
    assert!(layout_plan::text(&poison,0,"@").unwrap().is_none());
}

#[derive(Default)]
struct PlannedWire {
    held:Vec<u32>, events:Vec<(u32,bool)>, guards:usize,
    fail_guard:usize, panic_guard:usize, acknowledged:usize, facts:KeyboardFailureFacts,
}
impl StrokeWire for PlannedWire {
    fn completed(&mut self) {self.facts.completed_units+=1;self.facts.current_unit_uncertain=false;}
    fn guard(&mut self,check:&mut dyn FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
        check()?;
        self.guards+=1;
        if self.guards==self.panic_guard {panic!("injected transport panic");}
        ensure!(self.guards!=self.fail_guard,"layout/group changed");
        self.acknowledged=self.events.len();
        Ok(())
    }
    fn stroke(&mut self,code:u32,down:bool,_:&xkb::State)->anyhow::Result<()> {
        if down {self.held.push(code);self.facts.current_unit_uncertain=true;} else {assert_eq!(self.held.pop(),Some(code));}
        self.events.push((code,down)); Ok(())
    }
}
#[test] fn original_dispatch_is_per_scalar_chords_with_release_ack() {
    let map=original_layout("de");
    let plan=layout_plan::text(&map,0,"aA@b").unwrap().unwrap();
    let mut wire=PlannedWire::default();
    let mut completed=Vec::new();
    dispatch_plan(&mut wire,&map,&plan,&mut ||Ok(()),&mut |n|completed.push(n)).unwrap();
    assert_eq!(completed,vec![1,1,1,1]);
    assert!(wire.held.is_empty());
    assert_eq!(wire.acknowledged,wire.events.len());
    let expected=plan.units.iter().flat_map(|unit|unit.iter().map(|c|(*c,true)).chain(unit.iter().rev().map(|c|(*c,false)))).collect::<Vec<_>>();
    assert_eq!(wire.events,expected);
}
#[test] fn original_dispatch_stops_on_changed_snapshot_without_retrying_prefix() {
    let map=original_layout("us");
    let plan=layout_plan::text(&map,0,"ab").unwrap().unwrap();
    let mut wire=PlannedWire {fail_guard:4,..Default::default()};
    let mut completed=Vec::new();
    assert!(dispatch_plan(&mut wire,&map,&plan,&mut ||Ok(()),&mut |n|completed.push(n)).is_err());
    assert_eq!(completed,vec![1]);
    assert_eq!(wire.facts,KeyboardFailureFacts {completed_units:1,current_unit_uncertain:false,cleanup_failed:false});
    assert_eq!(wire.events.len(),2);
    assert!(wire.held.is_empty());
}
#[test] fn original_dispatch_cancel_or_panic_before_release_never_claims_completion() {
    let map=original_layout("us");
    let plan=layout_plan::text(&map,0,"Ab").unwrap().unwrap();
    for panic in [false,true] {
        let mut wire=PlannedWire {panic_guard:if panic {3} else {0},..Default::default()};
        let mut checks=0;
        let mut completed=Vec::new();
        let error=catch_native(||dispatch_plan(&mut wire,&map,&plan,&mut ||{checks+=1;ensure!(panic || checks!=3,"cancelled");Ok(())},&mut |n|completed.push(n)));
        assert!(error.is_err());
        assert!(completed.is_empty());
        assert!(wire.facts.current_unit_uncertain);
        assert_eq!(wire.facts.completed_units,0);
        assert_eq!(wire.held.len(),2,"session cleanup must release both owned keys");
        assert_eq!(wire.events.len(),2,"no replay or later scalar");
    }
}

#[test] fn original_failure_facts_survive_context_without_string_matching() {
    let facts=KeyboardFailureFacts {completed_units:2,current_unit_uncertain:true,cleanup_failed:true};
    let error=keyboard_error(anyhow::anyhow!("arbitrary transport error"),facts).context("outer facade context");
    assert_eq!(failure_facts(&error),Some(facts));
    assert_eq!(failure_facts(&anyhow::anyhow!("cleanup failed")),None);
}
#[test] fn original_guard_before_first_key_has_no_native_effects() {
    let map=original_layout("us");
    let plan=layout_plan::text(&map,0,"Ab").unwrap().unwrap();
    let mut wire=PlannedWire {fail_guard:1,..Default::default()};
    assert!(dispatch_plan(&mut wire,&map,&plan,&mut ||Ok(()),&mut |_|panic!("must not report progress")).is_err());
    assert!(wire.events.is_empty());
    assert_eq!(wire.facts,KeyboardFailureFacts::default());
}
#[test] fn original_progress_panic_after_ack_does_not_make_unit_uncertain() {
    let map=original_layout("us");
    let plan=layout_plan::text(&map,0,"Ab").unwrap().unwrap();
    let mut wire=PlannedWire::default();
    assert!(catch_native(||dispatch_plan(&mut wire,&map,&plan,&mut ||Ok(()),&mut |_|panic!("progress panic"))).is_err());
    assert!(wire.held.is_empty());
    assert_eq!(wire.facts,KeyboardFailureFacts {completed_units:1,current_unit_uncertain:false,cleanup_failed:false});
}

#[test] fn original_all_ascii_uses_real_printing_positions_not_consumer_aliases() {
    let text=(0x20..0x7f).map(|code|char::from_u32(code).unwrap()).collect::<String>();
    assert_original_text("us",0,&text);
    let map=original_layout("us");
    let plan=layout_plan::text(&map,0,"#*").unwrap().unwrap();
    assert_eq!(*plan.units[0].last().unwrap(),12,"# must be Shift+3, not XF86NumericPound");
    assert_eq!(*plan.units[1].last().unwrap(),17,"* must be Shift+8, not XF86NumericStar");
    assert!(plan.units.iter().all(|unit|unit.len()==2));
    for layout in ["us","de","fr"] {
        let map=original_layout(layout);
        for ch in text.chars() {
            if let Some(plan)=layout_plan::text(&map,0,&ch.to_string()).unwrap() {
                assert!(plan.units[0].iter().all(|code|*code<=135),"{layout} {ch:?}: {:?}",plan.units);
            }
        }
    }
}
#[test] fn original_consumer_alias_on_printing_position_is_not_direct_text() {
    let map=original_layout("us");
    let mut source=map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    append_section(&mut source,"xkb_symbols","\n replace key <AC01> { [ XF86NumericPound ] };").unwrap();
    let map=compile(source).unwrap();
    let plan=layout_plan::text(&map,0,"#").unwrap().unwrap();
    assert_eq!(*plan.units[0].last().unwrap(),12);
    assert_eq!(plan.units[0].len(),2);
}

// Native finalization tests use a private socket pair, never the host compositor.
fn disconnected_session(with_device:bool)->KeyboardSession {
    let (client,peer)=std::os::unix::net::UnixStream::pair().unwrap();
    let conn=Connection::from_socket(client).unwrap();
    let queue=conn.new_event_queue::<State>();
    let original=original_layout("us");
    let source=original.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let keyboard=if with_device {
        let registry=conn.display().get_registry(&queue.handle(),());
        let seat=registry.bind::<wl_seat::WlSeat,_,_>(1,7,&queue.handle(),());
        let manager=registry.bind::<manager::ZwpVirtualKeyboardManagerV1,_,_>(2,1,&queue.handle(),());
        Some(manager.create_virtual_keyboard(&seat,&queue.handle(),()))
    } else {None};
    drop(peer);
    KeyboardSession {conn,queue,state:State::default(),original,source,group:0,keyboard,file:None,held:Vec::new(),start:Instant::now(),failed:false,finished:false,pending_neutral:false,facts:KeyboardFailureFacts::default()}
}
#[test] fn original_finish_without_device_is_idempotent_and_closes_session() {
    let mut session=disconnected_session(false);
    session.facts.completed_units=2;
    session.finish().unwrap();
    session.finish().unwrap();
    assert!(session.finished);
    assert!(!session.failed);
    assert_eq!(session.facts.completed_units,2);
    assert!(session.plan_text("a").is_err());
}
#[test] fn original_finish_reports_native_cleanup_failure_without_inventing_pending_text() {
    let mut session=disconnected_session(true);
    session.facts.completed_units=3;
    let start=Instant::now();
    let error=session.finish().unwrap_err();
    assert!(start.elapsed()<Duration::from_secs(2));
    let expected=KeyboardFailureFacts {completed_units:3,current_unit_uncertain:false,cleanup_failed:true};
    assert_eq!(failure_facts(&error),Some(expected));
    assert!(session.failed && session.finished);
    assert!(session.keyboard.is_none());
    assert_eq!(failure_facts(&session.finish().unwrap_err()),Some(expected));
}

#[test] fn original_identical_fd_notifications_reuse_compilation_but_remain_fresh() {
    let source=original_layout("us").get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let file=keymap_file(&source).unwrap();
    let mut state=State::default();
    let start=Instant::now();
    // Four edges per uppercase scalar. Each notification still reads the FD.
    for _ in 0..2048 {
        state.record_map_source(super::super::shortcuts::read_map_source(&file,(source.len()+1) as u32));
        assert!(state.error.is_none());
        assert_eq!(state.canonical_map.as_deref(),Some(source.as_str()));
    }
    assert_eq!(state.map_revision,2048);
    assert_eq!(state.compiled_maps,1);
    println!("2048 fresh original-map FD observations: {:?}, {} compilation",start.elapsed(),state.compiled_maps);
    state.group=Some(1);state.group_revision=19;
    state.record_map_source(Ok(source));
    assert_eq!(state.map_revision,2049);
    assert_eq!(state.group,Some(1));
    assert_eq!(state.group_revision,19,"map cache must not manufacture group freshness");
}
#[test] fn original_map_cache_recompiles_changes_and_invalidates_errors() {
    let us=original_layout("us").get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let de=original_layout("de").get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let mut state=State::default();
    state.record_map_source(Ok(us.clone()));
    state.record_map_source(Ok(de.clone()));
    assert_eq!(state.compiled_maps,2);
    assert_eq!(state.canonical_map.as_deref(),Some(de.as_str()));
    state.record_map_source(Err(anyhow::anyhow!("Unreadable/truncated compositor FD")));
    assert_eq!(state.map_revision,3);
    assert!(state.map.is_none() && state.canonical_map.is_none() && state.raw_map.is_none());
    assert!(state.error.is_some());
    state.record_map_source(Ok(de));
    assert_eq!(state.compiled_maps,3,"error cannot leave a cache entry that bypasses revalidation");
    assert!(state.error.is_none());
    state.record_map_source(Ok("invalid xkb source".into()));
    assert!(state.map.is_none() && state.canonical_map.is_none() && state.raw_map.is_none());
    assert!(state.error.is_some());
    state.record_map_source(Ok(us));
    assert!(state.error.is_none());
    state.record_map_source(Ok(compiled_keymap("abc").unwrap().0));
    assert!(state.error.as_ref().unwrap().contains("temporary Neoism keymap"));
    assert!(state.map.is_none() && state.canonical_map.is_none());
}
#[test] fn original_enumerated_candidates_preserve_deterministic_chord_cost() {
    let ascii=(0x20..0x7f).map(|c|char::from_u32(c).unwrap()).collect::<String>();
    let map=original_layout("us");
    let full=layout_plan::text(&map,0,&ascii).unwrap().unwrap();
    for (ch,unit) in ascii.chars().zip(&full.units) {
        let single=layout_plan::text(&map,0,&ch.to_string()).unwrap().unwrap();
        assert_eq!(&single.units[0],unit);
    }
    let repeated=layout_plan::text(&map,0,&"A".repeat(512)).unwrap().unwrap();
    assert_eq!(repeated.unit_count(),512);
    assert!(repeated.units.iter().all(|unit|unit==&vec![50,38]));
    assert_eq!(layout_plan::text(&map,0,"#").unwrap().unwrap().units,vec![vec![50,12]]);
}

// These tests speak only wl_display.sync over a private socketpair. No registry,
// seat, virtual keyboard, environment socket, or desktop input is involved.
#[test]
fn pending_sync_callback_has_zero_subsequent_blocking_polls() {
    use std::io::{Read,Write};
    let (client,mut server)=std::os::unix::net::UnixStream::pair().unwrap();
    server.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let conn=Connection::from_socket(client).unwrap();
    let mut queue=conn.new_event_queue::<State>();
    let mut state=State::default();
    let start=Instant::now();
    let mut requests=0;
    let mut callbacks=0;
    BLOCKING_POLLS.with(|count|count.set(0));
    for _ in 0..64 {
        let done=Arc::new(AtomicBool::new(false));
        conn.display().sync(&queue.handle(),done.clone());
        conn.flush().unwrap();
        let mut request=[0u8;12];
        server.read_exact(&mut request).unwrap();
        assert_eq!(u32::from_ne_bytes(request[0..4].try_into().unwrap()),1);
        assert_eq!(u32::from_ne_bytes(request[4..8].try_into().unwrap()),12<<16);
        requests+=1;
        let mut event=[0u8;12];
        event[0..4].copy_from_slice(&request[8..12]);
        event[4..8].copy_from_slice(&(12u32<<16).to_ne_bytes());
        server.write_all(&event).unwrap();
        queue.prepare_read().unwrap().read().unwrap();
        assert!(!done.load(Ordering::Relaxed),"callback must be pending, not dispatched");
        pump_until(&conn,&mut queue,&mut state,Instant::now()+Duration::from_millis(500),||done.load(Ordering::Relaxed)).unwrap();
        assert!(done.load(Ordering::Relaxed));
        callbacks+=1;
    }
    let polls=BLOCKING_POLLS.with(|count|count.get());
    assert_eq!(polls,0,"dispatching completion must not enter a blocking poll");
    assert_eq!((requests,callbacks),(64,64));
    eprintln!("private keyboard sync microbench: requests={requests} callbacks={callbacks} subsequent_blocking_polls={polls} elapsed_us={}",start.elapsed().as_micros());
}

#[test]
fn cancellation_during_read_only_sync_wait_uses_only_cheap_check() {
    let metrics=super::super::latency::Scope::start(Duration::ZERO);
    let (client,_server)=std::os::unix::net::UnixStream::pair().unwrap();
    let conn=Connection::from_socket(client).unwrap();
    let mut queue=conn.new_event_queue::<State>();
    let mut waits=0;
    let mut full=0;
    let mut wait=|| {super::super::latency::count("test_wait_checks",1);waits+=1;ensure!(waits<2,"cancelled while waiting");Ok(())};
    let mut authoritative=|| {full+=1;Ok(())};
    let checks=Checks {wait:&mut wait,full:&mut authoritative};
    BLOCKING_POLLS.with(|count|count.set(0));
    let error=sync(&conn,&mut queue,&mut State::default(),checks.wait).unwrap_err();
    assert_eq!(error.to_string(),"cancelled while waiting");
    assert_eq!(waits,2);
    assert_eq!(full,0);
    assert_eq!(BLOCKING_POLLS.with(|count|count.get()),1);
    let metrics=metrics.finish();
    assert_eq!(metrics.stages["keyboard_sync"].0,1);
    assert_eq!(metrics.stages["keyboard_poll"].0,1);
    assert_eq!(metrics.counters["test_wait_checks"],2);
    assert_eq!(metrics.counters.get("keyboard_edges"),None);
}

#[test]
fn authoritative_focus_loss_before_edge_does_not_touch_device() {
    let (client,_server)=std::os::unix::net::UnixStream::pair().unwrap();
    let conn=Connection::from_socket(client).unwrap();
    let queue=conn.new_event_queue::<State>();
    let original=original_layout("us");
    let source=original.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let mut session=KeyboardSession {conn,queue,state:State::default(),original:original.clone(),source,group:0,
        keyboard:None,file:None,held:Vec::new(),start:Instant::now(),failed:false,finished:false,pending_neutral:false,facts:KeyboardFailureFacts::default()};
    let mut full_calls=0;
    let mut wait_calls=0;
    for down in [true,false] {
        let mut cheap=|| {wait_calls+=1;Ok(())};
        let mut full=|| {full_calls+=1;anyhow::bail!("foreground changed")};
        let mut checks=Checks {wait:&mut cheap,full:&mut full};
        let mut wire=CheckedStroke {session:&mut session,checks:&mut checks};
        // An absent device deliberately makes any accidentally emitted request
        // panic. Both press and release must fail at the authoritative gate.
        let error=wire.stroke(38,down,&xkb::State::new(&original)).unwrap_err();
        assert_eq!(error.to_string(),"foreground changed");
    }
    assert_eq!(full_calls,2);
    assert_eq!(wait_calls,0);
    assert!(session.held.is_empty());
    assert_eq!(session.facts,KeyboardFailureFacts::default());
    session.finish().unwrap();
    assert!(session.finished && session.keyboard.is_none() && session.file.is_none());
}
