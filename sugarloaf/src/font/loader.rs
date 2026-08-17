use std::path::PathBuf;

use crate::font::SharedData;
pub use ttf_parser::Language;

#[cfg(not(target_arch = "wasm32"))]
use font_kit::source::SystemSource;

#[derive(Clone, Debug)]
pub struct ID {
    #[cfg(not(target_arch = "wasm32"))]
    handle: Option<font_kit::handle::Handle>,
    // TODO: Fix wasm32
    #[cfg(target_arch = "wasm32")]
    _dummy: u32,
}

impl ID {
    #[cfg(not(target_arch = "wasm32"))]
    fn from_handle(handle: font_kit::handle::Handle) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn from_handle(_handle: ()) -> Self {
        Self { _dummy: 0 }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn to_handle(&self) -> Option<font_kit::handle::Handle> {
        self.handle.clone()
    }
}

#[derive(Clone, Debug)]
pub enum Source {
    File(PathBuf),
    Binary(SharedData),
}

/// Font query parameters
#[derive(Clone, Copy, Default, Debug)]
pub struct Query<'a> {
    pub families: &'a [Family<'a>],
    pub weight: Weight,
    pub stretch: Stretch,
    pub style: Style,
}

/// Font family
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family<'a> {
    Name(&'a str),
    Serif,
    SansSerif,
    Cursive,
    Fantasy,
    Monospace,
}

/// Font weight
#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Debug, Hash)]
pub struct Weight(pub u16);

impl Default for Weight {
    fn default() -> Weight {
        Weight::NORMAL
    }
}

impl Weight {
    pub const THIN: Weight = Weight(100);
    pub const EXTRA_LIGHT: Weight = Weight(200);
    pub const LIGHT: Weight = Weight(300);
    pub const NORMAL: Weight = Weight(400);
    pub const MEDIUM: Weight = Weight(500);
    pub const SEMIBOLD: Weight = Weight(600);
    pub const BOLD: Weight = Weight(700);
    pub const EXTRA_BOLD: Weight = Weight(800);
    pub const BLACK: Weight = Weight(900);
}

/// Font stretch/width
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default, PartialOrd, Ord)]
pub enum Stretch {
    UltraCondensed,
    ExtraCondensed,
    Condensed,
    SemiCondensed,
    #[default]
    Normal,
    SemiExpanded,
    Expanded,
    ExtraExpanded,
    UltraExpanded,
}

impl Stretch {
    fn to_number(self) -> u16 {
        match self {
            Stretch::UltraCondensed => 50,
            Stretch::ExtraCondensed => 62,
            Stretch::Condensed => 75,
            Stretch::SemiCondensed => 87,
            Stretch::Normal => 100,
            Stretch::SemiExpanded => 112,
            Stretch::Expanded => 125,
            Stretch::ExtraExpanded => 150,
            Stretch::UltraExpanded => 200,
        }
    }
}

/// Font style
#[derive(Clone, Default, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Style {
    #[default]
    Normal,
    Italic,
    Oblique,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct FontCandidate {
    handle: font_kit::handle::Handle,
    weight: Weight,
    stretch: Stretch,
    style: Style,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct NamedFontCandidate {
    family: String,
    candidate: FontCandidate,
}

#[cfg(not(target_arch = "wasm32"))]
fn candidate_from_data(
    handle: font_kit::handle::Handle,
    data: &[u8],
    font_index: u32,
) -> Option<FontCandidate> {
    let face = ttf_parser::Face::parse(data, font_index).ok()?;
    let stretch = match face.width() {
        ttf_parser::Width::UltraCondensed => Stretch::UltraCondensed,
        ttf_parser::Width::ExtraCondensed => Stretch::ExtraCondensed,
        ttf_parser::Width::Condensed => Stretch::Condensed,
        ttf_parser::Width::SemiCondensed => Stretch::SemiCondensed,
        ttf_parser::Width::Normal => Stretch::Normal,
        ttf_parser::Width::SemiExpanded => Stretch::SemiExpanded,
        ttf_parser::Width::Expanded => Stretch::Expanded,
        ttf_parser::Width::ExtraExpanded => Stretch::ExtraExpanded,
        ttf_parser::Width::UltraExpanded => Stretch::UltraExpanded,
    };
    let style = if face.is_oblique() {
        Style::Oblique
    } else if face.is_italic() {
        Style::Italic
    } else {
        Style::Normal
    };

    Some(FontCandidate {
        handle,
        weight: Weight(face.weight().to_number()),
        stretch,
        style,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn candidate_from_handle(handle: &font_kit::handle::Handle) -> Option<FontCandidate> {
    match handle {
        font_kit::handle::Handle::Path { path, font_index } => {
            let data = std::fs::read(path).ok()?;
            candidate_from_data(handle.clone(), &data, *font_index)
        }
        font_kit::handle::Handle::Memory { bytes, font_index } => {
            candidate_from_data(handle.clone(), bytes, *font_index)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn family_name(face: &ttf_parser::Face<'_>) -> Option<String> {
    face.names()
        .into_iter()
        .find(|name| {
            name.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY && name.is_unicode()
        })
        .and_then(|name| name.to_string())
        .or_else(|| {
            face.names()
                .into_iter()
                .find(|name| {
                    name.name_id == ttf_parser::name_id::FAMILY && name.is_unicode()
                })
                .and_then(|name| name.to_string())
        })
}

/// CSS-spec compliant font matching algorithm
/// Based on https://www.w3.org/TR/css-fonts-4/#font-matching-algorithm
#[cfg(not(target_arch = "wasm32"))]
fn find_best_match(candidates: &[FontCandidate], query: &Query) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }

    // Step 4a: Match font-stretch
    let mut matching_set: Vec<usize> = (0..candidates.len()).collect();

    let matches = matching_set
        .iter()
        .any(|&index| candidates[index].stretch == query.stretch);

    let matching_stretch = if matches {
        query.stretch
    } else if query.stretch <= Stretch::Normal {
        // closest stretch, first checking narrower values and then wider values
        let stretch = matching_set
            .iter()
            .filter(|&&index| candidates[index].stretch < query.stretch)
            .min_by_key(|&&index| {
                query.stretch.to_number() - candidates[index].stretch.to_number()
            });

        match stretch {
            Some(&matching_index) => candidates[matching_index].stretch,
            None => {
                let matching_index = *matching_set.iter().min_by_key(|&&index| {
                    candidates[index]
                        .stretch
                        .to_number()
                        .abs_diff(query.stretch.to_number())
                })?;
                candidates[matching_index].stretch
            }
        }
    } else {
        // closest stretch, first checking wider values and then narrower values
        let stretch = matching_set
            .iter()
            .filter(|&&index| candidates[index].stretch > query.stretch)
            .min_by_key(|&&index| {
                candidates[index].stretch.to_number() - query.stretch.to_number()
            });

        match stretch {
            Some(&matching_index) => candidates[matching_index].stretch,
            None => {
                let matching_index = *matching_set.iter().min_by_key(|&&index| {
                    query
                        .stretch
                        .to_number()
                        .abs_diff(candidates[index].stretch.to_number())
                })?;
                candidates[matching_index].stretch
            }
        }
    };
    matching_set.retain(|&index| candidates[index].stretch == matching_stretch);

    // Step 4b: Match font-style
    let style_preference = match query.style {
        Style::Italic => [Style::Italic, Style::Oblique, Style::Normal],
        Style::Oblique => [Style::Oblique, Style::Italic, Style::Normal],
        Style::Normal => [Style::Normal, Style::Oblique, Style::Italic],
    };

    let matching_style = *style_preference.iter().find(|&query_style| {
        matching_set
            .iter()
            .any(|&index| candidates[index].style == *query_style)
    })?;

    matching_set.retain(|&index| candidates[index].style == matching_style);

    // Step 4c: Match font-weight
    let weight = query.weight.0;

    let matching_weight = if matching_set
        .iter()
        .any(|&index| candidates[index].weight.0 == weight)
    {
        Weight(weight)
    } else if (400..450).contains(&weight)
        && matching_set
            .iter()
            .any(|&index| candidates[index].weight.0 == 500)
    {
        Weight::MEDIUM
    } else if (450..=500).contains(&weight)
        && matching_set
            .iter()
            .any(|&index| candidates[index].weight.0 == 400)
    {
        Weight::NORMAL
    } else if weight <= 500 {
        // Closest weight, first checking thinner values and then fatter ones
        let idx = matching_set
            .iter()
            .filter(|&&index| candidates[index].weight.0 <= weight)
            .min_by_key(|&&index| weight - candidates[index].weight.0);

        match idx {
            Some(&matching_index) => candidates[matching_index].weight,
            None => {
                let matching_index = *matching_set
                    .iter()
                    .min_by_key(|&&index| candidates[index].weight.0.abs_diff(weight))?;
                candidates[matching_index].weight
            }
        }
    } else {
        // Closest weight, first checking fatter values and then thinner ones
        let idx = matching_set
            .iter()
            .filter(|&&index| candidates[index].weight.0 >= weight)
            .min_by_key(|&&index| candidates[index].weight.0 - weight);

        match idx {
            Some(&matching_index) => candidates[matching_index].weight,
            None => {
                let matching_index = *matching_set
                    .iter()
                    .min_by_key(|&&index| weight.abs_diff(candidates[index].weight.0))?;
                candidates[matching_index].weight
            }
        }
    };
    matching_set.retain(|&index| candidates[index].weight == matching_weight);

    matching_set.into_iter().next()
}

pub struct Database {
    #[cfg(not(target_arch = "wasm32"))]
    system_source: SystemSource,
    #[cfg(not(target_arch = "wasm32"))]
    additional_fonts: Vec<NamedFontCandidate>,
}

impl Database {
    pub fn new() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            system_source: SystemSource::new(),
            #[cfg(not(target_arch = "wasm32"))]
            additional_fonts: Vec::new(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_fonts_dir<P: AsRef<std::path::Path>>(&mut self, path: P) {
        use font_kit::handle::Handle;
        use walkdir::WalkDir;

        for entry in WalkDir::new(path.as_ref())
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    let ext_lower = ext.to_string_lossy().to_lowercase();
                    if ext_lower == "ttf"
                        || ext_lower == "otf"
                        || ext_lower == "ttc"
                        || ext_lower == "otc"
                    {
                        let Ok(data) = std::fs::read(path) else {
                            continue;
                        };
                        let face_count =
                            ttf_parser::fonts_in_collection(&data).unwrap_or(1);
                        for font_index in 0..face_count {
                            let Ok(face) = ttf_parser::Face::parse(&data, font_index)
                            else {
                                continue;
                            };
                            let Some(family) = family_name(&face) else {
                                continue;
                            };
                            let handle =
                                Handle::from_path(path.to_path_buf(), font_index);
                            if let Some(candidate) =
                                candidate_from_data(handle, &data, font_index)
                            {
                                self.additional_fonts
                                    .push(NamedFontCandidate { family, candidate });
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn load_fonts_dir<P: AsRef<std::path::Path>>(&mut self, _path: P) {
        // No-op for WASM
    }

    /// Query for a font matching the given criteria
    /// Gets ALL faces from the family, then applies CSS-spec matching
    #[cfg(not(target_arch = "wasm32"))]
    pub fn query(&self, query: &Query) -> Option<ID> {
        use font_kit::family_name::FamilyName;

        tracing::debug!("Query starting: {:?}", query);

        // Convert query to font-kit family name
        for family in query.families {
            let family_name = match family {
                Family::Name(name) => FamilyName::Title(name.to_string()),
                Family::Serif => FamilyName::Serif,
                Family::SansSerif => FamilyName::SansSerif,
                Family::Cursive => FamilyName::Cursive,
                Family::Fantasy => FamilyName::Fantasy,
                Family::Monospace => FamilyName::Monospace,
            };

            // Get the family name string
            let family_name_str = match &family_name {
                FamilyName::Title(s) => s.as_str(),
                FamilyName::Serif => "serif",
                FamilyName::SansSerif => "sans-serif",
                FamilyName::Cursive => "cursive",
                FamilyName::Fantasy => "fantasy",
                FamilyName::Monospace => "monospace",
            };
            let family_name_lower = family_name_str.to_lowercase();

            tracing::debug!(
                "Searching for family: '{}' (lowercase: '{}')",
                family_name_str,
                family_name_lower
            );

            let mut candidates = Vec::new();

            // Step 1: collect all font faces from additional sources (user directories)
            tracing::debug!(
                "checking {} additional sources",
                self.additional_fonts.len()
            );
            for font in &self.additional_fonts {
                if font.family.to_lowercase() == family_name_lower {
                    candidates.push(font.candidate.clone());
                }
            }

            // step 2: try case-insensitive on system fonts
            if candidates.is_empty() {
                tracing::debug!("System fonts: trying case-insensitive match");
                if let Ok(families) = self.system_source.all_families() {
                    tracing::debug!("System has {} families total", families.len());
                    for system_family_name in families {
                        if system_family_name.to_lowercase() == family_name_lower {
                            tracing::debug!(
                                "  Found case-insensitive system match: '{}'",
                                system_family_name
                            );
                            if let Ok(family_handle) = self
                                .system_source
                                .select_family_by_name(&system_family_name)
                            {
                                for handle in family_handle.fonts() {
                                    if let Some(candidate) = candidate_from_handle(handle)
                                    {
                                        candidates.push(candidate);
                                    }
                                }
                            }
                            break;
                        }
                    }
                }
            }

            tracing::debug!("Total candidates found: {}", candidates.len());

            // Step 3: apply CSS-spec matching algorithm to select the best face
            if let Some(index) = find_best_match(&candidates, query) {
                tracing::debug!("Best match selected at index {}", index);
                return Some(ID::from_handle(candidates[index].handle.clone()));
            } else {
                tracing::debug!("No best match found from candidates");
            }
        }

        tracing::debug!("Query failed: no fonts found");
        None
    }

    #[cfg(target_arch = "wasm32")]
    pub fn query(&self, _query: &Query) -> Option<ID> {
        None
    }

    /// Get face source (path and index) for a given ID
    #[cfg(not(target_arch = "wasm32"))]
    pub fn face_source(&self, id: ID) -> Option<(Source, u32)> {
        // Reconstruct handle from ID
        tracing::debug!("face_source: getting source for ID");
        if let Some(handle) = id.to_handle() {
            tracing::debug!("face_source: handle retrieved");
            match handle {
                font_kit::handle::Handle::Path {
                    ref path,
                    font_index,
                } => {
                    tracing::debug!(
                        "face_source: Path source - {}, index {}",
                        path.display(),
                        font_index
                    );
                    return Some((Source::File(path.clone()), font_index));
                }
                font_kit::handle::Handle::Memory { bytes, font_index } => {
                    tracing::debug!(
                        "face_source: Memory source, {} bytes, index {}",
                        bytes.len(),
                        font_index
                    );
                    // Try to find the actual file path for this font
                    if let Some(path) = find_font_path_from_data(&bytes) {
                        tracing::debug!(
                            "face_source: Found file path for memory font: {}",
                            path.display()
                        );
                        return Some((Source::File(path), font_index));
                    }
                    // Fallback to binary if path not found
                    return Some((
                        Source::Binary(SharedData::new(bytes.to_vec())),
                        font_index,
                    ));
                }
            }
        } else {
            tracing::debug!("face_source: handle is None!");
        }
        tracing::debug!("face_source: returning None");
        None
    }

    #[cfg(target_arch = "wasm32")]
    pub fn face_source(&self, _id: ID) -> Option<(Source, u32)> {
        None
    }
}

impl Default for Database {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "macos")]
const SYSTEM_FONT_DIRS: &[&str] = &[
    "/Library/Fonts",
    "/System/Library/Fonts",
    "/System/Library/AssetsV2",
    "/Network/Library/Fonts",
];

#[cfg(target_os = "windows")]
const SYSTEM_FONT_DIRS: &[&str] = &[
    // Note: actual paths resolved at runtime using environment variables
];

#[cfg(all(unix, not(target_os = "macos")))]
const SYSTEM_FONT_DIRS: &[&str] = &["/usr/share/fonts", "/usr/local/share/fonts"];

#[cfg(not(target_arch = "wasm32"))]
fn get_font_name(face: &ttf_parser::Face) -> Option<String> {
    face.names()
        .into_iter()
        .find(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME && n.is_unicode())
        .and_then(|n| n.to_string())
        .or_else(|| {
            face.names()
                .into_iter()
                .find(|n| n.name_id == ttf_parser::name_id::FAMILY && n.is_unicode())
                .and_then(|n| n.to_string())
        })
}

#[cfg(not(target_arch = "wasm32"))]
fn get_font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = SYSTEM_FONT_DIRS.iter().map(PathBuf::from).collect();

    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Fonts"));
    }

    #[cfg(target_os = "windows")]
    {
        // System fonts
        if let Some(windir) = std::env::var_os("SYSTEMROOT") {
            dirs.push(PathBuf::from(windir).join("Fonts"));
        }
        // User fonts
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            let profile = PathBuf::from(profile);
            dirs.push(profile.join("AppData/Local/Microsoft/Windows/Fonts"));
            dirs.push(profile.join("AppData/Roaming/Microsoft/Windows/Fonts"));
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".fonts"));
        dirs.push(home.join(".local/share/fonts"));
    }

    dirs
}

// find the file path for a font given its binary data.
// parses the font to get its PostScript name, then searches system directories.
#[cfg(not(target_arch = "wasm32"))]
fn find_font_path_from_data(data: &[u8]) -> Option<PathBuf> {
    use memmap2::Mmap;
    use std::fs::File;
    use walkdir::WalkDir;

    // Parse font to get its name
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let target_name = get_font_name(&face)?;
    let target_name_lower = target_name.to_lowercase();

    tracing::debug!("find_font_path_from_data: searching for '{}'", target_name);

    // Search each directory for the font file
    for dir in get_font_dirs() {
        if !dir.exists() {
            continue;
        }

        for entry in WalkDir::new(&dir).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            // Check extension
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase());

            match ext.as_deref() {
                Some("ttf") | Some("otf") | Some("ttc") | Some("otc") => {}
                _ => continue,
            }

            // Use memory mapping for efficient file access
            let Ok(file) = File::open(path) else {
                continue;
            };
            let Ok(mmap) = (unsafe { Mmap::map(&file) }) else {
                continue;
            };

            // Handle font collections (TTC/OTC) - check all faces
            let face_count = ttf_parser::fonts_in_collection(&mmap).unwrap_or(1);
            for index in 0..face_count {
                if let Ok(file_face) = ttf_parser::Face::parse(&mmap, index) {
                    if let Some(file_name) = get_font_name(&file_face) {
                        if file_name.to_lowercase() == target_name_lower {
                            tracing::debug!(
                                "find_font_path_from_data: found match at {}",
                                path.display()
                            );
                            return Some(path.to_path_buf());
                        }
                    }
                }
            }
        }
    }

    tracing::debug!(
        "find_font_path_from_data: no file found for '{}'",
        target_name
    );
    None
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use font_kit::handle::Handle;

    #[test]
    fn parses_font_metadata_without_freetype() {
        let data = include_bytes!("../../resources/test-fonts/OpenSans-Italic.ttf");
        let handle = Handle::from_memory(std::sync::Arc::new(data.to_vec()), 0);
        let candidate = candidate_from_data(handle, data, 0).expect("valid test font");
        let face = ttf_parser::Face::parse(data, 0).expect("valid test font");

        assert_eq!(candidate.style, Style::Italic);
        assert_eq!(candidate.weight, Weight(400));
        assert_eq!(family_name(&face).as_deref(), Some("Open Sans"));
    }

    #[test]
    fn rejects_invalid_font_data_without_panicking() {
        let handle = Handle::from_memory(std::sync::Arc::new(vec![0; 16]), 0);
        assert!(candidate_from_data(handle, &[0; 16], 0).is_none());
    }
}
