use std::sync::Arc;

/// Immutable exact geometry; clones retain both allocations. Code rows use
/// local stops plus a registration origin. Inline Markdown retains its historic
/// screen-origin accumulation (registered with zero additional translation).
#[derive(Clone, Debug)]
pub struct SelectionGeometry {
    pub text: Arc<str>,
    pub stops: Arc<[SelectableCaretStop]>,
    pub width: f32,
}

pub fn selection_line_key(line: &SelectableLine) -> (i64, i64) {
    // Content coordinates stay fixed as the viewport scrolls. Round to pixel
    // precision so redraws at fractional scroll offsets replace the same row.
    (line.content_y.round() as i64, line.rect[0].round() as i64)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectableCaretStop {
    pub byte_offset: usize,
    pub x: f32,
}

#[derive(Clone, Debug)]
pub struct SelectableLine {
    pub text: Arc<str>,
    pub rect: [f32; 4],
    pub content_y: f32,
    pub caret_stops: Arc<[SelectableCaretStop]>,
    pub caret_origin_x: f32,
}

impl SelectableLine {
    pub fn new(text: &str, rect: [f32; 4], content_y: f32) -> Self {
        Self {
            text: text.into(),
            rect,
            content_y,
            caret_stops: uniform_caret_stops(text, rect).into(),
            caret_origin_x: 0.0,
        }
    }

    pub fn set(
        &mut self,
        text: &str,
        rect: [f32; 4],
        content_y: f32,
        caret_stops: Option<&[SelectableCaretStop]>,
    ) {
        if self.text.as_ref() != text {
            self.text = text.into();
        }
        self.rect = rect;
        self.content_y = content_y;
        self.caret_origin_x = 0.0;
        self.caret_stops =
            if let Some(stops) = caret_stops.filter(|stops| stops.len() >= 2) {
                stops.into()
            } else {
                uniform_caret_stops(text, rect).into()
            };
    }

    pub fn from_geometry(
        geometry: &SelectionGeometry,
        rect: [f32; 4],
        content_y: f32,
        origin_x: f32,
    ) -> Self {
        // Legacy slice registration used uniform fallback for a singleton
        // empty-row stop, anchoring it to the clipped hit rect, not text x.
        let origin_x = if geometry.text.is_empty() && geometry.stops.len() == 1 {
            rect[0] - geometry.stops[0].x
        } else {
            origin_x
        };
        Self {
            text: geometry.text.clone(),
            caret_stops: geometry.stops.clone(),
            rect,
            content_y,
            caret_origin_x: origin_x,
        }
    }

    pub fn caret_at_x(&self, x: f32) -> SelectableCaretStop {
        self.caret_stops
            .iter()
            .copied()
            .map(|mut stop| {
                stop.x += self.caret_origin_x;
                stop
            })
            .min_by(|a, b| {
                (a.x - x)
                    .abs()
                    .partial_cmp(&(b.x - x).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(SelectableCaretStop {
                byte_offset: 0,
                x: self.rect[0],
            })
    }

    /// Byte offset of the grapheme painted under `x`. Unlike `caret_at_x`,
    /// this chooses the interval between adjacent measured caret stops, so a
    /// hard hold on the right half of a wide glyph still targets that glyph.
    pub fn text_offset_at_x(&self, x: f32) -> usize {
        self.caret_stops
            .windows(2)
            .find(|pair| x < pair[1].x + self.caret_origin_x)
            .map(|pair| pair[0].byte_offset)
            .unwrap_or_else(|| self.text.len().saturating_sub(1))
    }

    pub fn caret_for_offset(&self, offset: usize) -> SelectableCaretStop {
        self.caret_stops
            .iter()
            .copied()
            .find(|stop| stop.byte_offset == offset)
            .map(|mut stop| {
                stop.x += self.caret_origin_x;
                stop
            })
            .unwrap_or_else(|| self.caret_at_x(self.rect[0]))
    }

    pub fn slice_between(&self, a: usize, b: usize) -> String {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        self.text
            .get(start.min(self.text.len())..end.min(self.text.len()))
            .unwrap_or_default()
            .to_string()
    }
}

pub fn uniform_caret_stops(text: &str, rect: [f32; 4]) -> Vec<SelectableCaretStop> {
    let count = text.chars().count();
    let mut stops = Vec::with_capacity(count + 1);
    stops.push(SelectableCaretStop {
        byte_offset: 0,
        x: rect[0],
    });
    if count == 0 {
        return stops;
    }
    let advance = rect[2].max(0.0) / count as f32;
    for (index, (byte_offset, ch)) in text.char_indices().enumerate() {
        stops.push(SelectableCaretStop {
            byte_offset: byte_offset + ch.len_utf8(),
            x: rect[0] + advance * (index + 1) as f32,
        });
    }
    stops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_clipped_row_preserves_legacy_fallback_caret() {
        let geometry = SelectionGeometry {
            text: "".into(),
            stops: vec![SelectableCaretStop {
                byte_offset: 0,
                x: 0.0,
            }]
            .into(),
            width: 0.0,
        };
        let rect = [42.0, 20.0, 8.0, 18.0];
        let retained = SelectableLine::from_geometry(&geometry, rect, 20.0, 38.0);
        let mut legacy = SelectableLine::new("", rect, 20.0);
        legacy.set(
            "",
            rect,
            20.0,
            Some(&[SelectableCaretStop {
                byte_offset: 0,
                x: 38.0,
            }]),
        );
        assert_eq!(retained.caret_at_x(40.0), legacy.caret_at_x(40.0));
        assert_eq!(retained.caret_for_offset(0), legacy.caret_for_offset(0));
    }

    #[test]
    fn retained_translation_matches_screen_space_adapter_exactly() {
        let geometry = SelectionGeometry {
            text: "a\u{301}\t界".into(),
            stops: vec![
                SelectableCaretStop {
                    byte_offset: 0,
                    x: 0.0,
                },
                SelectableCaretStop {
                    byte_offset: 3,
                    x: 7.125,
                },
                SelectableCaretStop {
                    byte_offset: 4,
                    x: 35.625,
                },
                SelectableCaretStop {
                    byte_offset: 7,
                    x: 50.25,
                },
            ]
            .into(),
            width: 50.25,
        };
        for origin in [-152.75, 0.0, 137.5] {
            let rect = [origin + 4.0, 20.0, 40.0, 18.0];
            let retained = SelectableLine::from_geometry(&geometry, rect, 20.0, origin);
            let screen: Vec<_> = geometry
                .stops
                .iter()
                .map(|stop| SelectableCaretStop {
                    byte_offset: stop.byte_offset,
                    x: stop.x + origin,
                })
                .collect();
            let mut legacy = SelectableLine::new(&geometry.text, rect, 20.0);
            legacy.set(&geometry.text, rect, 20.0, Some(&screen));
            for dx in -10..70 {
                let x = origin + dx as f32;
                assert_eq!(retained.caret_at_x(x), legacy.caret_at_x(x));
                assert_eq!(retained.text_offset_at_x(x), legacy.text_offset_at_x(x));
            }
            for offset in [0, 3, 4, 7, 99] {
                assert_eq!(
                    retained.caret_for_offset(offset),
                    legacy.caret_for_offset(offset)
                );
            }
            assert_eq!(retained.slice_between(3, 7), "\t界");
        }
    }

    #[test]
    fn exact_stops_select_a_narrow_subrange() {
        let line = SelectableLine {
            text: "WW ii".into(),
            rect: [10.0, 20.0, 50.0, 18.0],
            content_y: 20.0,
            caret_origin_x: 0.0,
            caret_stops: vec![
                SelectableCaretStop {
                    byte_offset: 0,
                    x: 10.0,
                },
                SelectableCaretStop {
                    byte_offset: 1,
                    x: 25.0,
                },
                SelectableCaretStop {
                    byte_offset: 2,
                    x: 40.0,
                },
                SelectableCaretStop {
                    byte_offset: 3,
                    x: 45.0,
                },
                SelectableCaretStop {
                    byte_offset: 4,
                    x: 48.0,
                },
                SelectableCaretStop {
                    byte_offset: 5,
                    x: 51.0,
                },
            ]
            .into(),
        };
        let start = line.caret_at_x(45.1);
        let end = line.caret_at_x(51.0);
        assert_eq!(line.slice_between(start.byte_offset, end.byte_offset), "ii");
    }
}
