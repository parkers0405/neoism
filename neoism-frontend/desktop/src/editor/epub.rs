#![allow(dead_code)] // EPUB support is staged but not routed into the desktop shell yet.

//! Native EPUB book model.
//!
//! EPUB is a ZIP container whose package document defines metadata, a
//! manifest and a linear reading order (the spine).  This module deliberately
//! keeps that book structure separate from the Markdown renderer: each XHTML
//! spine item is converted into reader source on demand, while durable
//! locations continue to use the EPUB resource href rather than transient
//! rendered line numbers alone.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{Read, Seek},
    ops::Range,
    path::{Component, Path, PathBuf},
};

use neoism_ui::editor::markdown::MarkdownPane;
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zip::ZipArchive;

const MAX_XML_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CHAPTER_BYTES: u64 = 32 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const READER_STATE_VERSION: u32 = 2;
const READER_STATE_DIR: &str = ".neoism/reader/books";

#[derive(Debug, Error)]
pub enum EpubError {
    #[error("could not open EPUB: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid EPUB archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid EPUB XML: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("EPUB is missing {0}")]
    Missing(&'static str),
    #[error("EPUB resource is too large: {0}")]
    TooLarge(String),
    #[error("EPUB contains an unsafe resource path: {0}")]
    UnsafePath(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EpubMetadata {
    pub title: String,
    pub creators: Vec<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpubManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub properties: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpubChapter {
    pub idref: String,
    /// Archive-relative, normalized resource path.
    pub href: String,
    pub title: String,
    pub linear: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpubTocEntry {
    pub label: String,
    pub href: String,
    pub children: Vec<EpubTocEntry>,
}

#[derive(Clone, Debug)]
pub struct EpubBook {
    /// Stable identity derived from package metadata and reading order. It is
    /// deliberately independent of the file path so reader state follows a
    /// book when it is renamed or moved.
    pub id: String,
    pub path: PathBuf,
    pub package_path: String,
    pub metadata: EpubMetadata,
    pub manifest: HashMap<String, EpubManifestItem>,
    pub chapters: Vec<EpubChapter>,
    pub toc: Vec<EpubTocEntry>,
}

#[derive(Clone, Debug, Default)]
pub struct EpubChapterContent {
    pub source: String,
    pub anchors: HashMap<String, usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpubLocation {
    /// Stable archive resource href, preferred when restoring after an EPUB
    /// is updated and its spine indices shift.
    pub chapter_href: String,
    #[serde(default)]
    pub source_line: usize,
    #[serde(default)]
    pub source_column: usize,
    #[serde(default)]
    pub fragment: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpubAnnotation {
    pub id: String,
    pub start: EpubLocation,
    pub end: EpubLocation,
    pub selected_text: String,
    #[serde(default)]
    pub note: String,
    #[serde(default = "default_highlight_color")]
    pub color: String,
    #[serde(default)]
    pub chapter_title: String,
    #[serde(default)]
    pub page_index: usize,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
    #[serde(default)]
    pub collection_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpubAnnotationCollection {
    pub id: String,
    pub name: String,
    pub file_name: String,
    #[serde(default)]
    pub created_unix_ms: u64,
    #[serde(default)]
    pub updated_unix_ms: u64,
}

fn default_highlight_color() -> String {
    "yellow".to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpubReadingState {
    #[serde(default = "reader_state_version")]
    pub version: u32,
    #[serde(default)]
    pub book_id: String,
    #[serde(default)]
    pub book_note_id: String,
    #[serde(default)]
    pub book_note_path: Option<String>,
    #[serde(default)]
    pub last_known_path: PathBuf,
    #[serde(default)]
    pub location: EpubLocation,
    #[serde(default)]
    pub progress: f32,
    #[serde(default)]
    pub scroll_y: f32,
    #[serde(default)]
    pub annotations: Vec<EpubAnnotation>,
    #[serde(default)]
    pub collections: Vec<EpubAnnotationCollection>,
}

fn reader_state_version() -> u32 {
    READER_STATE_VERSION
}

impl Default for EpubReadingState {
    fn default() -> Self {
        Self {
            version: READER_STATE_VERSION,
            book_id: String::new(),
            book_note_id: String::new(),
            book_note_path: None,
            last_known_path: PathBuf::new(),
            location: EpubLocation::default(),
            progress: 0.0,
            scroll_y: 0.0,
            annotations: Vec::new(),
            collections: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct EpubPane {
    pub book: EpubBook,
    pub chapter_index: usize,
    pub page_index: usize,
    pub page_count: usize,
    pub markdown: MarkdownPane,
    pub state: EpubReadingState,
    pub state_path: PathBuf,
    pub vault_root: PathBuf,
    pub rendered_images: Vec<EpubRenderedImage>,
    pub chapter_anchors: HashMap<String, usize>,
    pub error: Option<String>,
    pub showing_contents: bool,
    chapter_content: EpubChapterContent,
    page_ranges: Vec<Range<usize>>,
    page_start_line: usize,
    page_counts: HashMap<usize, usize>,
    skippable_chapters: HashMap<usize, bool>,
    state_dirty: bool,
    last_state_save: web_time::Instant,
}

#[derive(Clone, Debug)]
pub struct EpubRenderedImage {
    pub line: usize,
    pub image_id: u32,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl EpubBook {
    pub fn open(path: PathBuf) -> Result<Self, EpubError> {
        let mut archive = ZipArchive::new(File::open(&path)?)?;
        let container =
            read_zip_text(&mut archive, "META-INF/container.xml", MAX_XML_BYTES)?;
        let container_xml = epub_xml_for_parse(&container);
        let container_doc = Document::parse(&container_xml)?;
        let package_path = container_doc
            .descendants()
            .find(|node| node.has_tag_name("rootfile"))
            .and_then(|node| node.attribute("full-path"))
            .ok_or(EpubError::Missing("package rootfile"))?;
        let package_path = normalize_archive_path("", package_path)?;
        let package_xml = read_zip_text(&mut archive, &package_path, MAX_XML_BYTES)?;
        let package_xml = epub_xml_for_parse(&package_xml);
        let package_doc = Document::parse(&package_xml)?;
        let package_dir = archive_parent(&package_path);

        let metadata = parse_metadata(&package_doc);
        let manifest = parse_manifest(&package_doc, package_dir)?;
        let mut chapters = parse_spine(&package_doc, &manifest)?;
        if chapters.is_empty() {
            return Err(EpubError::Missing("reading-order spine"));
        }
        let toc = parse_toc(&mut archive, &package_doc, &manifest)?;
        let mut toc_titles = HashMap::new();
        collect_toc_titles(&toc, &mut toc_titles);
        for (index, chapter) in chapters.iter_mut().enumerate() {
            chapter.title = toc_titles
                .get(&chapter.href)
                .cloned()
                .unwrap_or_else(|| format!("Chapter {}", index + 1));
        }
        let id = stable_book_id(&metadata, &package_path, &chapters);

        Ok(Self {
            id,
            path,
            package_path,
            metadata,
            manifest,
            chapters,
            toc,
        })
    }

    pub fn load_chapter_source(&self, index: usize) -> Result<String, EpubError> {
        Ok(self.load_chapter_content(index)?.source)
    }

    pub fn load_chapter_content(
        &self,
        index: usize,
    ) -> Result<EpubChapterContent, EpubError> {
        let chapter = self
            .chapters
            .get(index)
            .ok_or(EpubError::Missing("spine chapter"))?;
        let mut archive = ZipArchive::new(File::open(&self.path)?)?;
        let xhtml = read_zip_text(&mut archive, &chapter.href, MAX_CHAPTER_BYTES)?;
        let chapter_dir = archive_parent(&chapter.href);
        Ok(xhtml_to_reader_content(&xhtml, chapter_dir))
    }

    pub fn load_resource(
        &self,
        href: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, EpubError> {
        let href = normalize_archive_path("", href)?;
        let mut archive = ZipArchive::new(File::open(&self.path)?)?;
        let mut entry = archive.by_name(&href)?;
        if entry.size() > max_bytes {
            return Err(EpubError::TooLarge(href));
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.by_ref().take(max_bytes + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max_bytes {
            return Err(EpubError::TooLarge(href));
        }
        Ok(bytes)
    }

    pub fn chapter_index_for_href(&self, href: &str) -> Option<usize> {
        let (resource, _) = split_fragment(href);
        let normalized = normalize_archive_path("", resource).ok()?;
        self.chapters
            .iter()
            .position(|chapter| chapter.href == normalized)
    }
}

impl EpubPane {
    pub fn load(path: PathBuf, state_root: &Path) -> Self {
        Self::load_with_vault(path, state_root, state_root, None)
    }

    pub fn load_in_vault(
        path: PathBuf,
        vault_root: &Path,
        legacy_state_root: Option<&Path>,
    ) -> Self {
        let state_root = vault_reader_state_root(vault_root);
        Self::load_with_vault(path, vault_root, &state_root, legacy_state_root)
    }

    fn load_with_vault(
        path: PathBuf,
        vault_root: &Path,
        state_root: &Path,
        legacy_state_root: Option<&Path>,
    ) -> Self {
        match EpubBook::open(path.clone()) {
            Ok(mut book) => {
                let state_path = state_path_for_book_id(state_root, &book.id);
                let legacy_path = legacy_state_root.and_then(|root| {
                    let direct = legacy_state_path_for_book(root, &path);
                    direct
                        .exists()
                        .then_some(direct)
                        .or_else(|| find_matching_legacy_state(root, &book))
                });
                let migrated = !state_path.exists()
                    && legacy_path
                        .as_ref()
                        .is_some_and(|candidate| candidate.exists());
                let mut state = load_reading_state(&state_path)
                    .or_else(|| legacy_path.as_deref().and_then(load_reading_state))
                    .unwrap_or_default();
                state.version = READER_STATE_VERSION;
                state.book_id.clone_from(&book.id);
                if state.book_note_id.is_empty() {
                    state.book_note_id = format!("book-note-{}", book.id);
                }
                state.last_known_path =
                    path.canonicalize().unwrap_or_else(|_| path.clone());
                let restored_chapter_index = book
                    .chapter_index_for_href(&state.location.chapter_href)
                    .unwrap_or(0);
                let chapter_index =
                    restored_reading_chapter_index(&book, restored_chapter_index);
                if chapter_index != restored_chapter_index {
                    state.location.chapter_href =
                        book.chapters[chapter_index].href.clone();
                    state.location.fragment = None;
                }
                match prepare_chapter_page(&book, chapter_index, &state) {
                    Ok(mut prepared) => {
                        let chapter_title =
                            reader_chapter_title(&book, chapter_index, &prepared.content);
                        if let Some(chapter) = book.chapters.get_mut(chapter_index) {
                            chapter.title = chapter_title.clone();
                        }
                        prepared.markdown.title = if prepared.page_index == 0 {
                            chapter_title
                        } else {
                            String::new()
                        };
                        let page_count = prepared.page_ranges.len();
                        let mut page_counts = HashMap::new();
                        page_counts.insert(chapter_index, page_count);
                        let mut pane = Self {
                            book,
                            chapter_index,
                            page_index: prepared.page_index,
                            page_count,
                            markdown: prepared.markdown,
                            state,
                            state_path,
                            vault_root: vault_root.to_path_buf(),
                            rendered_images: Vec::new(),
                            chapter_anchors: prepared.content.anchors.clone(),
                            error: None,
                            showing_contents: false,
                            chapter_content: prepared.content,
                            page_ranges: prepared.page_ranges,
                            page_start_line: prepared.page_start_line,
                            page_counts,
                            skippable_chapters: HashMap::new(),
                            state_dirty: false,
                            last_state_save: web_time::Instant::now(),
                        };
                        pane.refresh_reader_highlights();
                        pane.refresh_chapter_images();
                        if pane.state.book_note_path.is_some()
                            || pane
                                .state
                                .annotations
                                .iter()
                                .any(|annotation| !annotation.note.trim().is_empty())
                        {
                            if let Some(path) = pane.book_note_path() {
                                let _ = pane.adopt_book_note_path(path);
                            }
                        }
                        if migrated || !pane.state_path.exists() {
                            let _ = pane.save_state();
                        } else {
                            pane.state_dirty = true;
                        }
                        pane
                    }
                    Err(error) => Self::error(
                        path,
                        state_path,
                        vault_root.to_path_buf(),
                        error.to_string(),
                    ),
                }
            }
            Err(error) => {
                let state_path = legacy_state_path_for_book(state_root, &path);
                Self::error(
                    path,
                    state_path,
                    vault_root.to_path_buf(),
                    error.to_string(),
                )
            }
        }
    }

    fn error(
        path: PathBuf,
        state_path: PathBuf,
        vault_root: PathBuf,
        error: String,
    ) -> Self {
        let title = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "EPUB".to_string());
        let source = format!("# Could not open {title}\n\n{error}");
        let mut markdown = MarkdownPane::from_source(path.clone(), &source);
        markdown.read_only = true;
        Self {
            book: EpubBook {
                id: String::new(),
                path,
                package_path: String::new(),
                metadata: EpubMetadata {
                    title,
                    ..EpubMetadata::default()
                },
                manifest: HashMap::new(),
                chapters: Vec::new(),
                toc: Vec::new(),
            },
            chapter_index: 0,
            page_index: 0,
            page_count: 1,
            markdown,
            state: EpubReadingState::default(),
            state_path,
            vault_root,
            rendered_images: Vec::new(),
            chapter_anchors: HashMap::new(),
            error: Some(error),
            showing_contents: false,
            chapter_content: EpubChapterContent {
                source,
                anchors: HashMap::new(),
            },
            page_ranges: vec![0..1],
            page_start_line: 0,
            page_counts: HashMap::new(),
            skippable_chapters: HashMap::new(),
            state_dirty: false,
            last_state_save: web_time::Instant::now(),
        }
    }

    pub fn go_to_chapter(&mut self, index: usize) -> Result<bool, EpubError> {
        if index >= self.book.chapters.len() {
            return Ok(false);
        }
        let changed =
            self.showing_contents || index != self.chapter_index || self.page_index != 0;
        if !changed {
            return Ok(false);
        }
        self.load_chapter_page(index, 0, 0, 0)?;
        self.state.location = EpubLocation {
            chapter_href: self.book.chapters[index].href.clone(),
            ..EpubLocation::default()
        };
        self.update_progress();
        let _ = self.save_state();
        Ok(true)
    }

    pub fn go_to_page(&mut self, index: usize, page: usize) -> Result<bool, EpubError> {
        if index >= self.book.chapters.len() {
            return Ok(false);
        }
        let changed = self.showing_contents
            || index != self.chapter_index
            || page != self.page_index;
        if !changed {
            return Ok(false);
        }
        self.load_chapter_page(index, page, 0, 0)?;
        self.state.location = EpubLocation {
            chapter_href: self.book.chapters[index].href.clone(),
            source_line: self.page_start_line,
            ..EpubLocation::default()
        };
        self.update_progress();
        let _ = self.save_state();
        Ok(true)
    }

    pub fn next_page(&mut self) -> Result<bool, EpubError> {
        if self.showing_contents {
            if self.page_index + 1 < self.page_count {
                return self.go_to_contents_page(self.page_index + 1);
            }
            let insertion = self.contents_insertion_index();
            return match self.next_reading_chapter(insertion)? {
                Some(index) => self.go_to_page(index, 0),
                None => Ok(false),
            };
        }
        if self.page_index + 1 < self.page_count {
            return self.go_to_page(self.chapter_index, self.page_index + 1);
        }
        let insertion = self.contents_insertion_index();
        let next = self.next_reading_chapter(self.chapter_index.saturating_add(1))?;
        if !self.book.toc.is_empty()
            && self.chapter_index < insertion
            && next.is_some_and(|index| index >= insertion)
        {
            return self.open_contents_page();
        }
        match next {
            Some(index) => self.go_to_page(index, 0),
            None => Ok(false),
        }
    }

    pub fn previous_page(&mut self) -> Result<bool, EpubError> {
        if self.showing_contents {
            return if self.page_index > 0 {
                self.go_to_contents_page(self.page_index - 1)
            } else {
                match self.previous_reading_chapter(self.contents_insertion_index())? {
                    Some(index) => {
                        let pages = self.page_count_for_chapter(index)?;
                        self.go_to_page(index, pages.saturating_sub(1))
                    }
                    None => Ok(false),
                }
            };
        }
        if self.page_index > 0 {
            return self.go_to_page(self.chapter_index, self.page_index - 1);
        }
        let insertion = self.contents_insertion_index();
        let previous = self.previous_reading_chapter(self.chapter_index)?;
        if !self.book.toc.is_empty()
            && self.chapter_index >= insertion
            && previous.is_none_or(|index| index < insertion)
        {
            return self.open_contents_page_at_end();
        }
        let Some(previous) = previous else {
            return Ok(false);
        };
        let page_count = self.page_count_for_chapter(previous)?;
        self.go_to_page(previous, page_count.saturating_sub(1))
    }

    pub fn page_count_for_chapter(&mut self, index: usize) -> Result<usize, EpubError> {
        if let Some(count) = self.page_counts.get(&index).copied() {
            return Ok(count);
        }
        let content = self.book.load_chapter_content(index)?;
        let count = paginate_reader_source(&content.source).len().max(1);
        self.page_counts.insert(index, count);
        Ok(count)
    }

    pub fn page_previews_for_chapter(
        &mut self,
        index: usize,
    ) -> Result<Vec<String>, EpubError> {
        let content = if index == self.chapter_index {
            self.chapter_content.clone()
        } else {
            self.book.load_chapter_content(index)?
        };
        let ranges = paginate_reader_source(&content.source);
        self.page_counts.insert(index, ranges.len().max(1));
        Ok(ranges
            .into_iter()
            .map(|range| page_preview(&page_source(&content.source, range)))
            .collect())
    }

    fn contents_insertion_index(&self) -> usize {
        first_toc_chapter_index(&self.book, &self.book.toc)
            .unwrap_or(0)
            .min(self.book.chapters.len())
    }

    fn chapter_is_skippable(&mut self, index: usize) -> Result<bool, EpubError> {
        if let Some(value) = self.skippable_chapters.get(&index).copied() {
            return Ok(value);
        }
        let value = reader_chapter_is_skippable(
            &self.book,
            index,
            self.contents_insertion_index(),
        )?;
        self.skippable_chapters.insert(index, value);
        Ok(value)
    }

    fn next_reading_chapter(&mut self, start: usize) -> Result<Option<usize>, EpubError> {
        let mut index = start;
        while index < self.book.chapters.len() {
            if !self.chapter_is_skippable(index)? {
                return Ok(Some(index));
            }
            index += 1;
        }
        Ok(None)
    }

    fn previous_reading_chapter(
        &mut self,
        before: usize,
    ) -> Result<Option<usize>, EpubError> {
        let mut index = before;
        while index > 0 {
            index -= 1;
            if !self.chapter_is_skippable(index)? {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    /// Mount the EPUB navigation document as a normal, read-only reader page.
    /// It stays lazy: chapter XHTML is not opened merely to show the contents.
    pub fn open_contents_page(&mut self) -> Result<bool, EpubError> {
        if self.book.toc.is_empty() {
            return Ok(false);
        }
        if !self.showing_contents {
            self.capture_location();
            let source = contents_reader_source(&self.book.toc);
            self.chapter_content = EpubChapterContent {
                source,
                anchors: HashMap::new(),
            };
            self.page_ranges = paginate_reader_source(&self.chapter_content.source);
        }
        self.mount_contents_page(0);
        Ok(true)
    }

    fn open_contents_page_at_end(&mut self) -> Result<bool, EpubError> {
        self.open_contents_page()?;
        let last = self.page_count.saturating_sub(1);
        self.mount_contents_page(last);
        Ok(true)
    }

    fn go_to_contents_page(&mut self, page: usize) -> Result<bool, EpubError> {
        if !self.showing_contents {
            self.open_contents_page()?;
        }
        let changed = page != self.page_index;
        self.mount_contents_page(page);
        Ok(changed)
    }

    fn mount_contents_page(&mut self, page: usize) {
        if self.page_ranges.is_empty() {
            self.page_ranges.push(0..1);
        }
        self.showing_contents = true;
        self.page_count = self.page_ranges.len();
        self.page_index = page.min(self.page_count.saturating_sub(1));
        let range = self.page_ranges[self.page_index].clone();
        self.page_start_line = range.start;
        self.markdown
            .set_source_for_navigation(&page_source(&self.chapter_content.source, range));
        self.markdown.title = if self.page_index == 0 {
            "Contents".to_string()
        } else {
            String::new()
        };
        self.markdown.reader_footer = Some(format!(
            "Page {} of {}",
            self.page_index + 1,
            self.page_count
        ));
        self.markdown.path = self.book.path.clone();
        self.markdown.read_only = true;
        self.markdown.enter_normal();
        self.markdown.jump_to_line(1);
        self.markdown.restore_scroll_position(0.0);
        self.markdown.reader_highlights.clear();
        self.rendered_images.clear();
        self.chapter_anchors.clear();
    }

    fn load_chapter_page(
        &mut self,
        index: usize,
        page: usize,
        source_line: usize,
        source_column: usize,
    ) -> Result<(), EpubError> {
        self.capture_location();
        if self.showing_contents
            || index != self.chapter_index
            || self.chapter_content.source.is_empty()
        {
            self.chapter_content = self.book.load_chapter_content(index)?;
            self.page_ranges = paginate_reader_source(&self.chapter_content.source);
            self.chapter_anchors = self.chapter_content.anchors.clone();
        }
        self.showing_contents = false;
        if self.page_ranges.is_empty() {
            self.page_ranges.push(0..1);
        }
        self.chapter_index = index;
        self.page_count = self.page_ranges.len();
        self.page_counts.insert(index, self.page_count);
        self.page_index = page.min(self.page_count.saturating_sub(1));
        let range = self.page_ranges[self.page_index].clone();
        self.page_start_line = range.start;
        let chapter_title =
            reader_chapter_title(&self.book, index, &self.chapter_content);
        self.book.chapters[index].title = chapter_title.clone();
        let source = reader_page_source(
            &self.chapter_content.source,
            range.clone(),
            self.page_index,
            &chapter_title,
        );
        self.markdown.set_source_for_navigation(&source);
        self.markdown.title = if self.page_index == 0 {
            chapter_title
        } else {
            String::new()
        };
        self.markdown.reader_footer = Some(format!(
            "Page {} of {}",
            self.page_index + 1,
            self.page_count
        ));
        self.markdown.path = self.book.path.clone();
        self.markdown.read_only = true;
        self.markdown.enter_normal();
        let local_line = source_line
            .saturating_sub(range.start)
            .min(range.len().saturating_sub(1));
        self.markdown.jump_to_line(local_line.saturating_add(1));
        let line_len = self
            .markdown
            .lines
            .get(self.markdown.cursor_line)
            .map(String::len)
            .unwrap_or_default();
        self.markdown.cursor_col = source_column.min(line_len);
        self.markdown.restore_scroll_position(0.0);
        self.refresh_reader_highlights();
        self.refresh_chapter_images();
        Ok(())
    }

    pub fn next_chapter(&mut self) -> Result<bool, EpubError> {
        match self.next_reading_chapter(self.chapter_index.saturating_add(1))? {
            Some(index) => self.go_to_chapter(index),
            None => Ok(false),
        }
    }

    pub fn go_to_href(&mut self, href: &str) -> Result<bool, EpubError> {
        let (resource, mut fragment) = split_fragment(href);
        let Some(mut index) = self.book.chapter_index_for_href(resource) else {
            return Ok(false);
        };
        if self.chapter_is_skippable(index)? {
            let Some(next) = self.next_reading_chapter(index.saturating_add(1))? else {
                return Ok(false);
            };
            index = next;
            fragment = None;
        }
        if index != self.chapter_index {
            self.chapter_content = self.book.load_chapter_content(index)?;
            self.page_ranges = paginate_reader_source(&self.chapter_content.source);
            self.chapter_anchors = self.chapter_content.anchors.clone();
        }
        let line = fragment
            .and_then(|fragment| {
                let decoded = percent_decode_fragment(fragment);
                self.chapter_anchors
                    .get(fragment)
                    .or_else(|| self.chapter_anchors.get(&decoded))
                    .copied()
            })
            .unwrap_or(0);
        let page = page_for_source_line(&self.page_ranges, line);
        let changed = self.showing_contents
            || index != self.chapter_index
            || page != self.page_index;
        self.load_chapter_page(index, page, line, 0)?;
        self.state.location.fragment = fragment.map(str::to_string);
        self.state.location.source_line = line;
        self.state.scroll_y = 0.0;
        self.save_state()?;
        Ok(changed || fragment.is_some())
    }

    pub fn previous_chapter(&mut self) -> Result<bool, EpubError> {
        match self.previous_reading_chapter(self.chapter_index)? {
            Some(index) => self.go_to_chapter(index),
            None => Ok(false),
        }
    }

    pub fn capture_location(&mut self) {
        if self.showing_contents {
            return;
        }
        let Some(chapter) = self.book.chapters.get(self.chapter_index) else {
            return;
        };
        self.state.location.chapter_href = chapter.href.clone();
        self.state.location.source_line = self
            .page_start_line
            .saturating_add(self.markdown.cursor_line);
        self.state.location.source_column = self.markdown.cursor_col;
        self.state.scroll_y = self.markdown.scroll_y;
        self.update_progress();
        self.state_dirty = true;
    }

    pub fn save_state(&mut self) -> std::io::Result<()> {
        self.capture_location();
        save_reading_state(&self.state_path, &self.state)?;
        self.state_dirty = false;
        self.last_state_save = web_time::Instant::now();
        Ok(())
    }

    pub fn flush_state_if_due(&mut self) {
        if self.state_dirty
            && self.last_state_save.elapsed() >= std::time::Duration::from_millis(800)
        {
            let _ = self.save_state();
        }
    }

    pub fn state_save_pending(&self) -> bool {
        self.state_dirty
    }

    pub fn add_highlight_from_selection(
        &mut self,
        note: String,
    ) -> std::io::Result<Option<String>> {
        let Some(chapter) = self.book.chapters.get(self.chapter_index) else {
            return Ok(None);
        };
        let Some((start, end, selected_text)) = self.markdown.visual_selection() else {
            return Ok(None);
        };
        if selected_text.trim().is_empty() {
            return Ok(None);
        }
        let start_location = EpubLocation {
            chapter_href: chapter.href.clone(),
            source_line: self.page_start_line.saturating_add(start.line),
            source_column: start.col,
            fragment: None,
        };
        let end_location = EpubLocation {
            chapter_href: chapter.href.clone(),
            source_line: self.page_start_line.saturating_add(end.line),
            source_column: end.col,
            fragment: None,
        };
        let now = unix_time_ms();
        let id = format!("highlight-{now}-{}", self.state.annotations.len());
        if let Some(existing) = self.state.annotations.iter_mut().find(|annotation| {
            annotation.start == start_location && annotation.end == end_location
        }) {
            if !note.is_empty() {
                existing.note = note;
            }
            existing.updated_unix_ms = now;
            let id = existing.id.clone();
            let has_note = !existing.note.trim().is_empty();
            self.markdown.enter_normal();
            self.refresh_reader_highlights();
            self.save_state()?;
            if has_note {
                self.sync_annotation_to_book_note(&id)?;
            }
            return Ok(Some(id));
        }
        self.state.annotations.push(EpubAnnotation {
            id: id.clone(),
            start: start_location,
            end: end_location,
            selected_text,
            note,
            color: default_highlight_color(),
            chapter_title: self
                .book
                .chapters
                .get(self.chapter_index)
                .map(|chapter| chapter.title.clone())
                .unwrap_or_default(),
            page_index: self.page_index,
            created_unix_ms: now,
            updated_unix_ms: now,
            collection_ids: Vec::new(),
        });
        self.markdown.enter_normal();
        self.refresh_reader_highlights();
        self.save_state()?;
        if self
            .state
            .annotations
            .iter()
            .find(|annotation| annotation.id == id)
            .is_some_and(|annotation| !annotation.note.trim().is_empty())
        {
            self.sync_annotation_to_book_note(&id)?;
        }
        Ok(Some(id))
    }

    pub fn set_annotation_note(
        &mut self,
        id: &str,
        note: String,
    ) -> std::io::Result<bool> {
        let Some(annotation) = self
            .state
            .annotations
            .iter_mut()
            .find(|annotation| annotation.id == id)
        else {
            return Ok(false);
        };
        annotation.note = note;
        annotation.updated_unix_ms = unix_time_ms();
        let has_note = !annotation.note.trim().is_empty();
        self.save_state()?;
        if has_note {
            self.sync_annotation_to_book_note(id)?;
        } else {
            self.remove_annotation_from_book_note(id)?;
        }
        self.sync_annotation_collections(id)?;
        Ok(true)
    }

    pub fn set_annotation_color(
        &mut self,
        id: &str,
        color: &str,
    ) -> std::io::Result<bool> {
        let color = normalize_highlight_color(color);
        let Some(annotation) = self
            .state
            .annotations
            .iter_mut()
            .find(|annotation| annotation.id == id)
        else {
            return Ok(false);
        };
        annotation.color = color.to_string();
        annotation.updated_unix_ms = unix_time_ms();
        self.refresh_reader_highlights();
        self.save_state()?;
        Ok(true)
    }

    pub fn annotation_at_source_position(
        &self,
        line: usize,
        column: usize,
    ) -> Option<&EpubAnnotation> {
        let chapter = self.book.chapters.get(self.chapter_index)?;
        let position = EpubLocation {
            chapter_href: chapter.href.clone(),
            source_line: self.page_start_line.saturating_add(line),
            source_column: column,
            fragment: None,
        };
        self.state.annotations.iter().find(|annotation| {
            annotation.start.chapter_href == position.chapter_href
                && location_contains(&annotation.start, &annotation.end, &position)
        })
    }

    pub fn book_note_path(&self) -> Option<PathBuf> {
        resolve_book_note_path(
            &self.vault_root,
            &self.state.book_id,
            self.state.book_note_path.as_deref(),
        )
    }

    pub fn sync_annotation_to_book_note(&mut self, id: &str) -> std::io::Result<PathBuf> {
        let annotation = self
            .state
            .annotations
            .iter()
            .find(|annotation| annotation.id == id)
            .cloned()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "annotation")
            })?;
        let path = self.ensure_book_note()?;
        let source = fs::read_to_string(&path).unwrap_or_default();
        let block = annotation_markdown_block(&self.book, &annotation);
        let next = replace_annotation_block(&source, id, Some(&block));
        let next =
            replace_book_note_toc(&next, &book_note_toc_block(&self.state.annotations));
        atomic_write(&path, next.as_bytes())?;
        Ok(path)
    }

    pub fn annotation_collections(&self, id: &str) -> Vec<(String, String, bool)> {
        let memberships = self
            .state
            .annotations
            .iter()
            .find(|annotation| annotation.id == id)
            .map(|annotation| annotation.collection_ids.as_slice())
            .unwrap_or_default();
        self.state
            .collections
            .iter()
            .map(|collection| {
                (
                    collection.id.clone(),
                    collection.name.clone(),
                    memberships.contains(&collection.id),
                )
            })
            .collect()
    }

    pub fn create_annotation_collection(
        &mut self,
        name: String,
    ) -> std::io::Result<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "collection name cannot be empty",
            ));
        }
        if let Some(existing) = self
            .state
            .collections
            .iter()
            .find(|collection| collection.name.eq_ignore_ascii_case(name))
        {
            return Ok(existing.id.clone());
        }

        let now = unix_time_ms();
        let id = format!("collection-{now}-{}", self.state.collections.len() + 1);
        let file_name = self.allocate_collection_file_name(name, &id)?;
        self.state.collections.push(EpubAnnotationCollection {
            id: id.clone(),
            name: name.to_string(),
            file_name,
            created_unix_ms: now,
            updated_unix_ms: now,
        });
        self.save_state()?;
        self.sync_collection_note(&id)?;
        Ok(id)
    }

    pub fn toggle_annotation_collection(
        &mut self,
        annotation_id: &str,
        collection_id: &str,
    ) -> std::io::Result<bool> {
        if !self
            .state
            .collections
            .iter()
            .any(|collection| collection.id == collection_id)
        {
            return Ok(false);
        }
        let Some(annotation) = self
            .state
            .annotations
            .iter_mut()
            .find(|annotation| annotation.id == annotation_id)
        else {
            return Ok(false);
        };
        if let Some(index) = annotation
            .collection_ids
            .iter()
            .position(|id| id == collection_id)
        {
            annotation.collection_ids.remove(index);
        } else {
            annotation.collection_ids.push(collection_id.to_string());
        }
        annotation.updated_unix_ms = unix_time_ms();
        self.save_state()?;
        self.sync_collection_note(collection_id)?;
        Ok(true)
    }

    pub fn add_annotation_to_collection(
        &mut self,
        annotation_id: &str,
        collection_id: &str,
    ) -> std::io::Result<bool> {
        let Some(annotation) = self
            .state
            .annotations
            .iter_mut()
            .find(|annotation| annotation.id == annotation_id)
        else {
            return Ok(false);
        };
        if !annotation
            .collection_ids
            .iter()
            .any(|id| id == collection_id)
        {
            annotation.collection_ids.push(collection_id.to_string());
            annotation.updated_unix_ms = unix_time_ms();
            self.save_state()?;
        }
        self.sync_collection_note(collection_id)?;
        Ok(true)
    }

    pub fn sync_collection_note(
        &mut self,
        collection_id: &str,
    ) -> std::io::Result<PathBuf> {
        let book_note = self.ensure_book_note()?;
        let Some(collection) = self
            .state
            .collections
            .iter()
            .find(|collection| collection.id == collection_id)
            .cloned()
        else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "annotation collection not found",
            ));
        };
        let path = book_note
            .parent()
            .unwrap_or(self.vault_root.as_path())
            .join(&collection.file_name);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let source = collection_note_source(
            &existing,
            &self.book,
            &collection,
            &self.state.annotations,
        );
        atomic_write(&path, source.as_bytes())?;
        Ok(path)
    }

    fn sync_annotation_collections(
        &mut self,
        annotation_id: &str,
    ) -> std::io::Result<()> {
        let ids = self
            .state
            .annotations
            .iter()
            .find(|annotation| annotation.id == annotation_id)
            .map(|annotation| annotation.collection_ids.clone())
            .unwrap_or_default();
        for id in ids {
            self.sync_collection_note(&id)?;
        }
        Ok(())
    }

    fn allocate_collection_file_name(
        &mut self,
        name: &str,
        collection_id: &str,
    ) -> std::io::Result<String> {
        let book_note = self.ensure_book_note()?;
        let parent = book_note.parent().unwrap_or(self.vault_root.as_path());
        let stem = safe_note_stem(name);
        let book_name = book_note
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let used = self
            .state
            .collections
            .iter()
            .map(|collection| collection.file_name.to_ascii_lowercase())
            .collect::<std::collections::HashSet<_>>();
        let mut candidate = format!("{stem}.md");
        if candidate.eq_ignore_ascii_case(book_name)
            || used.contains(&candidate.to_ascii_lowercase())
            || parent.join(&candidate).exists()
        {
            let suffix = collection_id.trim_start_matches("collection-");
            let suffix = &suffix[..suffix.len().min(8)];
            candidate = format!("{stem} {suffix}.md");
        }
        let mut index = 2;
        while candidate.eq_ignore_ascii_case(book_name)
            || used.contains(&candidate.to_ascii_lowercase())
            || parent.join(&candidate).exists()
        {
            candidate = format!("{stem} {index}.md");
            index += 1;
        }
        Ok(candidate)
    }

    pub fn remove_annotation_from_book_note(
        &self,
        id: &str,
    ) -> std::io::Result<Option<PathBuf>> {
        let Some(path) = self.book_note_path() else {
            return Ok(None);
        };
        let source = fs::read_to_string(&path)?;
        let next = replace_annotation_block(&source, id, None);
        let remaining = self
            .state
            .annotations
            .iter()
            .filter(|annotation| annotation.id != id)
            .cloned()
            .collect::<Vec<_>>();
        let next = replace_book_note_toc(&next, &book_note_toc_block(&remaining));
        if next != source {
            atomic_write(&path, next.as_bytes())?;
        }
        Ok(Some(path))
    }

    fn ensure_book_note(&mut self) -> std::io::Result<PathBuf> {
        if let Some(path) = self.book_note_path() {
            return self.adopt_book_note_path(path);
        }
        let path = canonical_book_note_path(&self.vault_root, &self.book, None);
        fs::create_dir_all(path.parent().unwrap_or(&self.vault_root))?;
        let source = book_note_header(&self.book, &self.state.book_note_id);
        atomic_write(&path, source.as_bytes())?;
        self.remember_book_note_path(&path)?;
        Ok(path)
    }

    /// Move only notes created by the legacy flat `Books/*.md` layout.
    /// Notes a user deliberately moved elsewhere in the vault stay where they
    /// put them and are still rediscovered by `epub_book_id`.
    fn adopt_book_note_path(&mut self, path: PathBuf) -> std::io::Result<PathBuf> {
        let books_dir = self.vault_root.join("Books");
        let path = if path.parent() == Some(books_dir.as_path()) {
            let target =
                canonical_book_note_path(&self.vault_root, &self.book, Some(&path));
            fs::create_dir_all(target.parent().unwrap_or(&books_dir))?;
            fs::rename(&path, &target)?;
            target
        } else {
            path
        };
        self.refresh_book_note_document(&path)?;
        self.remember_book_note_path(&path)?;
        Ok(path)
    }

    fn refresh_book_note_document(&self, path: &Path) -> std::io::Result<()> {
        let source = fs::read_to_string(path)?;
        let mut next = source.clone();
        for annotation in self
            .state
            .annotations
            .iter()
            .filter(|annotation| !annotation.note.trim().is_empty())
        {
            let block = annotation_markdown_block(&self.book, annotation);
            next = replace_annotation_block(&next, &annotation.id, Some(&block));
        }
        next =
            replace_book_note_toc(&next, &book_note_toc_block(&self.state.annotations));
        if next != source {
            atomic_write(path, next.as_bytes())?;
        }
        Ok(())
    }

    fn remember_book_note_path(&mut self, path: &Path) -> std::io::Result<()> {
        self.state.book_note_path = path
            .strip_prefix(&self.vault_root)
            .ok()
            .map(|relative| relative.to_string_lossy().into_owned());
        self.save_state()
    }

    pub fn go_to_annotation(&mut self, id: &str) -> Result<bool, EpubError> {
        let Some(annotation) = self
            .state
            .annotations
            .iter()
            .find(|annotation| annotation.id == id)
            .cloned()
        else {
            return Ok(false);
        };
        let mut href = annotation.start.chapter_href.clone();
        if let Some(fragment) = annotation.start.fragment.as_deref() {
            href.push('#');
            href.push_str(fragment);
        }
        self.go_to_href(&href)?;
        let page = page_for_source_line(&self.page_ranges, annotation.start.source_line);
        self.load_chapter_page(
            self.chapter_index,
            page,
            annotation.start.source_line,
            annotation.start.source_column,
        )?;
        let line_len = self
            .markdown
            .lines
            .get(self.markdown.cursor_line)
            .map(String::len)
            .unwrap_or_default();
        self.markdown.cursor_col = annotation.start.source_column.min(line_len);
        self.capture_location();
        let _ = self.save_state();
        Ok(true)
    }

    pub fn remove_annotation(&mut self, id: &str) -> std::io::Result<bool> {
        let collection_ids = self
            .state
            .annotations
            .iter()
            .find(|annotation| annotation.id == id)
            .map(|annotation| annotation.collection_ids.clone())
            .unwrap_or_default();
        let before = self.state.annotations.len();
        self.state
            .annotations
            .retain(|annotation| annotation.id != id);
        if before == self.state.annotations.len() {
            return Ok(false);
        }
        let _ = self.remove_annotation_from_book_note(id);
        self.refresh_reader_highlights();
        self.save_state()?;
        for collection_id in collection_ids {
            self.sync_collection_note(&collection_id)?;
        }
        Ok(true)
    }

    fn refresh_reader_highlights(&mut self) {
        use neoism_ui::editor::markdown::{
            MarkdownPosition, MarkdownReaderHighlight, MarkdownReaderHighlightColor,
        };

        let Some(chapter) = self.book.chapters.get(self.chapter_index) else {
            self.markdown.reader_highlights.clear();
            return;
        };
        self.markdown.reader_highlights = self
            .state
            .annotations
            .iter()
            .filter(|annotation| {
                annotation.start.chapter_href == chapter.href
                    && annotation.end.source_line >= self.page_start_line
                    && annotation.start.source_line
                        < self
                            .page_start_line
                            .saturating_add(self.markdown.lines.len())
            })
            .map(|annotation| MarkdownReaderHighlight {
                start: MarkdownPosition {
                    line: annotation
                        .start
                        .source_line
                        .saturating_sub(self.page_start_line),
                    col: annotation.start.source_column,
                },
                end: MarkdownPosition {
                    line: annotation
                        .end
                        .source_line
                        .saturating_sub(self.page_start_line)
                        .min(self.markdown.lines.len().saturating_sub(1)),
                    col: annotation.end.source_column,
                },
                color: match normalize_highlight_color(&annotation.color) {
                    "green" => MarkdownReaderHighlightColor::Green,
                    "blue" => MarkdownReaderHighlightColor::Blue,
                    "pink" => MarkdownReaderHighlightColor::Pink,
                    "purple" => MarkdownReaderHighlightColor::Purple,
                    _ => MarkdownReaderHighlightColor::Yellow,
                },
            })
            .collect();
    }

    fn refresh_chapter_images(&mut self) {
        self.rendered_images.clear();
        let resources = self
            .markdown
            .lines
            .iter()
            .enumerate()
            .filter_map(|(line, source)| epub_image_href(source).map(|href| (line, href)))
            .collect::<Vec<_>>();
        for (line, href) in resources {
            let Ok(bytes) = self.book.load_resource(&href, MAX_IMAGE_BYTES) else {
                continue;
            };
            let Ok(image) = image_rs::load_from_memory(&bytes) else {
                continue;
            };
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            if width == 0 || height == 0 {
                continue;
            }
            self.rendered_images.push(EpubRenderedImage {
                line,
                image_id: epub_image_id(&self.book.path, &href),
                width,
                height,
                pixels: rgba.into_raw(),
            });
        }
        self.markdown.set_notebook_image_preview_dimensions(
            self.rendered_images
                .iter()
                .map(|image| (image.line, image.width, image.height)),
        );
    }

    fn update_progress(&mut self) {
        let chapters = self.book.chapters.len().max(1) as f32;
        let page_count = self.page_count.max(1) as f32;
        let within_page = if self.markdown.lines.is_empty() {
            0.0
        } else {
            self.markdown.cursor_line as f32 / self.markdown.lines.len() as f32
        };
        let within = (self.page_index as f32 + within_page) / page_count;
        self.state.progress =
            ((self.chapter_index as f32 + within) / chapters).clamp(0.0, 1.0);
    }
}

impl Drop for EpubPane {
    fn drop(&mut self) {
        let _ = self.save_state();
    }
}

struct PreparedEpubPage {
    markdown: MarkdownPane,
    content: EpubChapterContent,
    page_ranges: Vec<Range<usize>>,
    page_index: usize,
    page_start_line: usize,
}

fn prepare_chapter_page(
    book: &EpubBook,
    chapter_index: usize,
    state: &EpubReadingState,
) -> Result<PreparedEpubPage, EpubError> {
    let content = book.load_chapter_content(chapter_index)?;
    let page_ranges = paginate_reader_source(&content.source);
    let page_index = page_for_source_line(&page_ranges, state.location.source_line);
    let range = page_ranges.get(page_index).cloned().unwrap_or(0..1);
    let page_start_line = range.start;
    let chapter_title = reader_chapter_title(book, chapter_index, &content);
    let source =
        reader_page_source(&content.source, range.clone(), page_index, &chapter_title);
    let mut markdown = MarkdownPane::from_source(book.path.clone(), &source);
    markdown.title = if page_index == 0 {
        chapter_title
    } else {
        String::new()
    };
    markdown.reader_footer = Some(format!(
        "Page {} of {}",
        page_index + 1,
        page_ranges.len().max(1)
    ));
    markdown.read_only = true;
    markdown.enter_normal();
    let local_line = state
        .location
        .source_line
        .saturating_sub(page_start_line)
        .min(range.len().saturating_sub(1));
    markdown.jump_to_line(local_line.saturating_add(1));
    let line_len = markdown
        .lines
        .get(markdown.cursor_line)
        .map(String::len)
        .unwrap_or_default();
    markdown.cursor_col = state.location.source_column.min(line_len);
    markdown.restore_scroll_position(state.scroll_y);
    Ok(PreparedEpubPage {
        markdown,
        content,
        page_ranges,
        page_index,
        page_start_line,
    })
}

fn reader_chapter_title(
    book: &EpubBook,
    chapter_index: usize,
    content: &EpubChapterContent,
) -> String {
    let assigned = book
        .chapters
        .get(chapter_index)
        .map(|chapter| chapter.title.trim())
        .filter(|title| !title.is_empty())
        .unwrap_or(book.metadata.title.as_str());
    if !assigned.starts_with("Chapter ") {
        return assigned.to_string();
    }

    let first_text = content
        .source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && epub_image_href(line).is_none())
        .map(|line| {
            line.trim_start_matches('#')
                .trim()
                .replace(['*', '_', '`'], "")
        })
        .filter(|line| !line.is_empty() && line.chars().count() <= 80);
    first_text.unwrap_or_else(|| {
        if chapter_index == 0 {
            book.metadata.title.clone()
        } else {
            assigned.to_string()
        }
    })
}

fn first_toc_chapter_index(book: &EpubBook, entries: &[EpubTocEntry]) -> Option<usize> {
    entries
        .iter()
        .flat_map(|entry| {
            std::iter::once(book.chapter_index_for_href(&entry.href)).chain(
                std::iter::once(first_toc_chapter_index(book, &entry.children)),
            )
        })
        .flatten()
        .min()
}

fn reader_chapter_is_skippable(
    book: &EpubBook,
    index: usize,
    contents_insertion_index: usize,
) -> Result<bool, EpubError> {
    let content = book.load_chapter_content(index)?;
    let title = reader_chapter_title(book, index, &content);
    Ok(reader_content_is_skippable_frontmatter(
        &content,
        &title,
        index < contents_insertion_index,
    ))
}

/// A saved location can point at a title-only or boilerplate spine item after
/// an EPUB is updated (or after Neoism learns to filter a publisher footer).
/// Restore to the next real reading item when possible, otherwise the nearest
/// preceding one, so reopening never strands the reader on a blank page.
fn restored_reading_chapter_index(book: &EpubBook, restored: usize) -> usize {
    let restored = restored.min(book.chapters.len().saturating_sub(1));
    let insertion = first_toc_chapter_index(book, &book.toc).unwrap_or(0);
    if !reader_chapter_is_skippable(book, restored, insertion).unwrap_or(false) {
        return restored;
    }
    ((restored + 1)..book.chapters.len())
        .find(|index| {
            !reader_chapter_is_skippable(book, *index, insertion).unwrap_or(false)
        })
        .or_else(|| {
            (0..restored).rev().find(|index| {
                !reader_chapter_is_skippable(book, *index, insertion).unwrap_or(false)
            })
        })
        .unwrap_or(restored)
}

fn reader_content_is_skippable_frontmatter(
    content: &EpubChapterContent,
    title: &str,
    before_contents: bool,
) -> bool {
    let lines = content
        .source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return true;
    }
    if lines.iter().any(|line| epub_image_href(line).is_some()) {
        return false;
    }
    let cleaned = |line: &str| {
        line.trim_start_matches('#')
            .trim()
            .replace(['*', '_', '`'], "")
    };
    if lines.len() == 1 && cleaned(lines[0]).eq_ignore_ascii_case(title.trim()) {
        return true;
    }
    before_contents
        && lines.len() <= 3
        && lines.iter().map(|line| line.chars().count()).sum::<usize>() <= 160
}

/// Reflow converted chapter source into small, bounded reader pages. Page
/// boundaries prefer paragraph breaks and estimate wrapped visual rows; only
/// the active page is mounted into Sugarloaf.
fn paginate_reader_source(source: &str) -> Vec<Range<usize>> {
    // A full-height desktop reader comfortably holds roughly thirty wrapped
    // body rows. Staying a little above that target fills the page without
    // turning a chapter into one giant Sugarloaf surface; unusually large
    // text/images can still use the reader's normal intra-page scroll.
    const TARGET_VISUAL_ROWS: usize = 32;
    let lines = source.split('\n').collect::<Vec<_>>();
    if lines.is_empty() {
        return vec![0..1];
    }

    let mut blocks = Vec::<(Range<usize>, usize)>::new();
    let mut cursor = 0usize;
    while cursor < lines.len() {
        let block_start = cursor;
        let mut block_rows = 0usize;
        while cursor < lines.len() {
            let line = lines[cursor];
            block_rows = block_rows.saturating_add(estimated_reader_rows(line));
            cursor += 1;
            if line.trim().is_empty() && cursor > block_start + 1 {
                break;
            }
        }

        blocks.push((block_start..cursor, block_rows));
    }

    // Keep a heading with the paragraph it introduces. A standalone spine
    // divider ("Part One", for example) remains an intentionally quiet page.
    let mut grouped = Vec::<(Range<usize>, usize)>::new();
    let mut index = 0usize;
    while index < blocks.len() {
        let (mut range, mut rows) = blocks[index].clone();
        if index + 1 < blocks.len() && reader_block_is_heading(&lines, &range) {
            range.end = blocks[index + 1].0.end;
            rows = rows.saturating_add(blocks[index + 1].1);
            index += 1;
        }
        grouped.push((range, rows));
        index += 1;
    }

    let total_rows = grouped.iter().map(|(_, rows)| *rows).sum::<usize>().max(1);
    let desired_pages = total_rows
        .div_ceil(TARGET_VISUAL_ROWS)
        .max(1)
        .min(grouped.len().max(1));
    let mut pages = Vec::with_capacity(desired_pages);
    let mut block = 0usize;
    let mut remaining_rows = total_rows;
    while block < grouped.len() {
        let remaining_pages = desired_pages.saturating_sub(pages.len()).max(1);
        let page_start = grouped[block].0.start;
        let mut page_end = page_start;
        let mut page_rows = 0usize;
        while block < grouped.len() {
            let blocks_after = grouped.len().saturating_sub(block + 1);
            if page_rows > 0 && blocks_after < remaining_pages.saturating_sub(1) {
                break;
            }
            let target = remaining_rows as f32 / remaining_pages as f32;
            let candidate = page_rows.saturating_add(grouped[block].1);
            if page_rows > 0
                && (page_rows as f32 - target).abs() <= (candidate as f32 - target).abs()
            {
                break;
            }
            page_rows = candidate;
            page_end = grouped[block].0.end;
            block += 1;
        }
        // A single oversized paragraph is still a valid semantic page; EPUB
        // markup stays intact instead of being torn through an emphasis/link.
        if page_end == page_start {
            page_rows = grouped[block].1;
            page_end = grouped[block].0.end;
            block += 1;
        }
        pages.push(page_start..page_end);
        remaining_rows = remaining_rows.saturating_sub(page_rows);
    }
    pages
}

fn reader_block_is_heading(lines: &[&str], range: &Range<usize>) -> bool {
    lines[range.clone()]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .all(|line| line.trim_start().starts_with('#'))
}

fn estimated_reader_rows(line: &str) -> usize {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return 1;
    }
    if epub_image_href(trimmed).is_some() {
        return 24;
    }
    // The reader body is capped near 920 logical pixels. Its normal font
    // lands close to 90–100 average characters per rendered row; the old
    // 72-character estimate under-filled wide desktop pages dramatically.
    // Narrow panes still get the ordinary intra-page Sugarloaf scroll.
    let wrap_rows = trimmed.chars().count().div_ceil(96).max(1);
    if trimmed.starts_with('#') {
        wrap_rows.saturating_add(2)
    } else if trimmed.starts_with("```") {
        wrap_rows.saturating_add(1)
    } else {
        wrap_rows
    }
}

fn page_for_source_line(ranges: &[Range<usize>], line: usize) -> usize {
    ranges
        .iter()
        .position(|range| range.contains(&line))
        .unwrap_or_else(|| ranges.len().saturating_sub(1))
}

fn page_source(source: &str, range: Range<usize>) -> String {
    source
        .split('\n')
        .skip(range.start)
        .take(range.len())
        .collect::<Vec<_>>()
        .join("\n")
}

fn reader_page_source(
    source: &str,
    range: Range<usize>,
    page_index: usize,
    chapter_title: &str,
) -> String {
    let page = page_source(source, range);
    if page_index != 0 {
        return page;
    }
    let mut lines = page.split('\n').map(str::to_string).collect::<Vec<_>>();
    let Some(first) = lines.iter_mut().find(|line| !line.trim().is_empty()) else {
        return page;
    };
    let heading = first
        .trim_start()
        .trim_start_matches('#')
        .trim()
        .replace(['*', '_', '`'], "");
    if heading.eq_ignore_ascii_case(chapter_title.trim()) {
        first.clear();
        lines.join("\n")
    } else {
        page
    }
}

fn contents_reader_source(entries: &[EpubTocEntry]) -> String {
    fn append(entries: &[EpubTocEntry], depth: usize, output: &mut String) {
        for entry in entries {
            let label = escape_wiki_label(entry.label.trim());
            if label.is_empty() {
                continue;
            }
            output.push_str(&"  ".repeat(depth));
            output.push_str("- [[neoism-epub://");
            output.push_str(&entry.href);
            output.push('|');
            output.push_str(&label);
            output.push_str("]]\n");
            append(&entry.children, depth.saturating_add(1), output);
        }
    }

    let mut output = String::new();
    append(entries, 0, &mut output);
    output.trim().to_string()
}

fn page_preview(source: &str) -> String {
    let line = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && *line != "```")
        .unwrap_or("Page");
    if epub_image_href(line).is_some() {
        return "Illustration".to_string();
    }
    let cleaned = line
        .trim_start_matches(['#', '>', '-', '*', '`', ' '])
        .replace("**", "")
        .replace(['[', ']', '`'], "");
    let words = cleaned.split_whitespace().collect::<Vec<_>>();
    if words.len() <= 9 {
        words.join(" ")
    } else {
        format!("{}…", words[..9].join(" "))
    }
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

fn epub_image_href(line: &str) -> Option<String> {
    let marker = "(neoism-epub-resource://";
    let start = line.find(marker)? + marker.len();
    let end = line[start..].find(')')? + start;
    let href = line[start..end].trim();
    (!href.is_empty()).then(|| href.to_string())
}

fn epub_image_id(book: &Path, href: &str) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for byte in book
        .to_string_lossy()
        .as_bytes()
        .iter()
        .chain(href.as_bytes())
    {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash | (1 << 29)
}

fn parse_metadata(document: &Document<'_>) -> EpubMetadata {
    let text = |name: &str| {
        document
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == name)
            .and_then(|node| node.text())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let creators = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "creator")
        .filter_map(|node| node.text())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    EpubMetadata {
        title: text("title").unwrap_or_else(|| "Untitled book".to_string()),
        creators,
        language: text("language"),
        identifier: text("identifier"),
    }
}

fn parse_manifest(
    document: &Document<'_>,
    package_dir: &str,
) -> Result<HashMap<String, EpubManifestItem>, EpubError> {
    let mut manifest = HashMap::new();
    for node in document
        .descendants()
        .filter(|node| node.has_tag_name("item"))
    {
        let (Some(id), Some(href), Some(media_type)) = (
            node.attribute("id"),
            node.attribute("href"),
            node.attribute("media-type"),
        ) else {
            continue;
        };
        let href = normalize_archive_path(package_dir, href)?;
        let properties = node
            .attribute("properties")
            .unwrap_or_default()
            .split_ascii_whitespace()
            .map(str::to_string)
            .collect();
        manifest.insert(
            id.to_string(),
            EpubManifestItem {
                id: id.to_string(),
                href,
                media_type: media_type.to_string(),
                properties,
            },
        );
    }
    Ok(manifest)
}

fn parse_spine(
    document: &Document<'_>,
    manifest: &HashMap<String, EpubManifestItem>,
) -> Result<Vec<EpubChapter>, EpubError> {
    let mut chapters = Vec::new();
    for node in document
        .descendants()
        .filter(|node| node.has_tag_name("itemref"))
    {
        let Some(idref) = node.attribute("idref") else {
            continue;
        };
        let Some(item) = manifest.get(idref) else {
            continue;
        };
        if !matches!(
            item.media_type.as_str(),
            "application/xhtml+xml" | "text/html"
        ) {
            continue;
        }
        chapters.push(EpubChapter {
            idref: idref.to_string(),
            href: item.href.clone(),
            title: String::new(),
            linear: node.attribute("linear") != Some("no"),
        });
    }
    Ok(chapters)
}

fn parse_toc<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    package: &Document<'_>,
    manifest: &HashMap<String, EpubManifestItem>,
) -> Result<Vec<EpubTocEntry>, EpubError> {
    if let Some(nav) = manifest
        .values()
        .find(|item| item.properties.contains("nav"))
    {
        let xml = read_zip_text(archive, &nav.href, MAX_XML_BYTES)?;
        let xml = epub_xml_for_parse(&xml);
        let document = Document::parse(&xml)?;
        let nav_node = document.descendants().find(|node| {
            node.has_tag_name("nav")
                && (node.attribute(("http://www.idpf.org/2007/ops", "type"))
                    == Some("toc")
                    || node.attribute("type") == Some("toc"))
        });
        if let Some(nav_node) = nav_node {
            return Ok(parse_nav_list(nav_node, archive_parent(&nav.href)));
        }
    }

    let ncx_id = package
        .descendants()
        .find(|node| node.has_tag_name("spine"))
        .and_then(|node| node.attribute("toc"));
    let ncx = ncx_id.and_then(|id| manifest.get(id)).or_else(|| {
        manifest
            .values()
            .find(|item| item.media_type == "application/x-dtbncx+xml")
    });
    let Some(ncx) = ncx else {
        return Ok(Vec::new());
    };
    let xml = read_zip_text(archive, &ncx.href, MAX_XML_BYTES)?;
    let xml = epub_xml_for_parse(&xml);
    let document = Document::parse(&xml)?;
    Ok(document
        .descendants()
        .find(|node| node.has_tag_name("navMap"))
        .map(|root| parse_ncx_points(root, archive_parent(&ncx.href)))
        .unwrap_or_default())
}

fn parse_nav_list(nav: Node<'_, '_>, base: &str) -> Vec<EpubTocEntry> {
    let Some(list) = nav.children().find(|node| node.has_tag_name("ol")) else {
        return Vec::new();
    };
    parse_nav_items(list, base)
}

fn parse_nav_items(list: Node<'_, '_>, base: &str) -> Vec<EpubTocEntry> {
    list.children()
        .filter(|node| node.has_tag_name("li"))
        .filter_map(|item| {
            let anchor = item.descendants().find(|node| node.has_tag_name("a"))?;
            let href =
                normalize_href_with_fragment(base, anchor.attribute("href")?).ok()?;
            let children = item
                .children()
                .find(|node| node.has_tag_name("ol"))
                .map(|list| parse_nav_items(list, base))
                .unwrap_or_default();
            Some(EpubTocEntry {
                label: normalized_node_text(anchor),
                href,
                children,
            })
        })
        .collect()
}

fn collect_toc_titles(entries: &[EpubTocEntry], titles: &mut HashMap<String, String>) {
    for entry in entries {
        let (href, _) = split_fragment(&entry.href);
        titles
            .entry(href.to_string())
            .or_insert_with(|| entry.label.clone());
        collect_toc_titles(&entry.children, titles);
    }
}

fn parse_ncx_points(root: Node<'_, '_>, base: &str) -> Vec<EpubTocEntry> {
    root.children()
        .filter(|node| node.has_tag_name("navPoint"))
        .filter_map(|point| {
            let content = point.children().find(|node| node.has_tag_name("content"))?;
            let href =
                normalize_href_with_fragment(base, content.attribute("src")?).ok()?;
            let label = point
                .children()
                .find(|node| node.has_tag_name("navLabel"))
                .map(normalized_node_text)
                .unwrap_or_default();
            Some(EpubTocEntry {
                label,
                href,
                children: parse_ncx_points(point, base),
            })
        })
        .collect()
}

fn xhtml_to_markdown(xhtml: &str, base: &str) -> String {
    xhtml_to_reader_content(xhtml, base).source
}

fn xhtml_to_reader_content(xhtml: &str, base: &str) -> EpubChapterContent {
    // EPUB2 XHTML and NCX documents routinely carry an external DOCTYPE.
    // roxmltree intentionally rejects DTDs, and a reader must never fetch or
    // expand external entities from a book anyway. Remove only the declaration
    // before parsing; predefined/numeric XML entities continue to work while
    // external entity expansion remains impossible.
    let xhtml_without_doctype = epub_xml_for_parse(xhtml);
    let Ok(document) = Document::parse(&xhtml_without_doctype) else {
        return EpubChapterContent {
            source: strip_xml_fallback(xhtml),
            anchors: HashMap::new(),
        };
    };
    let root = document
        .descendants()
        .find(|node| node.has_tag_name("body"))
        .unwrap_or_else(|| document.root_element());
    let mut output = String::new();
    let mut raw_anchors = HashMap::new();
    for child in root.children() {
        render_xhtml_node(child, base, &mut output, &mut raw_anchors, 0, false);
    }
    let (source, raw_to_normalized) = normalize_reader_source_with_map(&output);
    let anchors = raw_anchors
        .into_iter()
        .map(|(id, raw_line)| {
            let line = raw_to_normalized.get(raw_line).copied().unwrap_or_default();
            (id, line)
        })
        .collect();
    EpubChapterContent { source, anchors }
}

/// Return XML with its optional DOCTYPE declaration removed.
///
/// The scanner understands quoted `>` characters and internal subsets, so it
/// does not truncate the document at the first angle bracket inside a public
/// identifier or entity declaration. No DTD content is retained or evaluated.
fn xml_without_doctype(source: &str) -> Cow<'_, str> {
    let bytes = source.as_bytes();
    let Some(start) = bytes
        .windows(b"<!DOCTYPE".len())
        .position(|window| window.eq_ignore_ascii_case(b"<!DOCTYPE"))
    else {
        return Cow::Borrowed(source);
    };

    let mut quote = None;
    let mut subset_depth = 0usize;
    let mut end = None;
    for (offset, byte) in bytes[start + b"<!DOCTYPE".len()..]
        .iter()
        .copied()
        .enumerate()
    {
        match (quote, byte) {
            (Some(active), value) if value == active => quote = None,
            (Some(_), _) => {}
            (None, b'\'' | b'"') => quote = Some(byte),
            (None, b'[') => subset_depth = subset_depth.saturating_add(1),
            (None, b']') => subset_depth = subset_depth.saturating_sub(1),
            (None, b'>') if subset_depth == 0 => {
                end = Some(start + b"<!DOCTYPE".len() + offset + 1);
                break;
            }
            _ => {}
        }
    }
    let Some(end) = end else {
        // Preserve the original parse error for a malformed/truncated
        // declaration instead of silently deleting the rest of the book.
        return Cow::Borrowed(source);
    };

    let mut sanitized = String::with_capacity(source.len());
    sanitized.push_str(&source[..start]);
    sanitized.push_str(&source[end..]);
    Cow::Owned(sanitized)
}

/// Normalize HTML named entities used by EPUB2 XHTML DTDs without evaluating
/// the DTD or touching the network. XML's five built-ins remain references;
/// common HTML names become Unicode, and unknown names are preserved as
/// literal text instead of making the whole chapter fall back to one line.
fn epub_xml_for_parse(source: &str) -> Cow<'_, str> {
    let without_doctype = xml_without_doctype(source);
    let value = without_doctype.as_ref();
    let bytes = value.as_bytes();
    let mut cursor = 0usize;
    let mut last_copied = 0usize;
    let mut output: Option<String> = None;

    while cursor < bytes.len() {
        if bytes[cursor] != b'&' {
            cursor += 1;
            continue;
        }
        let Some(relative_end) = bytes[cursor + 1..]
            .iter()
            .take(40)
            .position(|byte| *byte == b';')
        else {
            cursor += 1;
            continue;
        };
        let end = cursor + 1 + relative_end;
        let name = &value[cursor + 1..end];
        if name.starts_with('#') || matches!(name, "amp" | "lt" | "gt" | "quot" | "apos")
        {
            cursor = end + 1;
            continue;
        }
        if !name.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            cursor += 1;
            continue;
        }

        let output = output.get_or_insert_with(|| String::with_capacity(value.len()));
        output.push_str(&value[last_copied..cursor]);
        if let Some(replacement) = html_named_entity(name) {
            output.push_str(replacement);
        } else {
            output.push_str("&amp;");
            output.push_str(name);
            output.push(';');
        }
        last_copied = end + 1;
        cursor = end + 1;
    }

    if let Some(mut output) = output {
        output.push_str(&value[last_copied..]);
        Cow::Owned(output)
    } else {
        without_doctype
    }
}

fn html_named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "nbsp" => "\u{00a0}",
        "ensp" => "\u{2002}",
        "emsp" => "\u{2003}",
        "thinsp" => "\u{2009}",
        "ndash" => "–",
        "mdash" => "—",
        "hellip" => "…",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "laquo" => "«",
        "raquo" => "»",
        "bull" => "•",
        "middot" => "·",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "euro" => "€",
        "pound" => "£",
        "yen" => "¥",
        "cent" => "¢",
        "times" => "×",
        "divide" => "÷",
        "deg" => "°",
        "plusmn" => "±",
        "micro" => "µ",
        "para" => "¶",
        "sect" => "§",
        "larr" => "←",
        "uarr" => "↑",
        "rarr" => "→",
        "darr" => "↓",
        "harr" => "↔",
        _ => return None,
    })
}

fn render_xhtml_node(
    node: Node<'_, '_>,
    base: &str,
    output: &mut String,
    anchors: &mut HashMap<String, usize>,
    list_depth: usize,
    in_pre: bool,
) {
    if node.is_text() {
        let text = node.text().unwrap_or_default();
        if in_pre {
            output.push_str(text);
        } else {
            push_collapsed_text(output, text);
        }
        return;
    }
    if !node.is_element() {
        return;
    }
    let name = node.tag_name().name().to_ascii_lowercase();
    if element_is_reader_boilerplate(node, &name) {
        return;
    }
    let block_anchor = matches!(
        name.as_str(),
        "h1" | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "p"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "aside"
            | "figure"
            | "figcaption"
            | "div"
            | "blockquote"
            | "ul"
            | "ol"
            | "pre"
    );
    if block_anchor && element_anchor_id(node).is_some() {
        ensure_blank_line(output);
    }
    if let Some(id) = element_anchor_id(node) {
        anchors
            .entry(id.to_string())
            .or_insert_with(|| output.bytes().filter(|byte| *byte == b'\n').count());
    }
    match name.as_str() {
        "script" | "style" | "head" | "svg" => return,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            ensure_blank_line(output);
            let level = name[1..].parse::<usize>().unwrap_or(1).clamp(1, 6);
            output.push_str(&"#".repeat(level));
            output.push(' ');
            render_children(node, base, output, anchors, list_depth, false);
            ensure_blank_line(output);
            return;
        }
        "p" | "section" | "article" | "header" | "footer" | "aside" | "figure"
        | "figcaption" | "div" => {
            ensure_blank_line(output);
            render_children(node, base, output, anchors, list_depth, false);
            ensure_blank_line(output);
            return;
        }
        "br" => output.push_str("  \n"),
        "hr" => {
            ensure_blank_line(output);
            output.push_str("---");
            ensure_blank_line(output);
        }
        "strong" | "b" => {
            output.push_str("**");
            render_children(node, base, output, anchors, list_depth, in_pre);
            output.push_str("**");
        }
        "em" | "i" => {
            output.push('*');
            render_children(node, base, output, anchors, list_depth, in_pre);
            output.push('*');
        }
        "code" if !in_pre => {
            output.push('`');
            render_children(node, base, output, anchors, list_depth, false);
            output.push('`');
        }
        "pre" => {
            ensure_blank_line(output);
            output.push_str("```\n");
            render_children(node, base, output, anchors, list_depth, true);
            if !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str("```");
            ensure_blank_line(output);
        }
        "blockquote" => {
            let mut inner = String::new();
            let mut inner_anchors = HashMap::new();
            render_children(
                node,
                base,
                &mut inner,
                &mut inner_anchors,
                list_depth,
                false,
            );
            ensure_blank_line(output);
            for line in normalize_reader_source(&inner).lines() {
                output.push_str("> ");
                output.push_str(line);
                output.push('\n');
            }
            ensure_blank_line(output);
        }
        "ul" | "ol" => {
            ensure_blank_line(output);
            let ordered = name == "ol";
            let mut item_index = 1usize;
            for child in node.children().filter(|child| child.has_tag_name("li")) {
                output.push_str(&"  ".repeat(list_depth));
                if ordered {
                    output.push_str(&format!("{item_index}. "));
                    item_index += 1;
                } else {
                    output.push_str("- ");
                }
                render_children(child, base, output, anchors, list_depth + 1, false);
                output.push('\n');
            }
            ensure_blank_line(output);
        }
        "li" => render_children(node, base, output, anchors, list_depth, in_pre),
        "a" => {
            let label = normalized_node_text(node);
            if let Some(href) = node.attribute("href") {
                if href.starts_with("http://") || href.starts_with("https://") {
                    output.push_str(&format!("[[{href}|{}]]", escape_wiki_label(&label)));
                } else if let Ok(href) = normalize_href_with_fragment(base, href) {
                    output.push_str(&format!(
                        "[[neoism-epub://{}|{}]]",
                        href,
                        escape_wiki_label(&label)
                    ));
                } else {
                    output.push_str(&label);
                }
            } else {
                output.push_str(&label);
            }
        }
        "img" => {
            let alt = node.attribute("alt").unwrap_or("Illustration");
            if let Some(src) = node.attribute("src") {
                if let Ok(src) = normalize_href_with_fragment(base, src) {
                    ensure_blank_line(output);
                    output.push_str(&format!("![{}](neoism-epub-resource://{src})", alt));
                    ensure_blank_line(output);
                }
            }
        }
        _ => render_children(node, base, output, anchors, list_depth, in_pre),
    }
}

fn element_is_reader_boilerplate(node: Node<'_, '_>, name: &str) -> bool {
    let id = node
        .attribute("id")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let class = node
        .attribute("class")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let class_is_gutenberg = class.split_ascii_whitespace().any(|token| {
        matches!(
            token,
            "pg-boilerplate" | "pgheader" | "pgfooter" | "pg-footer"
        )
    });
    let id_is_gutenberg = matches!(
        id.as_str(),
        "pg-header"
            | "pg-footer"
            | "pg-end-separator"
            | "project-gutenberg-license"
            | "pg-footer-heading"
    );
    if class_is_gutenberg || id_is_gutenberg {
        return true;
    }

    // Older Gutenberg exports sometimes omit the canonical pg-* classes and
    // leave only the sentinel line. Match the full sentinel shape, not an
    // ordinary mention of Project Gutenberg in a book's prose.
    if matches!(name, "header" | "footer" | "div" | "p" | "span") {
        let text = normalized_node_text(node);
        let marker = text.trim().to_ascii_uppercase();
        return marker.starts_with("*** START OF THE PROJECT GUTENBERG EBOOK")
            || marker.starts_with("*** END OF THE PROJECT GUTENBERG EBOOK");
    }
    false
}

fn render_children(
    node: Node<'_, '_>,
    base: &str,
    output: &mut String,
    anchors: &mut HashMap<String, usize>,
    list_depth: usize,
    in_pre: bool,
) {
    for child in node.children() {
        render_xhtml_node(child, base, output, anchors, list_depth, in_pre);
    }
}

fn element_anchor_id<'a>(node: Node<'a, 'a>) -> Option<&'a str> {
    node.attribute("id")
        .or_else(|| node.attribute(("http://www.w3.org/XML/1998/namespace", "id")))
}

fn normalized_node_text(node: Node<'_, '_>) -> String {
    let mut text = String::new();
    for descendant in node.descendants().filter(|node| node.is_text()) {
        push_collapsed_text(&mut text, descendant.text().unwrap_or_default());
    }
    text.trim().to_string()
}

fn push_collapsed_text(output: &mut String, text: &str) {
    if text.chars().next().is_some_and(char::is_whitespace)
        && !output.is_empty()
        && !output.ends_with([' ', '\n'])
    {
        output.push(' ');
    }
    for word in text.split_whitespace() {
        if !output.is_empty()
            && !output.ends_with([' ', '\n', '(', '[', '*', '`'])
            && !matches!(
                word.chars().next(),
                Some('.' | ',' | ';' | ':' | '!' | '?' | ')')
            )
        {
            output.push(' ');
        }
        output.push_str(word);
    }
    if text.chars().next_back().is_some_and(char::is_whitespace)
        && !output.is_empty()
        && !output.ends_with([' ', '\n'])
    {
        output.push(' ');
    }
}

fn ensure_blank_line(output: &mut String) {
    while output.ends_with(' ') {
        output.pop();
    }
    if output.is_empty() {
        return;
    }
    if !output.ends_with('\n') {
        output.push('\n');
    }
    if !output.ends_with("\n\n") {
        output.push('\n');
    }
}

fn normalize_reader_source(source: &str) -> String {
    normalize_reader_source_with_map(source).0
}

fn normalize_reader_source_with_map(source: &str) -> (String, Vec<usize>) {
    let mut output = String::new();
    let mut blank = false;
    let raw_lines = source.split('\n').collect::<Vec<_>>();
    let mut raw_to_normalized = vec![None; raw_lines.len()];
    let mut output_line = 0usize;
    for (raw_line, line) in raw_lines.iter().enumerate() {
        let line = line.trim_end();
        if line.is_empty() {
            if !blank && !output.is_empty() {
                output.push('\n');
                output_line = output_line.saturating_add(1);
                blank = true;
            }
        } else {
            raw_to_normalized[raw_line] = Some(output_line);
            output.push_str(line);
            output.push('\n');
            output_line = output_line.saturating_add(1);
            blank = false;
        }
    }
    let mut next = None;
    for line in (0..raw_to_normalized.len()).rev() {
        if let Some(mapped) = raw_to_normalized[line] {
            next = Some(mapped);
        } else if let Some(mapped) = next {
            raw_to_normalized[line] = Some(mapped);
        }
    }
    let mut previous = 0usize;
    let raw_to_normalized = raw_to_normalized
        .into_iter()
        .map(|mapped| {
            if let Some(mapped) = mapped {
                previous = mapped;
            }
            mapped.unwrap_or(previous)
        })
        .collect();
    (output.trim().to_string(), raw_to_normalized)
}

fn percent_decode_fragment(fragment: &str) -> String {
    let mut output = Vec::with_capacity(fragment.len());
    let bytes = fragment.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hi = (bytes[index + 1] as char).to_digit(16);
            let lo = (bytes[index + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                output.push((hi * 16 + lo) as u8);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn strip_xml_fallback(source: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    for ch in source.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                output.push(' ');
            }
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn escape_wiki_label(label: &str) -> String {
    label.replace('|', "\\|").replace("]]", "]\\]")
}

fn read_zip_text<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    path: &str,
    max_bytes: u64,
) -> Result<String, EpubError> {
    let safe_path = normalize_archive_path("", path)?;
    let mut entry = archive.by_name(&safe_path)?;
    if entry.size() > max_bytes {
        return Err(EpubError::TooLarge(safe_path));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.by_ref().take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(EpubError::TooLarge(safe_path));
    }
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes.drain(..3);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn archive_parent(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
}

fn split_fragment(href: &str) -> (&str, Option<&str>) {
    href.split_once('#')
        .map(|(path, fragment)| (path, Some(fragment)))
        .unwrap_or((href, None))
}

fn normalize_href_with_fragment(base: &str, href: &str) -> Result<String, EpubError> {
    let (path, fragment) = split_fragment(href);
    let normalized = if path.is_empty() {
        normalize_archive_path("", base)?
    } else {
        normalize_archive_path(base, path)?
    };
    Ok(match fragment.filter(|fragment| !fragment.is_empty()) {
        Some(fragment) => format!("{normalized}#{fragment}"),
        None => normalized,
    })
}

fn normalize_archive_path(base: &str, value: &str) -> Result<String, EpubError> {
    let value = value.replace('\\', "/");
    if value.starts_with('/') {
        return Err(EpubError::UnsafePath(value));
    }
    let joined = if base.is_empty() {
        PathBuf::from(&value)
    } else {
        Path::new(base).join(&value)
    };
    let mut parts = Vec::new();
    for component in joined.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(EpubError::UnsafePath(value));
                }
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(EpubError::UnsafePath(value));
            }
        }
    }
    if parts.is_empty() {
        return Err(EpubError::UnsafePath(value));
    }
    Ok(parts.join("/"))
}

fn stable_book_id(
    metadata: &EpubMetadata,
    package_path: &str,
    chapters: &[EpubChapter],
) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    let mut feed = |value: &str| {
        for byte in value.as_bytes().iter().chain(std::iter::once(&0)) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    feed(metadata.identifier.as_deref().unwrap_or_default().trim());
    feed(metadata.title.trim());
    for creator in &metadata.creators {
        feed(creator.trim());
    }
    feed(metadata.language.as_deref().unwrap_or_default().trim());
    feed(package_path);
    for chapter in chapters {
        feed(&chapter.href);
    }
    format!("book-{hash:016x}")
}

fn normalize_highlight_color(color: &str) -> &'static str {
    match color.trim().to_ascii_lowercase().as_str() {
        "green" => "green",
        "blue" => "blue",
        "pink" => "pink",
        "purple" => "purple",
        _ => "yellow",
    }
}

fn location_contains(
    start: &EpubLocation,
    end: &EpubLocation,
    point: &EpubLocation,
) -> bool {
    if start.chapter_href != point.chapter_href || end.chapter_href != point.chapter_href
    {
        return false;
    }
    let start_key = (start.source_line, start.source_column);
    let end_key = (end.source_line, end.source_column);
    let point_key = (point.source_line, point.source_column);
    let (low, high) = if start_key <= end_key {
        (start_key, end_key)
    } else {
        (end_key, start_key)
    };
    point_key >= low && point_key < high
}

fn safe_note_stem(title: &str) -> String {
    let value = title
        .trim()
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            other => other,
        })
        .collect::<String>();
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        "Untitled Book".to_string()
    } else {
        value
    }
}

fn canonical_book_note_path(
    vault_root: &Path,
    book: &EpubBook,
    current_path: Option<&Path>,
) -> PathBuf {
    let base = safe_note_stem(&book.metadata.title);
    let books_dir = vault_root.join("Books");
    let candidate = books_dir.join(&base).join(format!("{base}.md"));
    if !candidate.exists() || current_path == Some(candidate.as_path()) {
        return candidate;
    }

    let suffix = book.id.trim_start_matches("book-");
    let suffix = &suffix[..suffix.len().min(8)];
    let folder = format!("{base} {suffix}");
    let candidate = books_dir.join(&folder).join(format!("{base}.md"));
    if !candidate.exists() || current_path == Some(candidate.as_path()) {
        return candidate;
    }

    for index in 2.. {
        let folder = format!("{base} {suffix} {index}");
        let candidate = books_dir.join(folder).join(format!("{base}.md"));
        if !candidate.exists() || current_path == Some(candidate.as_path()) {
            return candidate;
        }
    }
    unreachable!()
}

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn book_note_header(book: &EpubBook, note_id: &str) -> String {
    let authors = if book.metadata.creators.is_empty() {
        String::new()
    } else {
        book.metadata.creators.join(", ")
    };
    let mut source = format!(
        "---\nneoism_note_id: {}\nepub_book_id: {}\ntype: book\ntitle: {}\n",
        yaml_string(note_id),
        yaml_string(&book.id),
        yaml_string(&book.metadata.title),
    );
    if !authors.is_empty() {
        source.push_str(&format!("author: {}\n", yaml_string(&authors)));
    }
    if let Some(identifier) = book.metadata.identifier.as_deref() {
        if !identifier.trim().is_empty() {
            source.push_str(&format!(
                "epub_identifier: {}\n",
                yaml_string(identifier.trim())
            ));
        }
    }
    source.push_str("tags:\n  - book\n  - reading\n---\n\n");
    source.push_str(&format!(
        "# {}\n\n{}\n\n## Highlights and notes\n",
        book.metadata.title,
        book_note_toc_block(&[]),
    ));
    source
}

const BOOK_NOTE_TOC_START: &str = "<!-- neoism-epub-toc:start -->";
const BOOK_NOTE_TOC_END: &str = "<!-- neoism-epub-toc:end -->";

fn annotation_note_heading(annotation: &EpubAnnotation) -> String {
    let title = if annotation.chapter_title.trim().is_empty() {
        "Book note".to_string()
    } else {
        format!(
            "{} · Page {}",
            annotation.chapter_title.trim(),
            annotation.page_index.saturating_add(1)
        )
    };
    title
        .replace('|', "—")
        .replace("[[", "[")
        .replace("]]", "]")
}

fn book_note_toc_block(annotations: &[EpubAnnotation]) -> String {
    let mut block = format!("{BOOK_NOTE_TOC_START}\n## Contents\n");
    let mut count = 0usize;
    for annotation in annotations
        .iter()
        .filter(|annotation| !annotation.note.trim().is_empty())
    {
        let heading = annotation_note_heading(annotation);
        block.push_str(&format!("\n- [[#{heading}|{heading}]]"));
        count += 1;
    }
    if count == 0 {
        block.push_str("\n_No notes yet._");
    }
    block.push_str(&format!("\n{BOOK_NOTE_TOC_END}"));
    block
}

fn replace_book_note_toc(source: &str, replacement: &str) -> String {
    if let Some(start) = source.find(BOOK_NOTE_TOC_START) {
        if let Some(relative_end) = source[start..].find(BOOK_NOTE_TOC_END) {
            let end = start + relative_end + BOOK_NOTE_TOC_END.len();
            let mut next = String::with_capacity(source.len() + replacement.len());
            next.push_str(source[..start].trim_end());
            next.push_str("\n\n");
            next.push_str(replacement);
            next.push_str("\n\n");
            next.push_str(source[end..].trim_start());
            return next;
        }
    }
    if let Some(index) = source.find("## Highlights and notes") {
        let mut next = String::with_capacity(source.len() + replacement.len());
        next.push_str(source[..index].trim_end());
        next.push_str("\n\n");
        next.push_str(replacement);
        next.push_str("\n\n");
        next.push_str(&source[index..]);
        return next;
    }
    format!("{}\n\n{}\n", source.trim_end(), replacement)
}

fn annotation_markdown_block(book: &EpubBook, annotation: &EpubAnnotation) -> String {
    let title = annotation_note_heading(annotation);
    let quote = annotation
        .selected_text
        .trim()
        .lines()
        .map(|line| format!("> {}", line.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<!-- neoism-epub-annotation:start {} -->\n### {}\n\n{}\n\n{}\n\n[[neoism-reader://{}/{}|Open in book]]\n<!-- neoism-epub-annotation:end {} -->",
        annotation.id,
        title,
        quote,
        annotation.note.trim(),
        book.id,
        annotation.id,
        annotation.id,
    )
}

const COLLECTION_CONTENT_START: &str = "<!-- neoism-epub-collection:start -->";
const COLLECTION_CONTENT_END: &str = "<!-- neoism-epub-collection:end -->";

fn collection_note_source(
    existing: &str,
    book: &EpubBook,
    collection: &EpubAnnotationCollection,
    annotations: &[EpubAnnotation],
) -> String {
    let note_id = format!("epub-collection-{}", collection.id);
    let header = format!(
        "---\nneoism_note_id: {}\nepub_book_id: {}\ntype: book-annotation-collection\nepub_collection_id: {}\ncollection_name: {}\n---\n\n# {}\n\n",
        yaml_string(&note_id),
        yaml_string(&book.id),
        yaml_string(&collection.id),
        yaml_string(&collection.name),
        collection.name,
    );
    let mut managed = format!("{COLLECTION_CONTENT_START}\n");
    for annotation in annotations.iter().filter(|annotation| {
        annotation.collection_ids.contains(&collection.id)
            && !annotation.note.trim().is_empty()
    }) {
        managed.push_str(&annotation_markdown_block(book, annotation));
        managed.push_str("\n\n");
    }
    managed.push_str(COLLECTION_CONTENT_END);

    if let (Some(start), Some(end)) = (
        existing.find(COLLECTION_CONTENT_START),
        existing.find(COLLECTION_CONTENT_END),
    ) {
        let end = end + COLLECTION_CONTENT_END.len();
        let mut source = existing.to_string();
        source.replace_range(start..end, &managed);
        source
    } else if existing.trim().is_empty() {
        format!("{header}{managed}\n")
    } else {
        format!("{}\n\n{managed}\n", existing.trim_end())
    }
}

fn replace_annotation_block(source: &str, id: &str, replacement: Option<&str>) -> String {
    let start_marker = format!("<!-- neoism-epub-annotation:start {id} -->");
    let end_marker = format!("<!-- neoism-epub-annotation:end {id} -->");
    if let Some(start) = source.find(&start_marker) {
        if let Some(relative_end) = source[start..].find(&end_marker) {
            let end = start + relative_end + end_marker.len();
            let mut next = String::with_capacity(source.len());
            next.push_str(source[..start].trim_end());
            if let Some(replacement) = replacement {
                next.push_str("\n\n");
                next.push_str(replacement.trim());
            }
            let tail = source[end..].trim_start_matches(['\r', '\n']);
            if !tail.is_empty() {
                next.push_str("\n\n");
                next.push_str(tail);
            } else {
                next.push('\n');
            }
            return next;
        }
    }
    let Some(replacement) = replacement else {
        return source.to_string();
    };
    let mut next = source.trim_end().to_string();
    next.push_str("\n\n");
    next.push_str(replacement.trim());
    next.push('\n');
    next
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("file")
    ));
    fs::write(&temp, bytes)?;
    fs::rename(temp, path)
}

fn visit_files(root: &Path, extension: &str, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|name| name == ".neoism" || name == ".git")
            {
                continue;
            }
            visit_files(&path, extension, output);
        } else if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            output.push(path);
        }
    }
}

fn resolve_book_note_path(
    vault_root: &Path,
    book_id: &str,
    stored_relative_path: Option<&str>,
) -> Option<PathBuf> {
    if let Some(relative) = stored_relative_path {
        let candidate = vault_root.join(relative);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let needle = format!("epub_book_id: {}", yaml_string(book_id));
    let mut files = Vec::new();
    visit_files(vault_root, "md", &mut files);
    files.into_iter().find(|path| {
        fs::read_to_string(path).ok().is_some_and(|source| {
            source.lines().take(40).any(|line| line.trim() == needle)
        })
    })
}

/// Resolve a book from its stable identity only when navigation needs it.
/// The fast path uses the last path in the sidecar; a vault scan is the
/// event-driven fallback after a drag/rename, avoiding a permanent poller.
pub fn resolve_book_path(vault_root: &Path, book_id: &str) -> Option<PathBuf> {
    let state_path =
        state_path_for_book_id(&vault_reader_state_root(vault_root), book_id);
    if let Some(state) = load_reading_state(&state_path) {
        if state.last_known_path.is_file()
            && EpubBook::open(state.last_known_path.clone())
                .ok()
                .is_some_and(|book| book.id == book_id)
        {
            return Some(state.last_known_path);
        }
    }
    let mut books = Vec::new();
    visit_files(vault_root, "epub", &mut books);
    books.into_iter().find(|path| {
        EpubBook::open(path.clone())
            .ok()
            .is_some_and(|book| book.id == book_id)
    })
}

pub fn vault_reader_state_root(vault_root: &Path) -> PathBuf {
    vault_root.join(READER_STATE_DIR)
}

pub fn state_path_for_book_id(state_root: &Path, book_id: &str) -> PathBuf {
    let safe = book_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect::<String>();
    state_root.join(format!(
        "{}.json",
        if safe.is_empty() { "book" } else { &safe }
    ))
}

/// Legacy v1 state was keyed by the canonical absolute path. Kept only for
/// one-time migration into the vault-owned, stable-book-id record.
pub fn legacy_state_path_for_book(state_root: &Path, book_path: &Path) -> PathBuf {
    let identity = book_path
        .canonicalize()
        .unwrap_or_else(|_| book_path.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let mut hash = 0xcbf29ce484222325u64;
    for byte in identity.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    state_root.join(format!("{hash:016x}.json"))
}

fn find_matching_legacy_state(state_root: &Path, book: &EpubBook) -> Option<PathBuf> {
    let chapter_hrefs = book
        .chapters
        .iter()
        .map(|chapter| chapter.href.as_str())
        .collect::<HashSet<_>>();
    let candidates = fs::read_dir(state_root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                return None;
            }
            let state = load_reading_state(&path)?;
            let hrefs = std::iter::once(state.location.chapter_href.as_str())
                .chain(state.annotations.iter().flat_map(|annotation| {
                    [
                        annotation.start.chapter_href.as_str(),
                        annotation.end.chapter_href.as_str(),
                    ]
                }))
                .filter(|href| !href.is_empty())
                .collect::<Vec<_>>();
            (!hrefs.is_empty() && hrefs.iter().all(|href| chapter_hrefs.contains(href)))
                .then_some(path)
        })
        .collect::<Vec<_>>();
    (candidates.len() == 1).then(|| candidates[0].clone())
}

pub fn load_reading_state(path: &Path) -> Option<EpubReadingState> {
    let source = fs::read_to_string(path).ok()?;
    let mut state: EpubReadingState = serde_json::from_str(&source).ok()?;
    if state.version == 0 || state.version > READER_STATE_VERSION {
        return None;
    }
    state.version = READER_STATE_VERSION;
    Some(state)
}

pub fn save_reading_state(path: &Path, state: &EpubReadingState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    let payload = serde_json::to_vec_pretty(state)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(&temp, payload)?;
    fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    fn write_epub(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let stored = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        zip.start_file("META-INF/container.xml", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
        zip.start_file("OEBPS/content.opf", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Golden Book</dc:title><dc:creator>A. Reader</dc:creator><dc:language>en</dc:language></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="one" href="text/one.xhtml" media-type="application/xhtml+xml"/><item id="two" href="text/two.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="one"/><itemref idref="two"/></spine></package>"#).unwrap();
        zip.start_file("OEBPS/nav.xhtml", deflated).unwrap();
        zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="text/one.xhtml">Opening</a></li><li><a href="text/two.xhtml#part">Second</a></li></ol></nav></body></html>"#).unwrap();
        zip.start_file("OEBPS/text/one.xhtml", deflated).unwrap();
        zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Opening</h1><p>Hello <em>reader</em>.</p><img src="../images/pixel.png" alt="A tiny test image"/><p><a href="two.xhtml#part">Continue</a></p></body></html>"#).unwrap();
        zip.start_file("OEBPS/text/two.xhtml", deflated).unwrap();
        zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1 id="part">Second</h1><blockquote><p>Remember this.</p></blockquote></body></html>"#).unwrap();
        zip.start_file("OEBPS/images/pixel.png", deflated).unwrap();
        let mut pixel = Cursor::new(Vec::new());
        image_rs::DynamicImage::new_rgba8(1, 1)
            .write_to(&mut pixel, image_rs::ImageFormat::Png)
            .unwrap();
        zip.write_all(&pixel.into_inner()).unwrap();
        zip.finish().unwrap();
    }

    fn write_reader_sequence_epub(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let stored = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        zip.start_file("META-INF/container.xml", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
        zip.start_file("OPS/content.opf", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Sequence Book</dc:title></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/><item id="title" href="title.xhtml" media-type="application/xhtml+xml"/><item id="part" href="part.xhtml" media-type="application/xhtml+xml"/><item id="one" href="one.xhtml" media-type="application/xhtml+xml"/><item id="image" href="cover.png" media-type="image/png"/></manifest><spine><itemref idref="cover"/><itemref idref="title"/><itemref idref="part"/><itemref idref="one"/></spine></package>"#).unwrap();
        zip.start_file("OPS/nav.xhtml", deflated).unwrap();
        zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="part.xhtml">PART ONE</a><ol><li><a href="one.xhtml">1</a></li></ol></li></ol></nav></body></html>"#).unwrap();
        zip.start_file("OPS/cover.xhtml", deflated).unwrap();
        zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="cover.png" alt="Cover"/></body></html>"#).unwrap();
        zip.start_file("OPS/title.xhtml", deflated).unwrap();
        zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Sequence Book</h1><p>by A. Reader</p></body></html>"#).unwrap();
        zip.start_file("OPS/part.xhtml", deflated).unwrap();
        zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>PART ONE</h1></body></html>"#).unwrap();
        zip.start_file("OPS/one.xhtml", deflated).unwrap();
        zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h2>1</h2><p>The chapter body starts on the same page as its heading.</p></body></html>"#).unwrap();
        zip.start_file("OPS/cover.png", deflated).unwrap();
        let mut pixel = Cursor::new(Vec::new());
        image_rs::DynamicImage::new_rgba8(1, 1)
            .write_to(&mut pixel, image_rs::ImageFormat::Png)
            .unwrap();
        zip.write_all(&pixel.into_inner()).unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn opens_epub3_metadata_spine_toc_and_chapter_source() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_epub(&path);
        let mut book = EpubBook::open(path).unwrap();
        assert_eq!(book.metadata.title, "Golden Book");
        assert_eq!(book.metadata.creators, ["A. Reader"]);
        assert_eq!(book.chapters.len(), 2);
        assert_eq!(book.chapters[0].href, "OEBPS/text/one.xhtml");
        assert_eq!(book.toc[1].href, "OEBPS/text/two.xhtml#part");
        assert_eq!(book.chapter_index_for_href(&book.toc[1].href), Some(1));
        let source = book.load_chapter_source(0).unwrap();
        assert!(source.contains("# Opening"));
        assert!(source.contains("Hello *reader*."));
        assert!(source.contains("[[neoism-epub://OEBPS/text/two.xhtml#part|Continue]]"));
        assert!(source.contains("neoism-epub-resource://OEBPS/images/pixel.png"));
        let second = book.load_chapter_content(1).unwrap();
        assert_eq!(second.anchors.get("part"), Some(&0));
        book.chapters[0].title = "Opening".to_string();
    }

    #[test]
    fn fragment_navigation_jumps_to_the_xhtml_anchor() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_epub(&path);
        let state_root = dir.path().join("reader-state");
        let mut pane = EpubPane::load(path, &state_root);

        assert!(pane.go_to_href("OEBPS/text/two.xhtml#part").unwrap());
        assert_eq!(pane.chapter_index, 1);
        assert_eq!(pane.markdown.cursor_line, 0);
        assert_eq!(pane.state.location.fragment.as_deref(), Some("part"));
    }

    #[test]
    fn contents_is_a_real_reader_page_with_working_chapter_links() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_epub(&path);
        let state_root = dir.path().join("reader-state");
        let mut pane = EpubPane::load(path, &state_root);

        assert!(pane.open_contents_page().unwrap());
        assert!(pane.showing_contents);
        assert_eq!(pane.markdown.title, "Contents");
        let contents = pane.markdown.lines.join("\n");
        assert!(contents.contains("Opening"));
        assert!(contents.contains("Second"));
        assert!(contents.contains("neoism-epub://OEBPS/text/two.xhtml#part"));

        assert!(pane.go_to_href("OEBPS/text/two.xhtml#part").unwrap());
        assert!(!pane.showing_contents);
        assert_eq!(pane.chapter_index, 1);
        assert_eq!(pane.markdown.title, "Second");
        assert!(!pane.markdown.title.contains("Page"));
    }

    #[test]
    fn reading_sequence_places_contents_after_cover_and_skips_title_only_pages() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sequence.epub");
        write_reader_sequence_epub(&path);
        let mut pane = EpubPane::load(path, &dir.path().join("reader-state"));

        assert_eq!(pane.chapter_index, 0);
        assert_eq!(restored_reading_chapter_index(&pane.book, 2), 3);
        assert!(pane.next_page().unwrap());
        assert!(pane.showing_contents);
        assert_eq!(pane.markdown.title, "Contents");

        assert!(pane.next_page().unwrap());
        assert!(!pane.showing_contents);
        assert_eq!(pane.chapter_index, 3);
        assert_eq!(pane.markdown.title, "1");
        assert!(pane.markdown.lines.join("\n").contains("chapter body"));
        assert_eq!(pane.markdown.scroll_y, 0.0);

        assert!(pane.previous_page().unwrap());
        assert!(pane.showing_contents);
    }

    #[test]
    fn title_only_sections_and_short_frontmatter_are_skipped() {
        assert!(reader_content_is_skippable_frontmatter(
            &EpubChapterContent::default(),
            "Project Gutenberg footer",
            false,
        ));

        let divider = EpubChapterContent {
            source: "# PART ONE. CHIBA CITY BLUES".to_string(),
            anchors: HashMap::new(),
        };
        assert!(reader_content_is_skippable_frontmatter(
            &divider,
            "PART ONE. CHIBA CITY BLUES",
            false,
        ));

        let title_page = EpubChapterContent {
            source: "Neuromancer\n\nby William Gibson".to_string(),
            anchors: HashMap::new(),
        };
        assert!(reader_content_is_skippable_frontmatter(
            &title_page,
            "Neuromancer",
            true,
        ));

        let real_chapter = EpubChapterContent {
            source: "# 1\n\nThe sky above the port was the color of television."
                .to_string(),
            anchors: HashMap::new(),
        };
        assert!(!reader_content_is_skippable_frontmatter(
            &real_chapter,
            "1",
            false,
        ));
    }

    #[test]
    fn continuation_pages_drop_the_repeated_title_and_start_at_the_top() {
        let source = (0..20)
            .map(|index| {
                format!(
                    "Paragraph {index} fills a normal reader row with enough words to exercise bounded desktop pagination."
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let ranges = paginate_reader_source(&source);
        assert!(ranges.len() > 1);
        let page = reader_page_source(&source, ranges[1].clone(), 1, "Chapter 1");
        let mut markdown = MarkdownPane::from_source(PathBuf::from("book.epub"), &page);
        markdown.title = String::new();
        markdown.restore_scroll_position(300.0);
        markdown.set_source_for_navigation(&page);

        assert!(markdown.title.is_empty());
        assert_eq!(markdown.scroll_y, 0.0);
    }

    #[test]
    fn persisted_location_restores_by_href_not_old_spine_index() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_epub(&path);
        let state_root = dir.path().join("reader-state");
        let book = EpubBook::open(path.clone()).unwrap();
        let state_path = state_path_for_book_id(&state_root, &book.id);
        let state = EpubReadingState {
            location: EpubLocation {
                chapter_href: "OEBPS/text/two.xhtml".to_string(),
                source_line: 1,
                source_column: 2,
                fragment: Some("part".to_string()),
            },
            progress: 0.75,
            scroll_y: 88.0,
            ..EpubReadingState::default()
        };
        save_reading_state(&state_path, &state).unwrap();
        let pane = EpubPane::load(path, &state_root);
        assert_eq!(pane.chapter_index, 1);
        assert_eq!(pane.state.location.chapter_href, "OEBPS/text/two.xhtml");
        assert_eq!(pane.markdown.cursor_line, 1);
        assert_eq!(pane.markdown.scroll_y, 88.0);
    }

    #[test]
    fn path_normalization_rejects_archive_escape() {
        assert_eq!(
            normalize_archive_path("OEBPS/text", "../images/cover.jpg").unwrap(),
            "OEBPS/images/cover.jpg"
        );
        assert!(normalize_archive_path("", "../../etc/passwd").is_err());
        assert!(normalize_archive_path("", "/absolute").is_err());
    }

    #[test]
    fn epub2_doctypes_are_ignored_without_retaining_entity_declarations() {
        let external = r#"<?xml version="1.0"?>
<!DOCTYPE ncx PUBLIC "-//NISO//DTD ncx 2005-1//EN"
  "http://www.daisy.org/z3986/2005/ncx-2005-1.dtd">
<ncx><navMap/></ncx>"#;
        let sanitized = xml_without_doctype(external);
        assert!(!sanitized.contains("DOCTYPE"));
        assert!(Document::parse(&sanitized).is_ok());

        let internal = r#"<!DOCTYPE book [
<!ELEMENT book (#PCDATA)>
<!ENTITY unsafe SYSTEM "file:///etc/passwd">
]><book>safe</book>"#;
        let sanitized = xml_without_doctype(internal);
        assert_eq!(sanitized.as_ref(), "<book>safe</book>");
        assert!(!sanitized.contains("unsafe"));
        assert!(Document::parse(&sanitized).is_ok());

        let html_entities = r#"<!DOCTYPE html SYSTEM "xhtml1-strict.dtd"><html><body>A&nbsp;reader &amp; an &unknown; name</body></html>"#;
        let sanitized = epub_xml_for_parse(html_entities);
        let document = Document::parse(&sanitized).unwrap();
        let text = document.descendants().find_map(|node| node.text()).unwrap();
        assert_eq!(text, "A\u{00a0}reader & an &unknown; name");
    }

    #[test]
    fn xhtml_conversion_preserves_reading_semantics() {
        let source = xhtml_to_markdown(
            r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"><html><body><h2>Title</h2><p>A&nbsp;reader</p><ol><li>One</li><li><strong>Two</strong></li></ol><pre>let x = 1;</pre><img src="../img/p.png" alt="Plot"/></body></html>"#,
            "OPS/text",
        );
        assert!(source.contains("## Title"));
        assert!(source.contains("A reader"));
        assert!(source.contains("1. One"));
        assert!(source.contains("2. **Two**"));
        assert!(source.contains("```\nlet x = 1;\n```"));
        assert!(source.contains("![Plot](neoism-epub-resource://OPS/img/p.png)"));
    }

    #[test]
    fn xhtml_conversion_drops_project_gutenberg_boilerplate() {
        let source = xhtml_to_markdown(
            r#"<html><body>
                <p>The final verse.</p>
                <footer class="pg-boilerplate pgheader" id="pg-footer">
                  <div id="pg-end-separator"><span>*** END OF THE PROJECT GUTENBERG EBOOK TEST BOOK ***</span></div>
                  <h2 id="pg-footer-heading">THE FULL PROJECT GUTENBERG LICENSE</h2>
                  <p>License boilerplate should not become a reader page.</p>
                </footer>
              </body></html>"#,
            "OPS",
        );

        assert_eq!(source.trim(), "The final verse.");
        assert!(!source.contains("GUTENBERG"));
        assert!(!source.contains("***"));
    }

    #[test]
    fn visual_highlight_is_persisted_and_projected_back_into_reader() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_epub(&path);
        let state_root = dir.path().join("reader-state");
        let mut pane = EpubPane::load(path.clone(), &state_root);
        let image_page = (0..pane.page_count)
            .find(|page| {
                pane.go_to_page(0, *page).unwrap();
                !pane.rendered_images.is_empty()
            })
            .expect("image should be mounted on one bounded page");
        assert!(image_page < pane.page_count);
        assert_eq!(pane.rendered_images.len(), 1);
        assert_eq!(
            (
                pane.rendered_images[0].width,
                pane.rendered_images[0].height
            ),
            (1, 1)
        );
        pane.go_to_page(0, 0).unwrap();
        pane.markdown.cursor_line = pane
            .markdown
            .lines
            .iter()
            .position(|line| line.contains("Hello"))
            .unwrap();
        pane.markdown.cursor_col = 0;
        pane.markdown.enter_visual();
        for _ in 0..5 {
            pane.markdown.move_right();
        }
        let id = pane
            .add_highlight_from_selection("Good opening".to_string())
            .unwrap()
            .unwrap();
        assert!(pane.set_annotation_color(&id, "purple").unwrap());
        assert!(pane
            .set_annotation_note(&id, "Updated note".to_string())
            .unwrap());
        let themes = pane
            .create_annotation_collection("Themes".to_string())
            .unwrap();
        let research = pane
            .create_annotation_collection("Research".to_string())
            .unwrap();
        assert!(pane.add_annotation_to_collection(&id, &themes).unwrap());
        assert!(pane.add_annotation_to_collection(&id, &research).unwrap());
        let book_dir = pane
            .book_note_path()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(fs::read_to_string(book_dir.join("Themes.md"))
            .unwrap()
            .contains("Updated note"));
        assert!(fs::read_to_string(book_dir.join("Research.md"))
            .unwrap()
            .contains("Updated note"));
        assert!(pane
            .set_annotation_note(&id, "Updated in every collection".to_string())
            .unwrap());
        assert!(fs::read_to_string(book_dir.join("Themes.md"))
            .unwrap()
            .contains("Updated in every collection"));
        assert!(fs::read_to_string(book_dir.join("Research.md"))
            .unwrap()
            .contains("Updated in every collection"));
        let note_path = pane.book_note_path().expect("book note created in vault");
        assert_eq!(
            note_path.strip_prefix(&state_root).unwrap(),
            Path::new("Books/Golden Book/Golden Book.md")
        );
        let note_source = fs::read_to_string(&note_path).unwrap();
        assert!(note_source.contains("## Contents"));
        assert!(note_source.contains("[[#Opening · Page 1|Opening · Page 1]]"));
        assert!(note_source.contains(&format!(
            "[[neoism-reader://{}/{}|Open in book]]",
            pane.book.id, id
        )));
        assert!(!note_source.contains("[Open in book]("));
        let moved_note = state_root.join("Moved Neuromancer.md");
        fs::rename(&note_path, &moved_note).unwrap();
        assert!(pane
            .set_annotation_note(&id, "Found after note move".to_string())
            .unwrap());
        let moved_book_dir = moved_note.parent().unwrap().to_path_buf();
        assert!(fs::read_to_string(&moved_note)
            .unwrap()
            .contains("Found after note move"));
        assert!(pane.go_to_annotation(&id).unwrap());
        assert_eq!(pane.state.annotations.len(), 1);
        assert_eq!(pane.state.annotations[0].note, "Found after note move");
        assert_eq!(pane.state.annotations[0].color, "purple");
        assert_eq!(pane.markdown.reader_highlights.len(), 1);
        drop(pane);

        let mut restored = EpubPane::load(path.clone(), &state_root);
        assert_eq!(restored.state.annotations[0].id, id);
        assert_eq!(restored.markdown.reader_highlights.len(), 1);
        assert!(restored.remove_annotation(&id).unwrap());
        assert!(!fs::read_to_string(moved_book_dir.join("Themes.md"))
            .unwrap()
            .contains(&id));
        assert!(!fs::read_to_string(moved_book_dir.join("Research.md"))
            .unwrap()
            .contains(&id));
        drop(restored);

        let without_annotation = EpubPane::load(path, &state_root);
        assert!(without_annotation.state.annotations.is_empty());
        assert!(without_annotation.markdown.reader_highlights.is_empty());
    }

    #[test]
    fn stable_book_identity_and_vault_state_survive_epub_move() {
        let dir = tempdir().unwrap();
        let vault = dir.path().join("Default");
        fs::create_dir_all(&vault).unwrap();
        let first = vault.join("book.epub");
        write_epub(&first);
        let first_id = EpubBook::open(first.clone()).unwrap().id;
        let mut pane = EpubPane::load_in_vault(first.clone(), &vault, None);
        pane.state.book_note_path = Some("Books/book.md".to_string());
        pane.save_state().unwrap();
        drop(pane);

        let moved_dir = vault.join("Library");
        fs::create_dir_all(&moved_dir).unwrap();
        let moved = moved_dir.join("renamed.epub");
        fs::rename(&first, &moved).unwrap();
        let second_id = EpubBook::open(moved.clone()).unwrap().id;
        assert_eq!(first_id, second_id);

        let restored = EpubPane::load_in_vault(moved.clone(), &vault, None);
        assert_eq!(
            restored.state.book_note_path.as_deref(),
            Some("Books/book.md")
        );
        assert_eq!(
            restored.state.last_known_path,
            moved.canonicalize().unwrap()
        );
        assert_eq!(resolve_book_path(&vault, &first_id), Some(moved));
    }

    #[test]
    fn legacy_flat_book_note_moves_into_its_book_folder() {
        let dir = tempdir().unwrap();
        let vault = dir.path().join("Default");
        let books = vault.join("Books");
        fs::create_dir_all(&books).unwrap();
        let path = vault.join("book.epub");
        write_epub(&path);
        let mut pane = EpubPane::load_in_vault(path, &vault, None);
        let legacy = books.join("Golden Book.md");
        fs::write(
            &legacy,
            book_note_header(&pane.book, &pane.state.book_note_id),
        )
        .unwrap();
        pane.state.book_note_path = Some("Books/Golden Book.md".to_string());
        pane.save_state().unwrap();

        let adopted = pane.ensure_book_note().unwrap();
        assert_eq!(
            adopted.strip_prefix(&vault).unwrap(),
            Path::new("Books/Golden Book/Golden Book.md")
        );
        assert!(!legacy.exists());
        assert!(adopted.is_file());
        assert_eq!(
            pane.state.book_note_path.as_deref(),
            Some("Books/Golden Book/Golden Book.md")
        );
    }

    #[test]
    fn reader_pages_are_bounded_and_contiguous() {
        let paragraph = "A long paragraph that deliberately wraps across several visual rows in the reader while remaining one semantic block. ".repeat(7);
        let source = (0..12)
            .map(|index| format!("## Section {index}\n\n{paragraph}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let ranges = paginate_reader_source(&source);
        assert!(ranges.len() > 3);
        assert!(ranges.iter().all(|range| range.start < range.end));
        assert_eq!(ranges.first().unwrap().start, 0);
        assert_eq!(ranges.last().unwrap().end, source.split('\n').count());
        assert!(ranges
            .windows(2)
            .all(|pages| pages[0].end == pages[1].start));
        assert!(ranges
            .iter()
            .all(|range| page_source(&source, range.clone()).len() < source.len()));
    }
}
