//! Pointer-only Wayland transactions: never bind or create a keyboard.
use anyhow::{Context, ensure};
use std::{os::fd::AsRawFd, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, protocol::{wl_callback, wl_output, wl_pointer, wl_registry, wl_seat}};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1 as xmanager, zxdg_output_v1 as xoutput};
use wayland_protocols_wlr::virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1 as manager, zwlr_virtual_pointer_v1 as pointer};
use super::Display;

/// Original screenshot pixels and extents; do not pre-scale to output pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Point { pub x:u32, pub y:u32, pub width:u32, pub height:u32 }
impl Point {
    fn validate(self)->anyhow::Result<()> {
        ensure!(self.width>0 && self.height>0 && self.x<self.width && self.y<self.height,"Invalid screenshot pointer coordinates"); Ok(())
    }
    fn between(self,end:Self,step:u32)->Self {
        let axis=|a:u32,b:u32|(i64::from(a)+(i64::from(b)-i64::from(a))*i64::from(step)/20) as u32;
        Self {x:axis(self.x,end.x),y:axis(self.y,end.y),..self}
    }
}
pub(super) enum Command { Move(Point), Click(Point,u32), Drag(Point,Point), Scroll {amount:i32,horizontal:bool} }
#[derive(Default)]
struct State {
    outputs:Vec<(wl_output::WlOutput,Display)>, seats:Vec<wl_seat::WlSeat>,
    managers:Vec<manager::ZwlrVirtualPointerManagerV1>, xmanager:Option<xmanager::ZxdgOutputManagerV1>, removed:bool,
}
impl Dispatch<wl_registry::WlRegistry,()> for State {
    fn event(s:&mut Self,r:&wl_registry::WlRegistry,e:wl_registry::Event,_:&(),_:&Connection,q:&QueueHandle<Self>) {
        match e {
            wl_registry::Event::Global {name,interface,version}=>match interface.as_str() {
                "wl_output"=>s.outputs.push((r.bind(name,version.min(4),q,()),Display{id:String::new(),x:0,y:0,width:0,height:0})),
                "wl_seat"=>s.seats.push(r.bind(name,version.min(7),q,())),
                "zwlr_virtual_pointer_manager_v1"=>s.managers.push(r.bind(name,version.min(2),q,())),
                "zxdg_output_manager_v1"=>s.xmanager=Some(r.bind(name,version.min(3),q,())),
                _=>{}
            },
            wl_registry::Event::GlobalRemove {..}=>s.removed=true,
            _=>{}
        }
    }
}
impl Dispatch<xoutput::ZxdgOutputV1,usize> for State {
    fn event(s:&mut Self,_:&xoutput::ZxdgOutputV1,e:xoutput::Event,i:&usize,_:&Connection,_:&QueueHandle<Self>) {
        let d=&mut s.outputs[*i].1;
        match e {
            xoutput::Event::Name {name}=>d.id=name,
            xoutput::Event::LogicalPosition {x,y}=>{d.x=x;d.y=y;},
            xoutput::Event::LogicalSize {width,height}=>{d.width=width.max(0) as u32;d.height=height.max(0) as u32;},
            _=>{}
        }
    }
}
impl Dispatch<wl_callback::WlCallback,Arc<AtomicBool>> for State {
    fn event(_:&mut Self,_:&wl_callback::WlCallback,_:wl_callback::Event,done:&Arc<AtomicBool>,_:&Connection,_:&QueueHandle<Self>) {done.store(true,Ordering::Relaxed);}
}
wayland_client::delegate_noop!(State: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(State: ignore wl_seat::WlSeat);
wayland_client::delegate_noop!(State: ignore manager::ZwlrVirtualPointerManagerV1);
wayland_client::delegate_noop!(State: ignore pointer::ZwlrVirtualPointerV1);
wayland_client::delegate_noop!(State: ignore xmanager::ZxdgOutputManagerV1);

fn sync(conn:&Connection,queue:&mut EventQueue<State>,state:&mut State,check:&mut impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    let done=Arc::new(AtomicBool::new(false));
    conn.display().sync(&queue.handle(),done.clone());
    let deadline=Instant::now()+Duration::from_millis(500);
    while !done.load(Ordering::Relaxed) {
        check()?;
        queue.dispatch_pending(state)?;
        let blocked=match conn.flush() {
            Ok(())=>false,
            Err(wayland_client::backend::WaylandError::Io(e)) if e.kind()==std::io::ErrorKind::WouldBlock=>true,
            Err(e)=>return Err(e.into()),
        };
        ensure!(Instant::now()<deadline,"Wayland pointer acknowledgement timed out; observe before retrying");
        if done.load(Ordering::Relaxed) {break;}
        if let Some(read)=queue.prepare_read() {
            let mut fd=libc::pollfd {fd:read.connection_fd().as_raw_fd(),events:libc::POLLIN|if blocked {libc::POLLOUT} else {0},revents:0};
            // SAFETY: initialized pollfd borrowing a live connection, bounded wait.
            let n=unsafe {libc::poll(&mut fd,1,10)};
            if n<0 {
                let error=std::io::Error::last_os_error();
                if error.kind()!=std::io::ErrorKind::Interrupted {return Err(error.into());}
            } else if n>0 {
                ensure!(fd.revents&(libc::POLLERR|libc::POLLHUP|libc::POLLNVAL)==0,"Wayland pointer connection closed");
                if fd.revents&libc::POLLIN!=0 {read.read()?;}
            }
        }
    }
    check()?; Ok(())
}
fn matching_output(observed:&Display,outputs:&[Display])->anyhow::Result<()> {
    ensure!(outputs.len()==1,"Wayland pointer control requires exactly one output");
    ensure!(!observed.id.is_empty() && observed.width>0 && observed.height>0 && outputs[0]==*observed,"Pointer output name/geometry differs from observation; take a new screenshot"); Ok(())
}
struct Native {
    conn:Connection,queue:EventQueue<State>,state:State,pointer:pointer::ZwlrVirtualPointerV1,
    held:Option<u32>,destroyed:bool,start:Instant,display:Display,
}
impl Native {
    fn time(&self)->u32 {self.start.elapsed().as_millis() as u32}
    fn frame(&mut self,check:&mut impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
        self.pointer.frame();
        sync(&self.conn,&mut self.queue,&mut self.state,check)?;
        ensure!(!self.state.removed && self.state.seats.len()==1 && self.state.managers.len()==1,"Wayland topology changed during pointer action");
        matching_output(&self.display,&self.state.outputs.iter().map(|o|o.1.clone()).collect::<Vec<_>>())
    }
    fn motion(&mut self,p:Point,check:&mut impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
        check()?; p.validate()?;
        self.pointer.motion_absolute(self.time(),p.x,p.y,p.width,p.height);
        self.frame(check)
    }
    fn press(&mut self,button:u32,check:&mut impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
        check()?; self.held=Some(button);
        self.pointer.button(self.time(),button,wl_pointer::ButtonState::Pressed); self.frame(check)
    }
    fn cleanup(&mut self)->anyhow::Result<()> {
        if self.destroyed {return Ok(());}
        if let Some(button)=self.held.take() {self.pointer.button(self.time(),button,wl_pointer::ButtonState::Released);}
        self.pointer.frame(); self.pointer.destroy(); self.destroyed=true;
        sync(&self.conn,&mut self.queue,&mut self.state,&mut ||Ok(()))
    }
}
impl Drop for Native {
    fn drop(&mut self) {let _=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||self.cleanup()));}
}
/// Low-level regression API. Caller owns permission, fresh-frame/target checks,
/// serialization and revocation; `check` is called before every input event.
/// Button codes: Linux BTN_LEFT=0x110, BTN_RIGHT=0x111, BTN_MIDDLE=0x112.
/// Success acknowledges compositor dispatch, NOT application consumption.
pub(super) fn send(display:&Display,command:Command,mut check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    match &command {
        Command::Move(p)=>p.validate()?,
        Command::Click(p,b)=>{p.validate()?;ensure!((0x110..=0x112).contains(b),"Unsupported button");},
        Command::Drag(a,b)=>{a.validate()?;b.validate()?;ensure!(a.width==b.width && a.height==b.height,"Drag extents differ");},
        Command::Scroll {amount,..}=>ensure!((-20..=20).contains(amount),"Scroll outside -20..20"),
    }
    check()?;
    let conn=Connection::connect_to_env().context("Cannot connect native Wayland pointer")?;
    let mut queue=conn.new_event_queue::<State>(); let mut state=State::default();
    conn.display().get_registry(&queue.handle(),());
    sync(&conn,&mut queue,&mut state,&mut check)?;
    ensure!(state.outputs.len()==1 && state.seats.len()==1 && state.managers.len()==1,"Pointer requires one output, one seat and one supported virtual-pointer manager");
    let xm=state.xmanager.as_ref().context("Pointer requires xdg-output name and logical geometry")?;
    ensure!(xm.version()>=2,"xdg-output names unsupported");
    for (i,(output,_)) in state.outputs.iter().enumerate() {xm.get_xdg_output(output,&queue.handle(),i);}
    sync(&conn,&mut queue,&mut state,&mut check)?;
    matching_output(display,&state.outputs.iter().map(|o|o.1.clone()).collect::<Vec<_>>())?;
    ensure!(!state.removed,"Wayland topology changed"); check()?;
    let manager=&state.managers[0];
    let pointer=if manager.version()>=2 {manager.create_virtual_pointer_with_output(Some(&state.seats[0]),Some(&state.outputs[0].0),&queue.handle(),())}
        else {manager.create_virtual_pointer(Some(&state.seats[0]),&queue.handle(),())};
    let mut native=Native {conn,queue,state,pointer,held:None,destroyed:false,start:Instant::now(),display:display.clone()};
    let result=(|| {
        native.frame(&mut check)?;
        match command {
            Command::Move(p)=>native.motion(p,&mut check)?,
            Command::Click(p,b)=>{native.motion(p,&mut check)?;native.press(b,&mut check)?;},
            Command::Drag(a,b)=>{
                native.motion(a,&mut check)?;native.press(0x110,&mut check)?;
                for step in 1..=20 {native.motion(a.between(b,step),&mut check)?;std::thread::sleep(Duration::from_millis(16));}
            },
            Command::Scroll {amount,horizontal}=>{
                check()?;
                let axis=if horizontal {wl_pointer::Axis::HorizontalScroll} else {wl_pointer::Axis::VerticalScroll};
                native.pointer.axis_source(wl_pointer::AxisSource::Wheel);
                if amount!=0 {native.pointer.axis_discrete(native.time(),axis,f64::from(amount)*15.0,amount);}
                else {native.pointer.axis_stop(native.time(),axis);}
                native.frame(&mut check)?;
            }
        }
        Ok(())
    })();
    let cleanup=native.cleanup();
    match (result,cleanup) {
        (Ok(()),Ok(()))=>{check()?;Ok(())},
        (Err(e),Ok(()))=>Err(e),
        (Ok(()),Err(e))=>Err(e.context("Pointer cleanup not acknowledged")),
        (Err(e),Err(c))=>Err(anyhow::anyhow!("{e:#}; pointer cleanup failed: {c:#}")),
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn stalled_transport_and_cancellation_are_bounded_without_desktop_input() {
        let (client,_server)=std::os::unix::net::UnixStream::pair().unwrap();
        let conn=Connection::from_socket(client).unwrap();
        let mut queue=conn.new_event_queue::<State>();let mut state=State::default();
        let start=Instant::now();
        assert!(sync(&conn,&mut queue,&mut state,&mut ||Ok(())).is_err());
        assert!(start.elapsed()<Duration::from_secs(2));
        let start=Instant::now();
        assert!(sync(&conn,&mut queue,&mut state,&mut ||anyhow::bail!("cancelled")).is_err());
        assert!(start.elapsed()<Duration::from_millis(100));
    }
    #[test] fn screenshot_ratios_are_not_rescaled() {
        let a=Point{x:0,y:0,width:1600,height:900};let b=Point{x:1599,y:899,..a};
        assert_eq!(a.between(b,20),b);assert_eq!(a.between(b,10).x,799);
        assert!(Point{x:1600,..a}.validate().is_err());assert!(Point{width:0,..a}.validate().is_err());
        assert_eq!(b.between(a,20),a);
    }
    #[test] fn rejects_ambiguous_or_stale_outputs() {
        let d=Display{id:"DP-1".into(),x:-1920,y:20,width:1920,height:1080};
        assert!(matching_output(&d,&[d.clone()]).is_ok());
        assert!(matching_output(&d,&[]).is_err());assert!(matching_output(&d,&[d.clone(),d.clone()]).is_err());
        for other in [Display{id:"DP-2".into(),..d.clone()},Display{x:0,..d.clone()},Display{width:3840,..d.clone()}] {assert!(matching_output(&d,&[other]).is_err());}
    }
}
