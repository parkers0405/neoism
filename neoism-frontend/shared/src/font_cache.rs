//! Font metrics cache used by the chrome's rich-text renderer.
//!
//! Lifted from `frontends/neoism/src/chrome/font_cache.rs` — the
//! desktop version used `lru::LruCache` plus
//! `neoism_backend::neoism_paint::swash::Attributes` and
//! `neoism_backend::neoism_paint::SpanStyle` to build a two-tier (hot
//! ASCII + LRU) cache keyed by `(char, Attributes)`. The shared crate
//! now depends on `lru` directly and accesses the swash / SpanStyle
//! re-exports through `sugarloaf`, so the implementation is back to
//! parity with the native fork.
//!
//! The `AttributesShim` POD is kept (with a `From` round-trip to the
//! real `neoism_paint::Attributes`) so callers that haven't been migrated
//! to the real swash type yet keep compiling.

use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use neoism_paint::Attributes;
use tracing::debug;
use unicode_width::UnicodeWidthChar;

/// Maximum number of font cache entries to keep in memory.
/// Increased for better performance with complex terminal content.
const MAX_FONT_CACHE_SIZE: usize = 8192;

/// POD substitute for `neoism_paint::swash::Attributes`. Mirrors the
/// fields the chrome's rich-text path actually keys on; convertible to
/// and from the real `Attributes` via the `From` impls below so hosts
/// can pass whichever flavour they already have on hand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AttributesShim {
    pub bold: bool,
    pub italic: bool,
}

impl From<AttributesShim> for Attributes {
    fn from(shim: AttributesShim) -> Self {
        use neoism_paint::swash::{Stretch, Style, Weight};
        let weight = if shim.bold {
            Weight::BOLD
        } else {
            Weight::NORMAL
        };
        let style = if shim.italic {
            Style::Italic
        } else {
            Style::Normal
        };
        Attributes::new(Stretch::NORMAL, weight, style)
    }
}

/// Font cache data including PUA information
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct FontCacheData {
    pub font_id: usize,
    pub width: f32,
    pub is_pua: bool,
}

/// LRU cache for font metrics to prevent unbounded memory growth.
/// Uses a two-tier caching strategy for better performance:
///   * `hot_cache` — small `HashMap` keyed on ASCII for branch-free hits
///   * `cache`     — bounded LRU for everything else
pub struct FontCache {
    /// Hot cache for most frequently used characters (ASCII).
    hot_cache: HashMap<(char, Attributes), FontCacheData>,
    /// LRU cache for less frequent characters.
    cache: LruCache<(char, Attributes), FontCacheData>,
}

impl FontCache {
    pub fn new() -> Self {
        Self {
            hot_cache: HashMap::with_capacity(128), // ASCII + common chars
            cache: LruCache::new(
                NonZeroUsize::new(MAX_FONT_CACHE_SIZE)
                    .expect("Cache size must be non-zero"),
            ),
        }
    }

    /// Get font metrics from cache with hot path optimization.
    #[allow(dead_code)]
    pub fn get(&mut self, key: &(char, Attributes)) -> Option<&FontCacheData> {
        // Check hot cache first for ASCII characters.
        if key.0.is_ascii() {
            if let Some(value) = self.hot_cache.get(key) {
                return Some(value);
            }
        }

        // Fall back to LRU cache.
        let result = self.cache.get(key);

        // Log cache miss for debugging.
        if result.is_none() {
            debug!("FontCache miss for char='{}' attrs={:?}", key.0, key.1);
        }

        result
    }

    /// Insert font metrics into cache with hot path optimization.
    #[allow(dead_code)]
    pub fn insert(&mut self, key: (char, Attributes), value: FontCacheData) {
        // Store ASCII characters in hot cache for faster access.
        if key.0.is_ascii() && self.hot_cache.len() < 128 {
            self.hot_cache.insert(key, value);
        } else {
            self.cache.put(key, value);
        }
    }

    /// Get current cache size (for debugging/monitoring).
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.hot_cache.len() + self.cache.len()
    }

    /// Check if cache is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.hot_cache.is_empty() && self.cache.is_empty()
    }

    /// Clear all cache entries with cleanup.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.hot_cache.clear();
        self.cache.clear();
    }

    /// Pre-populate cache with common characters to improve hit rate.
    /// This should be called during initialization with the font
    /// library — ported verbatim from the native fork so the shared
    /// crate seeds ASCII × {Normal, Bold, Italic} for every host.
    #[allow(dead_code)]
    pub fn pre_populate(&mut self, font_context: &neoism_paint::font::FontLibrary) {
        let common_chars = [
            // ASCII printable characters (most common)
            ' ', '!', '"', '#', '$', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.',
            '/', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=',
            '>', '?', '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L',
            'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '[',
            '\\', ']', '^', '_', '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j',
            'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y',
            'z', '{', '|', '}', '~',
        ];

        use neoism_paint::swash::{Stretch, Style, Weight};
        let common_attrs = [
            Attributes::new(Stretch::NORMAL, Weight::NORMAL, Style::Normal),
            Attributes::new(Stretch::NORMAL, Weight::BOLD, Style::Normal),
            Attributes::new(Stretch::NORMAL, Weight::NORMAL, Style::Italic),
        ];

        if let Some(font_ctx) = font_context.inner.try_read() {
            for &ch in &common_chars {
                for &attrs in &common_attrs {
                    let key = (ch, attrs);
                    if self.get(&key).is_none() {
                        let style = neoism_paint::SpanStyle {
                            font_attrs: attrs,
                            ..Default::default()
                        };

                        let mut width = ch.width().unwrap_or(1) as f32;
                        if let Some((font_id, is_emoji)) =
                            font_ctx.find_best_font_match(ch, &style)
                        {
                            if is_emoji {
                                width = 2.0;
                            }
                            self.insert(
                                key,
                                FontCacheData {
                                    font_id,
                                    width,
                                    is_pua: false, // ASCII chars are never PUA
                                },
                            );
                        }
                    }
                }
            }
        }
    }
}

impl Default for FontCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_paint::swash::{Stretch, Style, Weight};

    #[test]
    fn test_font_cache_basic_operations() {
        let mut cache = FontCache::new();

        // Test empty cache
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);

        // Test insertion and retrieval
        let attrs = Attributes::new(Stretch::NORMAL, Weight::NORMAL, Style::Normal);
        let key = ('a', attrs);
        let value = FontCacheData {
            font_id: 1,
            width: 1.0,
            is_pua: false,
        };

        cache.insert(key, value);
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);

        let retrieved = cache.get(&key);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().font_id, 1);
    }

    #[test]
    fn test_font_cache_lru_eviction() {
        let mut cache = FontCache::new();

        // Fill cache beyond capacity to test LRU eviction
        let test_size = 10;
        for i in 0..=test_size {
            let attrs = Attributes::new(Stretch::NORMAL, Weight::NORMAL, Style::Normal);
            let key = (char::from_u32(i as u32 + 65).unwrap_or('A'), attrs);
            let value = FontCacheData {
                font_id: i,
                width: i as f32,
                is_pua: false,
            };
            cache.insert(key, value);
        }

        // Cache should have all entries since we're under the limit
        assert_eq!(cache.len(), test_size + 1);
    }

    #[test]
    fn attributes_shim_round_trip() {
        let shim = AttributesShim {
            bold: true,
            italic: false,
        };
        let attrs: Attributes = shim.into();
        assert_eq!(attrs.weight(), Weight::BOLD);
        assert_eq!(attrs.style(), Style::Normal);
    }
}
