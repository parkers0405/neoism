use librashader_presets::{ParsePresetError, ShaderFeatures, ShaderPreset};
use std::fs;
use std::io::Write;

macro_rules! resource {
    ($resource:literal) => {
        include_bytes!($resource) as &[u8]
    };
}

use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum LoadError {
    ParseError(ParsePresetError),
    IoError(std::io::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LoadError::ParseError(details) => {
                write!(f, "Parse failed: {details}")
            }
            LoadError::IoError(details) => {
                write!(f, "File failed: {details}")
            }
        }
    }
}

impl Error for LoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            LoadError::IoError(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LoadError {
    fn from(err: std::io::Error) -> Self {
        LoadError::IoError(err)
    }
}

const NEWPIXIECRT_ACCUMULATE: &[u8] = resource!("./newpixiecrt/accumulate.slang");
const NEWPIXIECRT_BLUR_HORIZ: &[u8] = resource!("./newpixiecrt/blur_horiz.slang");
const NEWPIXIECRT_BLUR_VERT: &[u8] = resource!("./newpixiecrt/blur_vert.slang");
const NEWPIXIECRT_CRTFRAME: &[u8] = resource!("./newpixiecrt/crtframe.png");
const NEWPIXIECRT_NEWPIXIECRT: &[u8] = resource!("./newpixiecrt/newpixie-crt.slang");
const NEWPIXIECRT_NEWPIXIECRTP: &[u8] = resource!("./newpixiecrt/newpixie-crt.slangp");

pub fn newpixiecrt() -> Result<ShaderPreset, LoadError> {
    let dir_path = std::env::temp_dir().join("newpixiecrt");
    if !dir_path.exists() {
        fs::create_dir_all(&dir_path)?;
    }

    let files = vec![
        ("accumulate.slang", NEWPIXIECRT_ACCUMULATE),
        ("blur_horiz.slang", NEWPIXIECRT_BLUR_HORIZ),
        ("blur_vert.slang", NEWPIXIECRT_BLUR_VERT),
        ("crtframe.png", NEWPIXIECRT_CRTFRAME),
        ("newpixie-crt.slang", NEWPIXIECRT_NEWPIXIECRT),
        ("newpixie-crt.slangp", NEWPIXIECRT_NEWPIXIECRTP),
    ];

    // Create files in the directory
    for (filename, content) in files {
        let file_path = dir_path.join(filename);
        let mut file = fs::File::create(file_path)?;
        file.write_all(content)?;
    }

    match ShaderPreset::try_parse(
        dir_path.join("newpixie-crt.slangp"),
        ShaderFeatures::NONE,
    ) {
        Ok(preset) => Ok(preset),
        Err(err) => Err(LoadError::ParseError(err)),
    }
}

const CRT_CURVE: &[u8] = resource!("./crt_curve.slang");
const CRT_CURVE_PRESET: &[u8] = resource!("./crt_curve.slangp");

pub fn crt_curve() -> Result<ShaderPreset, LoadError> {
    let dir_path = std::env::temp_dir().join("crt_curve");
    if !dir_path.exists() {
        fs::create_dir_all(&dir_path)?;
    }

    let files = vec![
        ("crt_curve.slang", CRT_CURVE),
        ("crt_curve.slangp", CRT_CURVE_PRESET),
    ];

    for (filename, content) in files {
        let file_path = dir_path.join(filename);
        let mut file = fs::File::create(file_path)?;
        file.write_all(content)?;
    }

    match ShaderPreset::try_parse(dir_path.join("crt_curve.slangp"), ShaderFeatures::NONE)
    {
        Ok(preset) => Ok(preset),
        Err(err) => Err(LoadError::ParseError(err)),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn crt_curve_builtin_parses() {
        super::crt_curve().expect("crt_curve builtin should parse");
    }
}
