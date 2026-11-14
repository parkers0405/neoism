//! Top occupied rows from the bundled Press Start 2P outlines (1000 units/em, 125 units/cell).
//! Nine means an empty column. Keep the font's lowercase height and side bearings, not a flat edge.
use crate::ui::fire::PixelFire;
use std::{collections::HashMap, sync::OnceLock};

const EDGES: &[(char, &[u8; 8])] = &[
    ('!', b"99000999"), ('#', b"10010019"), ('$', b"21101149"),
    ('%', b"10032109"), ('&', b"10001549"), ('*', b"31121139"),
    ('+', b"93311339"), ('-', b"93333339"), ('/', b"65432109"),
    ('<', b"93210099"), ('=', b"22222229"), ('>', b"90012399"),
    ('?', b"10000019"), ('@', b"10000019"), ('B', b"00000019"),
    ('C', b"21000019"), ('G', b"21000009"), ('P', b"00000019"), ('S', b"10000019"),
    ('T', b"90000009"), ('\\', b"01234569"), ('^', b"91000199"),
    ('a', b"52222239"), ('b', b"00222239"), ('c', b"32222229"),
    ('d', b"32222009"), ('e', b"32222239"), ('f', b"92210009"),
    ('g', b"32222229"), ('i', b"96200669"), ('k', b"00443229"), ('l', b"96000669"),
    ('m', b"22222239"), ('n', b"22222239"), ('o', b"32222239"),
    ('p', b"22222239"), ('r', b"92232229"), ('s', b"32222259"),
    ('t', b"92200229"), ('u', b"22666229"), ('w', b"22626229"),
    ('|', b"99900999"), ('~', b"32223439"),
];

pub(crate) fn glyph(character: char) -> Option<&'static PixelFire> {
    static FIRE: OnceLock<HashMap<char, PixelFire>> = OnceLock::new();
    FIRE.get_or_init(|| EDGES.iter().map(|&(character, edge)| {
        let profile = edge.map(|row| (row != b'9').then_some((row - b'0') as usize));
        (character, PixelFire::new(&profile, character as u32 + 71))
    }).collect()).get(&character)
}

pub(crate) fn font(size: f32) -> rune::tok::TextStyle {
    rune::tok::TextStyle::new(size, 400.0).family(crate::ui::theme::PIXEL).lh(size)
}

/// The same supersampled live texture for activity and one-shot goal lettering.
pub(crate) fn rgba(fire: &PixelFire, time: f32, colors: [u32; 5]) -> Vec<u8> {
    let mut rgba = vec![0; 40 * 88 * 4];
    fire.cells(time, colors, |x, y, color| {
        let (x, y) = ((x + 1.0) as usize * 4, (y + 13.0) as usize * 4);
        for row in y..y + 4 {
            for column in x..x + 4 {
                let index = (row * 40 + column) * 4;
                rgba[index..index + 4].copy_from_slice(&[(color >> 16) as u8, (color >> 8) as u8, color as u8, 255]);
            }
        }
    });
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flames_touch_each_glyph_outline_without_burning_empty_columns() {
        for state in [super::super::State::Compacting, super::super::State::Thinking, super::super::State::Working,
            super::super::State::Generating, super::super::State::WaitingSubagents, super::super::State::Background] {
            for character in state.label().chars().chain(super::super::SCRAMBLE.iter().map(|&c| c as char)) {
                if character == ' ' { assert!(glyph(character).is_none()); continue; }
                let edge = EDGES.iter().find(|(c, _)| *c == character).unwrap().1;
                for frame in 0..32 {
                    let mut roots = [None; 8];
                    glyph(character).unwrap().cells(frame as f32 / 16.0, crate::ui::fire::CLASSIC, |x, y, _| {
                        let root = &mut roots[x as usize];
                        *root = Some(root.map_or(y as i32, |previous: i32| previous.max(y as i32)));
                    });
                    assert_eq!(roots, edge.map(|row| (row != b'9').then_some((row - b'0') as i32 - 1)), "{character}, frame {frame}");
                }
            }
        }
    }
}