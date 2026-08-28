#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectableCaretStop {
    pub byte_offset: usize,
    pub x: f32,
}

#[derive(Clone, Debug)]
pub struct SelectableLine {
    pub text: String,
    pub rect: [f32; 4],
    pub content_y: f32,
    pub caret_stops: Vec<SelectableCaretStop>,
}

impl SelectableLine {
    pub fn new(text: &str, rect: [f32; 4], content_y: f32) -> Self {
        Self {
            text: text.to_string(),
            rect,
            content_y,
            caret_stops: uniform_caret_stops(text, rect),
        }
    }

    pub fn set(
        &mut self,
        text: &str,
        rect: [f32; 4],
        content_y: f32,
        caret_stops: Option<&[SelectableCaretStop]>,
    ) {
        self.text.clear();
        self.text.push_str(text);
        self.rect = rect;
        self.content_y = content_y;
        self.caret_stops.clear();
        if let Some(stops) = caret_stops.filter(|stops| stops.len() >= 2) {
            self.caret_stops.extend_from_slice(stops);
        } else {
            self.caret_stops.extend(uniform_caret_stops(text, rect));
        }
    }

    pub fn caret_at_x(&self, x: f32) -> SelectableCaretStop {
        self.caret_stops
            .iter()
            .copied()
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
    fn exact_stops_select_a_narrow_subrange() {
        let line = SelectableLine {
            text: "WW ii".into(),
            rect: [10.0, 20.0, 50.0, 18.0],
            content_y: 20.0,
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
            ],
        };
        let start = line.caret_at_x(45.1);
        let end = line.caret_at_x(51.0);
        assert_eq!(line.slice_between(start.byte_offset, end.byte_offset), "ii");
    }
}
