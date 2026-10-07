//! Versioned, length-bounded pipe protocol shared with the standalone Servo worker.
//! JSON controls/metadata followed by binary RGBA packets. No GPU handles or host bridge.
use crate::{ArtifactDocument, ArtifactInput, Viewport, MAX_FRAME_BYTES};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_JSON_BYTES: usize = 32 * 1024 * 1024;
const JSON: u32 = 1;
const RGBA: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
pub enum Control {
    Reconcile {
        document: ArtifactDocument,
        generation: u64,
    },
    Resize {
        key: String,
        viewport: Viewport,
        generation: u64,
    },
    Input {
        key: String,
        input: ArtifactInput,
    },
    Destroy {
        key: String,
    },
    Pump,
    Shutdown,
}
impl Control {
    pub fn validate(&self) -> Result<(), crate::Error> {
        let key = match self {
            Self::Reconcile {
                document,
                generation,
            } => {
                if *generation == 0 {
                    return Err(crate::Error::InvalidInput);
                }
                return document.validate();
            }
            Self::Resize {
                key,
                viewport,
                generation,
            } => {
                if *generation == 0 {
                    return Err(crate::Error::InvalidInput);
                }
                viewport.validate()?;
                Some(key)
            }
            Self::Input { key, input } => {
                input.validate()?;
                Some(key)
            }
            Self::Destroy { key } => Some(key),
            Self::Pump | Self::Shutdown => None,
        };
        if key.is_some_and(|key| key.is_empty() || key.len() > 1024) {
            return Err(crate::Error::InvalidKey);
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub enum Output {
    Hello {
        version: u32,
    },
    Frame(FrameMetadata),
    Status {
        animating: bool,
        diagnostics: Vec<String>,
    },
    Fatal(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameMetadata {
    pub key: String,
    pub generation: u64,
    pub revision: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub bytes: usize,
}
impl FrameMetadata {
    pub fn validate(&self) -> io::Result<()> {
        if self.generation == 0 {
            return Err(invalid("Invalid frame generation"));
        }
        if self.key.is_empty() || self.key.len() > 1024 {
            return Err(invalid("Invalid frame key"));
        }
        Viewport {
            width: self.width,
            height: self.height,
            scale: 1.0,
        }
        .validate()
        .map_err(|_| invalid("Invalid frame viewport"))?;
        let stride = (self.width as usize)
            .checked_mul(4)
            .ok_or_else(|| invalid("Frame stride overflow"))?;
        let bytes = stride
            .checked_mul(self.height as usize)
            .ok_or_else(|| invalid("Frame size overflow"))?;
        if self.stride != stride || self.bytes != bytes || bytes > MAX_FRAME_BYTES {
            return Err(invalid("Invalid RGBA frame layout"));
        }
        Ok(())
    }
}
pub fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn read_header(reader: &mut impl Read) -> io::Result<Option<(u32, usize)>> {
    let mut header = [0u8; 8];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let kind = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let length =
        u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    Ok(Some((kind, length)))
}
pub fn encode_json(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let json = serde_json::to_vec(value).map_err(|e| invalid(&e.to_string()))?;
    if json.len() > MAX_JSON_BYTES {
        return Err(invalid("JSON packet exceeds limit"));
    }
    Ok(json)
}
pub fn write_encoded_json(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX_JSON_BYTES {
        return Err(invalid("JSON packet exceeds limit"));
    }
    write_packet(writer, JSON, bytes)
}
pub fn write_json(writer: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    write_encoded_json(writer, &encode_json(value)?)
}
pub fn read_json<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<Option<T>> {
    let Some((kind, length)) = read_header(reader)? else {
        return Ok(None);
    };
    if kind != JSON || length > MAX_JSON_BYTES {
        return Err(invalid("Invalid JSON packet header"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| invalid(&e.to_string()))
}
fn write_packet(writer: &mut impl Write, kind: u32, bytes: &[u8]) -> io::Result<()> {
    let length =
        u32::try_from(bytes.len()).map_err(|_| invalid("Packet size overflow"))?;
    writer.write_all(&kind.to_le_bytes())?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(bytes)
}
pub fn write_frame(
    writer: &mut impl Write,
    metadata: &FrameMetadata,
    bytes: &[u8],
) -> io::Result<()> {
    metadata.validate()?;
    if bytes.len() != metadata.bytes {
        return Err(invalid("Frame body length mismatch"));
    }
    write_json(writer, &Output::Frame(metadata.clone()))?;
    write_packet(writer, RGBA, bytes)
}
pub fn read_frame(
    reader: &mut impl Read,
    metadata: &FrameMetadata,
) -> io::Result<Vec<u8>> {
    metadata.validate()?;
    let Some((kind, length)) = read_header(reader)? else {
        return Err(invalid("Missing RGBA packet"));
    };
    // Check layout and advertised packet length BEFORE allocation.
    if kind != RGBA || length != metadata.bytes {
        return Err(invalid("Invalid RGBA packet header"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    fn meta() -> FrameMetadata {
        FrameMetadata {
            key: "a".into(),
            generation: 1,
            revision: 7,
            sequence: 1,
            width: 1,
            height: 1,
            stride: 4,
            bytes: 4,
        }
    }
    #[test]
    fn reject_bad_control_keys_epochs_and_revisions() -> io::Result<()> {
        assert!(Control::Destroy { key: String::new() }.validate().is_err());
        assert!(Control::Resize {
            key: "a".into(),
            viewport: Viewport {
                width: 1,
                height: 1,
                scale: 1.0
            },
            generation: 0
        }
        .validate()
        .is_err());
        let mut document = ArtifactDocument {
            key: "a".into(),
            html: "<p>x</p>".into(),
            revision: 1,
            viewport: Viewport {
                width: 1,
                height: 1,
                scale: 1.0,
            },
            visible: true,
            theme: crate::Theme::Light,
            styles: crate::ArtifactStyles::default(),
        };
        let encoded = encode_json(&Control::Reconcile {
            document: document.clone(),
            generation: 1,
        })?;
        let text = String::from_utf8(encoded)
            .map_err(|e| invalid(&e.to_string()))?
            .replace("\"revision\":1", "\"revision\":-1");
        assert!(serde_json::from_str::<Control>(&text).is_err());
        document.key = "a".repeat(1025);
        assert!(Control::Reconcile {
            document,
            generation: 1
        }
        .validate()
        .is_err());
        let mut bad = meta();
        bad.generation = 0;
        assert!(bad.validate().is_err());
        Ok(())
    }
    #[test]
    fn frame_roundtrip_and_revision() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, &meta(), &[255, 0, 0, 255]).expect("valid test frame");
        let mut reader = Cursor::new(buffer);
        let message: Output =
            read_json(&mut reader).expect("valid JSON").expect("packet");
        let Output::Frame(header) = message else {
            panic!("expected frame metadata");
        };
        assert_eq!(header.revision, 7);
        assert_eq!(
            read_frame(&mut reader, &header).expect("valid frame"),
            [255, 0, 0, 255]
        );
    }
    #[test]
    fn reject_oversized_json_before_body_read() {
        let mut bytes = JSON.to_le_bytes().to_vec();
        bytes.extend(u32::MAX.to_le_bytes());
        assert!(read_json::<Control>(&mut Cursor::new(bytes)).is_err());
    }
    #[test]
    fn reject_truncated_and_wrong_kind() {
        assert!(read_json::<Control>(&mut Cursor::new(vec![1, 0])).is_err());
        let mut bytes = RGBA.to_le_bytes().to_vec();
        bytes.extend(4u32.to_le_bytes());
        assert!(read_json::<Control>(&mut Cursor::new(bytes)).is_err());
    }
    #[test]
    fn reject_frame_layout_and_keys() {
        let valid = meta();
        for bad in [
            FrameMetadata {
                stride: 5,
                ..valid.clone()
            },
            FrameMetadata {
                bytes: usize::MAX,
                ..valid.clone()
            },
            FrameMetadata {
                key: String::new(),
                ..valid.clone()
            },
            FrameMetadata {
                width: u32::MAX,
                ..valid
            },
        ] {
            assert!(bad.validate().is_err());
        }
    }
    #[test]
    fn reject_frame_body_length_before_allocation() {
        let mut bytes = RGBA.to_le_bytes().to_vec();
        bytes.extend(u32::MAX.to_le_bytes());
        assert!(read_frame(&mut Cursor::new(bytes), &meta()).is_err());
        assert!(read_frame(&mut Cursor::new(Vec::new()), &meta()).is_err());
    }
}
