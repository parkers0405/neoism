use super::{App,splash_art};
use rune::{El,Shape,Tx,canvas,col,el};
use std::{f32::consts::TAU,time::Duration};

#[allow(dead_code)]
#[path="../../../rune/app/src/wordmark/animation.rs"]
mod animation;

pub struct Splash {
    animation:animation::WordmarkAnimation,
    clock:f64, last:Option<f64>, over:Option<usize>, hover:[f32;8], reduced:bool,
}
impl Default for Splash {
    fn default()->Self { Self { animation:Default::default(), clock:0., last:None, over:None, hover:[0.;8], reduced:std::env::var("NEOISM_REDUCED_MOTION").as_deref()==Ok("1") } }
}
impl Splash {
    pub fn tick(&mut self,now:f64,visible:bool,tx:&mut Tx)->bool {
        if !visible || self.reduced {self.last=None;self.over=None;self.hover=[0.;8];return false;}
        tx.wake_after(Duration::from_millis(32));
        let dt=self.last.map_or(0.,|v|(now-v).clamp(0.,0.1));
        if self.last.is_some() && dt<1./32. {return false;}
        self.last=Some(now);self.clock+=dt;
        let alpha=1.-(-14.*dt as f32).exp();
        for (i,h) in self.hover.iter_mut().enumerate(){*h+=((self.over==Some(i)) as u8 as f32-*h)*alpha;}
        true
    }
}
fn rect_points([x,y,w,h]:[f32;4])->[[f32;2];4]{[[x,y],[x+w,y],[x+w,y+h],[x,y+h]]}
fn color(hex:u32)->rune::Color {
    let c=|v:u32| {let v=(v&255) as f32/255.;if v<=0.04045{v/12.92}else{((v+0.055)/1.055).powf(2.4)}};
    rune::Color{r:c(hex>>16),g:c(hex>>8),b:c(hex),a:1.}
}
pub fn view(app:&App)->El {
    let s=&app.splash;
    let width=((app.width-if app.sidebar {200.}else{0.})-52.).clamp(120.,720.);
    let w=width/8.;let h=w*193./(1196./6.);let height=h*2.55;
    let t=s.clock as f32;let frame=s.animation.frame(s.clock);
    let mut shape=Shape::new().view_box(width,height);
    let mut draw=|points:[[f32;2];4],tint:u32,a:f32|{shape=std::mem::take(&mut shape).fill(points.to_vec(),color(tint).alpha(a));};
    for (i,mask) in splash_art::MASKS.iter().enumerate(){
        let lift=s.hover[i];let scale=if s.reduced{1.}else{1.+lift*0.18+((t/3.4+i as f32*0.16)*TAU).sin()*0.025};
        let rect=[w*(i as f32+(1.-scale)/2.),h*(0.8+(1.-scale)/2.-lift*0.18),w*scale,h*scale];
        if frame.as_ref().is_none_or(|f|f.show_original()){
            for layer in [[25.74,-20.85,0.30],[12.87,-10.42,0.50],[0.,0.,1.]]{
                for (row,bits) in mask.iter().enumerate(){for col in 0..7 {if bits&(1<<(6-col))!=0 {
                    let corners=rect_points([col as f32*31.25,-250.+row as f32*31.25,31.35,31.35]);
                    draw(corners.map(|p|animation::map_point(p,i,layer,rect)),0xe9e8d4,layer[2]);
                }}}
            }
        }
        if let Some(f)=&frame {f.glyph_quads(i,mask,rect,|q,a|draw(q,0xe9e8d4,a));}
        splash_art::fire(i,t,|row,col,cell,tint|{
            let corners=rect_points(cell);
            if let Some(f)=frame.as_ref().filter(|f|!f.show_original()) {if let Some((q,a))=f.attached_quad(i,row,col,corners,rect){draw(q,tint,a);}}
            else {draw(corners.map(|p|animation::map_point(p,i,[0.,0.,1.],rect)),tint,1.);}
        });
    }
    let mut mark=el().key("neoboard-splash").w(width).h(height).instant_changes().child(canvas(shape).w(width).h(height));
    for i in 0..8 {mark=mark.child(el().key(format!("splash-letter-{i}")).absolute(i as f32*w,h*0.45).w(w).h(h*1.5)
        .on_hover_change(move |a:&mut App,inside,_|{if inside {a.splash.over=Some(i);}else if a.splash.over==Some(i){a.splash.over=None;}})
        .on_press(|a:&mut App|{if !a.splash.reduced{a.splash.animation.replay(a.splash.clock);}}));}
    col().w_full().items_center().child(mark)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn last_letter_responds_to_hover_and_click_and_setup_still_copies() {
        let mut app=App::new();app.splash.reduced=false;
        let mut rt=rune::Runtime::new(app,crate::view::root,(1252.,800.),rune::Env::new(true,1.));
        for _ in 0..5{rt.frame(1./60.);}
        let (x,y)=rt.find("splash-letter-7").unwrap();rt.pointer_move(x,y);
        rt.pointer_down(x,y);rt.pointer_up(x,y);rt.frame(1./60.);
        assert_eq!(rt.state.splash.over,Some(7));
        assert!(rt.state.splash.animation.is_active(rt.state.splash.clock));
        let (x,y)=rt.find("home-setup").unwrap();rt.pointer_move(x,y);rt.pointer_down(x,y);rt.pointer_up(x,y);rt.frame(1./60.);
        let (x,y)=rt.find("copy-setup").unwrap();rt.pointer_move(x,y);rt.pointer_down(x,y);rt.pointer_up(x,y);rt.frame(1./60.);
        assert_eq!(rt.state.home_copied,Some("copy-setup"));
    }
}