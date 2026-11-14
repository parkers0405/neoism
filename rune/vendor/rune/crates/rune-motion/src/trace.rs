//! motion-lint JSONL traces, written from what was actually presented (post-snap, post-mask), with
//! exact "this was animated on purpose" intent bits. Same format apps write
//! and `motion-lint analyze` reads.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use crate::props::{NodeId, PropId, prop};
use crate::scene::Presented;
use crate::store::{FxMap, MotionEvent};

/// Delta-encoded motion-lint scalar channels. Values hold until sampled again or removed.
#[derive(Default)]
pub struct TraceChannels {
    ids: FxMap<String, usize>,
    channels: Vec<Channel>,
}

struct Channel {
    name: String,
    kind: String,
    value: Option<f64>,
    previous: Option<f64>,
    described: bool,
    owner: Option<(u32, &'static str)>,
}

impl TraceChannels {
    /// `kind` is motion-lint's `position`, `level`, or `phase`. Non-finite samples are ignored.
    pub fn sample(&mut self, name: &str, kind: &str, value: f32) {
        if !value.is_finite() { return; }
        assert!(matches!(kind, "position" | "level" | "phase"), "unknown trace channel kind");
        let i = if let Some(&i) = self.ids.get(name) { i } else {
            let i = self.channels.len();
            self.ids.insert(name.to_owned(), i);
            self.channels.push(Channel { name: name.to_owned(), kind: kind.to_owned(), value: None, previous: None, described: false, owner: None });
            i
        };
        let channel = &mut self.channels[i];
        if channel.kind != kind { channel.kind = kind.to_owned(); channel.described = false; }
        channel.value = Some((f64::from(value) * 1e4).round() / 1e4);
    }

    pub fn remove(&mut self, name: &str) {
        if let Some(&i) = self.ids.get(name) { self.channels[i].value = None; }
    }

    /// Associates an effect channel with its element for visibility gating.
    pub fn owner(&mut self, name: &str, owner: u32, unit: &'static str) {
        if let Some(&i) = self.ids.get(name) {
            let channel = &mut self.channels[i];
            if channel.owner != Some((owner, unit)) { channel.owner = Some((owner, unit)); channel.described = false; }
        }
    }

    /// Appends optional `cm`, `c`, and `cg` fields to an open JSON object.
    pub fn write_json(&mut self, line: &mut String) {
        let mut metas = String::new();
        let mut values = String::new();
        let mut gone = String::new();
        for (i, channel) in self.channels.iter_mut().enumerate() {
            let id = i + 1;
            if let Some(value) = channel.value {
                if !channel.described {
                    if !metas.is_empty() { metas.push(','); }
                    let _ = write!(metas, r#"{{"id":{id},"name":{},"kind":{}"#, json_str(&channel.name), json_str(&channel.kind));
                    if let Some((owner, unit)) = channel.owner { let _ = write!(metas, r#", "owner":{owner},"unit":{}"#, json_str(unit)); }
                    metas.push('}');
                    channel.described = true;
                }
                if channel.previous != Some(value) {
                    if !values.is_empty() { values.push(','); }
                    let _ = write!(values, "[{id},{value}]");
                }
            } else if channel.previous.is_some() {
                if !gone.is_empty() { gone.push(','); }
                let _ = write!(gone, "{id}");
            }
            channel.previous = channel.value;
        }
        for (key, values) in [("cm", metas), ("c", values), ("cg", gone)] {
            if !values.is_empty() { let _ = write!(line, r#",{key:?}:[{values}]"#); }
        }
    }
}

/// motion-lint element flag bits (motion-lint's `Flags`).
pub mod flags {
    pub const ANIMATED: u32 = 1;
    pub const ANCESTOR_ANIMATED: u32 = 2;
    pub const SCROLLING: u32 = 4;
    pub const CLIPPED: u32 = 8;
    pub const FILTERED: u32 = 16;
    /// Runs a loop of its own (`El::looping`: a spinner, a sheen, a busy ring's dots), whose
    /// wraps and turns are its design, however briefly it shows.
    pub const LOOPING: u32 = 32;
    /// Draws nothing and holds nothing: a hit or hover zone, a spacer.
    pub const BLANK: u32 = 64;
}

/// motion-lint intent classes (motion-lint's `Intent`): in flags at `<< 8` (own) and `<< 16`
/// (ancestor), and in `ai` intent records.
pub struct Intent;

impl Intent {
    pub const POSITION: u32 = 1;
    pub const SIZE: u32 = 2;
    pub const OPACITY: u32 = 4;
    pub const SHAPE: u32 = 8;
    pub const TRANSITION: u32 = 16;
    pub const REVEAL: u32 = 32;
    pub const CONTENTS: u32 = 64;

    /// The intent class a built-in property's animation declares.
    pub fn for_prop(p: PropId) -> u32 {
        match p {
            x if x == prop::FRAME.id || x == prop::TRAVEL.id => Self::POSITION | Self::SIZE,
            x if x == prop::OFFSET.id || x == prop::ROTATION.id || x == prop::SCROLL.id => Self::POSITION,
            x if x == prop::OPACITY.id => Self::OPACITY,
            x if x == prop::SCALE.id => Self::SIZE | Self::TRANSITION,
            x if x == prop::BLUR.id => Self::TRANSITION,
            x if x == prop::RADIUS.id => Self::SHAPE,
            x if x == prop::FILL.id => Self::CONTENTS,
            x if x == prop::MASK.id => Self::REVEAL,
            x if x == prop::PRESENCE.id => Self::TRANSITION | Self::OPACITY,
            _ => Self::CONTENTS,
        }
    }
}

/// The trace's header line.
#[derive(Clone, Debug)]
pub struct TraceHeader {
    pub launch: String,
    pub width: f32,
    pub height: f32,
    pub hz: f32,
    pub scale: f32,
}

/// A `Write` that keeps everything in shared memory, for tests (`trace.contents()`).
#[derive(Clone, Default)]
pub struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes one JSONL line per frame. Only elements whose presented state changed are written;
/// unmounted ones go in `g`.
pub struct MotionTrace {
    out: Box<dyn Write + Send>,
    ids: FxMap<u64, u32>,
    states: FxMap<u32, [f32; 12]>,
    last_t: Option<f64>,
    hz: f64,
    time_scale: f32,
    line: String,
    names: FxMap<u64, &'static str>,
    described: FxMap<u64, (Option<&'static str>, Option<(&'static str, &'static str)>)>,
    namespaces: FxMap<u64, (&'static str, &'static str)>,
    class: String,
    host: String,
    channels: TraceChannels,
    events: Vec<String>,
}

impl MotionTrace {
    pub fn new(out: impl Write + Send + 'static, header: &TraceHeader) -> io::Result<MotionTrace> {
        let mut out: Box<dyn Write + Send> = Box::new(io::BufWriter::new(out));
        writeln!(
            out,
            r#"{{"type":"header","launch":{},"width":{},"height":{},"hz":{},"scale":{},"platform":"rune","units":"pt","clock":"virtual"}}"#,
            json_str(&header.launch),
            header.width,
            header.height,
            header.hz,
            header.scale
        )?;
        Ok(MotionTrace {
            out,
            ids: FxMap::default(),
            states: FxMap::default(),
            last_t: None,
            hz: header.hz.max(1.0) as f64,
            time_scale: 1.0,
            line: String::new(),
            names: FxMap::default(),
            described: FxMap::default(),
            namespaces: FxMap::default(),
            class: "rune".into(),
            host: "scene".into(),
            channels: TraceChannels::default(),
            events: Vec::new(),
        })
    }

    /// Opens `$RUNE_MOTION_TRACE` if it is set (one env read).
    pub fn from_env(header: &TraceHeader) -> Option<MotionTrace> {
        let path = std::env::var_os("RUNE_MOTION_TRACE")?;
        let file = std::fs::File::create(path).ok()?;
        MotionTrace::new(file, header).ok()
    }

    /// An in-memory trace and a handle to read it back.
    pub fn in_memory(header: &TraceHeader) -> (MotionTrace, SharedBuffer) {
        let buf = SharedBuffer::default();
        (MotionTrace::new(buf.clone(), header).expect("in-memory write"), buf)
    }

    /// The clock's time scale, written as `sc` so motion-lint counts the app's own frames.
    pub fn set_time_scale(&mut self, sc: f32) {
        self.time_scale = sc;
    }

    /// Set before the first frame when comparing against another host (for example `pill`).
    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    /// The comparison class. motion-lint pairs named elements by both `nm` and `cls`.
    /// Use the reference writer's class (e.g. `gpui`) for cross-renderer comparisons.
    pub fn class(mut self, class: impl Into<String>) -> Self {
        self.class = class.into();
        self
    }

    /// Overrides this node's class and host; None restores the writer's defaults.
    pub fn namespace(&mut self, node: NodeId, namespace: Option<(&'static str, &'static str)>) -> bool {
        if let Some(namespace) = namespace { self.namespaces.insert(node.0, namespace) != Some(namespace) }
        else { self.namespaces.remove(&node.0).is_some() }
    }

    /// Stable display name, independent of reconciliation identity. None restores the NodeId.
    pub fn name(&mut self, node: NodeId, name: Option<&'static str>) -> bool {
        if let Some(name) = name { self.names.insert(node.0, name) != Some(name) }
        else { self.names.remove(&node.0).is_some() }
    }

    pub fn channel(&mut self, name: &str, kind: &str, value: f32) {
        self.channels.sample(name, kind, value);
    }

    pub fn remove_channel(&mut self, name: &str) { self.channels.remove(name); }

    /// Recorded once, on the next frame. `format_args!` avoids eager string allocation.
    pub fn event(&mut self, event: impl std::fmt::Display) { self.events.push(event.to_string()); }

    fn id(&mut self, node: NodeId) -> u32 {
        let next = self.ids.len() as u32 + 1;
        *self.ids.entry(node.0).or_insert(next)
    }

    /// Writes one frame. `t` is animation time in seconds.
    pub fn frame(
        &mut self,
        t: f64,
        presented: &[Presented],
        gone: &[NodeId],
        events: &[MotionEvent],
        intents: &[(NodeId, u32, f32)],
    ) {
        let t_ms = t * 1000.0;
        let n = self.last_t.map_or(1, |last| (((t_ms - last) / (1000.0 / self.hz)).round().max(1.0)) as u64);
        self.last_t = Some(t_ms);
        let mut line = std::mem::take(&mut self.line);
        line.clear();
        let _ = write!(line, r#"{{"t":{},"n":{},"sc":{}"#, r3(t_ms), n, self.time_scale);
        if !events.is_empty() || !self.events.is_empty() {
            line.push_str(r#","ev":["#);
            for (k, e) in events.iter().enumerate() {
                if k > 0 {
                    line.push(',');
                }
                let s = match *e {
                    MotionEvent::Settled { node, prop, .. } => format!("settled {}:{}", node.0, prop.0),
                    MotionEvent::Completed { token, finished, .. } => {
                        format!("completed #{token}{}", if finished { "" } else { " (replaced)" })
                    }
                };
                line.push_str(&json_str(&s));
            }
            for (k, event) in self.events.drain(..).enumerate() {
                if k > 0 || !events.is_empty() { line.push(','); }
                line.push_str(&json_str(&event));
            }
            line.push(']');
        }
        let mut metas = String::new();
        let mut elems = String::new();
        for p in presented {
            let id = self.id(p.node);
            let name = self.names.get(&p.node.0).copied();
            let namespace = self.namespaces.get(&p.node.0).copied();
            if self.described.insert(p.node.0, (name, namespace)) != Some((name, namespace)) {
                if !metas.is_empty() {
                    metas.push(',');
                }
                let name = name.map_or_else(|| p.node.0.to_string(), str::to_owned);
                let (class, host) = namespace.unwrap_or((&self.class, &self.host));
                let _ = write!(metas, r#"{{"id":{id},"cls":{},"nm":{},"host":{}"#, json_str(class), json_str(&name), json_str(host));
                if let Some(parent) = p.parent {
                    let pv = self.id(parent);
                    let _ = write!(metas, r#","pv":{pv}"#);
                }
                metas.push('}');
            }
            let r = p.rect;
            let ins = p.mask.inset.0;
            let mut s = [r.x, r.y, r.w, r.h, p.opacity, p.radius.0[0], p.flags as f32, f32::NAN, 0.0, 0.0, 0.0, 0.0];
            if ins.iter().any(|&v| v > 0.25) {
                s[7] = r.x + ins[0];
                s[8] = r.y + ins[1];
                s[9] = (r.w - ins[0] - ins[2]).max(0.0);
                s[10] = (r.h - ins[1] - ins[3]).max(0.0);
            }
            let rounded = s.map(|v| if v.is_nan() { v } else { (v * 100.0).round() / 100.0 });
            if self.states.get(&id).is_some_and(|old| same(old, &rounded)) {
                continue;
            }
            self.states.insert(id, rounded);
            if !elems.is_empty() {
                elems.push(',');
            }
            let _ = write!(
                elems,
                "[{id},{},{},{},{},{},{},{}",
                r2(r.x),
                r2(r.y),
                r2(r.w),
                r2(r.h),
                r3(p.opacity as f64),
                r2(p.radius.0[0]),
                p.flags
            );
            if !s[7].is_nan() {
                let _ = write!(elems, ",{},{},{},{}", r2(s[7]), r2(s[8]), r2(s[9]), r2(s[10]));
            }
            elems.push(']');
        }
        if !intents.is_empty() {
            line.push_str(r#","ai":["#);
            for (k, &(node, bits, secs)) in intents.iter().enumerate() {
                let id = self.id(node);
                let rect = presented.iter().find(|p| p.node == node).map(|p| p.rect).unwrap_or_default();
                if k > 0 {
                    line.push(',');
                }
                let _ = write!(
                    line,
                    "[{id},{},{},{},{},{bits},{}]",
                    r2(rect.x),
                    r2(rect.y),
                    r2(rect.w),
                    r2(rect.h),
                    (secs * 1000.0).round()
                );
            }
            line.push(']');
        }
        if !metas.is_empty() {
            let _ = write!(line, r#","m":[{metas}]"#);
        }
        if !elems.is_empty() {
            let _ = write!(line, r#","e":[{elems}]"#);
        }
        if !gone.is_empty() {
            line.push_str(r#","g":["#);
            for (k, node) in gone.iter().enumerate() {
                let id = self.id(*node);
                self.states.remove(&id);
                if k > 0 {
                    line.push(',');
                }
                let _ = write!(line, "{id}");
            }
            line.push(']');
        }
        self.channels.write_json(&mut line);
        line.push('}');
        let _ = writeln!(self.out, "{line}");
        self.line = line;
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

impl Drop for MotionTrace {
    fn drop(&mut self) {
        let _ = self.out.flush();
    }
}

fn same(a: &[f32; 12], b: &[f32; 12]) -> bool {
    a.iter().zip(b).all(|(x, y)| x == y || (x.is_nan() && y.is_nan()))
}

fn r2(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
