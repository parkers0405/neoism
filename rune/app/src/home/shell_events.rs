//! Shell metadata in PTY output: OSC 133 prompt/input/start/end marks, OSC 7 directories and
//! screen clears. Observes bytes without consuming them; Ghostty still parses every one.

#[derive(Debug, PartialEq)]
pub enum ShellEvent {
    Prompt,
    Input,
    Start,
    End(Option<i32>),
    Directory(String),
    Clear,
}

/// Observes shell metadata without consuming bytes; Ghostty still parses every one
/// (port of `gpuix/src/terminal/shell-events.ts`).
#[derive(Default)]
pub struct ShellEvents {
    state: ScanState,
    payload: Vec<u8>,
    overflow: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum ScanState {
    #[default]
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
    Str,
    StrEscape,
}

impl ShellEvents {
    pub fn reset(&mut self) {
        self.state = ScanState::Text;
        self.payload.clear();
        self.overflow = false;
    }

    /// Calls `emit(end, event)` with the byte index just past each event's sequence.
    pub fn feed(&mut self, bytes: &[u8], mut emit: impl FnMut(usize, ShellEvent)) {
        use ScanState::*;
        for (i, &byte) in bytes.iter().enumerate() {
            match self.state {
                Text => if byte == 27 { self.state = Escape },
                Escape => {
                    if byte == b'c' { emit(i + 1, ShellEvent::Clear); }
                    self.state = match byte { b'[' => Csi, b']' => Osc, b'P' | b'X' | b'^' | b'_' => Str, 27 => Escape, _ => Text };
                    self.payload.clear();
                    self.overflow = false;
                }
                Csi => {
                    if byte == 24 || byte == 26 { self.reset(); }
                    else if byte == 27 { self.reset(); self.state = Escape; }
                    else if (64..=126).contains(&byte) {
                        // Only ED 2/3 (erase screen / scrollback) matters to blocks.
                        if !self.overflow && byte == b'J' && (self.payload == b"2" || self.payload == b"3") { emit(i + 1, ShellEvent::Clear); }
                        self.reset();
                    } else if self.payload.len() < 32 { self.payload.push(byte); } else { self.overflow = true; }
                }
                Str | StrEscape => {
                    self.state = if self.state == StrEscape && byte == b'\\' { Text } else if byte == 27 { StrEscape } else { Str };
                }
                Osc | OscEscape => {
                    if byte == 24 || byte == 26 { self.reset(); continue; }
                    if byte == 7 || (self.state == OscEscape && byte == b'\\') {
                        if !self.overflow { if let Some(event) = osc_event(&self.payload) { emit(i + 1, event); } }
                        self.reset();
                        continue;
                    }
                    if self.state == OscEscape {
                        self.reset();
                        self.state = if byte == 27 { Escape } else { Text };
                        continue;
                    }
                    if byte == 27 { self.state = OscEscape; continue; }
                    if self.payload.len() < 8192 { self.payload.push(byte); } else { self.overflow = true; }
                }
            }
        }
    }
}

fn osc_event(payload: &[u8]) -> Option<ShellEvent> {
    let value = std::str::from_utf8(payload).ok()?;
    if let Some(rest) = value.strip_prefix("133;").or_else(|| value.strip_prefix("633;")) {
        let mut parts = rest.split(';');
        let kind = parts.next()?;
        let status = parts.next().and_then(|s| s.parse().ok());
        return match kind {
            "A" => Some(ShellEvent::Prompt),
            "B" => Some(ShellEvent::Input),
            "C" => Some(ShellEvent::Start),
            "D" => Some(ShellEvent::End(status)),
            _ => None,
        };
    }
    let url = value.strip_prefix("7;file://")?;
    let path = &url[url.find('/')?..];
    Some(ShellEvent::Directory(percent_decode(path)))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { out.push(v); i += 3; continue; }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_osc_133_and_7() {
        let mut events = ShellEvents::default();
        let mut seen = Vec::new();
        events.feed(b"\x1b]133;A\x07$ \x1b]133;B\x1b\\ls\r\n\x1b]133;C\x07out\x1b]133;D;2\x07\x1b]7;file://host/tmp/a%20b\x07\x1b[2J", |_, e| seen.push(e));
        assert_eq!(seen, vec![ShellEvent::Prompt, ShellEvent::Input, ShellEvent::Start, ShellEvent::End(Some(2)),
            ShellEvent::Directory("/tmp/a b".into()), ShellEvent::Clear]);
    }
}
