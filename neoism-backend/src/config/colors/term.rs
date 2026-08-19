use crate::config::colors::{ColorArray, ColorBuilder, Format};
use crate::config::Colors;
use neoism_terminal_core::colors::term::{COUNT, DIM_FACTOR};
use neoism_terminal_core::colors::{ColorRgb, NamedColor};
use std::ops::{Index, IndexMut};

// Phase 3b: the per-terminal palette storage type `TermColors` moved
// into `neoism-terminal-core::colors::term`. Backend keeps the
// renderer-side `List` (which folds the configured theme together
// with `TermColors`).

#[derive(Copy, Debug, Clone)]
pub struct List([ColorArray; COUNT]);

impl Default for List {
    fn default() -> Self {
        List([ColorArray::default(); COUNT])
    }
}

impl From<&Colors> for List {
    fn from(colors: &Colors) -> List {
        // Type inference fails without this annotation.
        let mut list = List([ColorArray::default(); COUNT]);

        list.fill_named(colors);
        list.fill_cube();
        list.fill_gray_ramp();

        list
    }
}

impl List {
    pub fn fill_named(&mut self, colors: &Colors) {
        self[NamedColor::Black] = colors.black;
        self[NamedColor::Red] = colors.red;
        self[NamedColor::Green] = colors.green;
        self[NamedColor::Yellow] = colors.yellow;
        self[NamedColor::Blue] = colors.blue;
        self[NamedColor::Magenta] = colors.magenta;
        self[NamedColor::Cyan] = colors.cyan;
        self[NamedColor::White] = colors.white;

        // Lights.
        self[NamedColor::LightBlack] = colors.light_black;
        self[NamedColor::LightRed] = colors.light_red;
        self[NamedColor::LightGreen] = colors.light_green;
        self[NamedColor::LightYellow] = colors.light_yellow;
        self[NamedColor::LightBlue] = colors.light_blue;
        self[NamedColor::LightMagenta] = colors.light_magenta;
        self[NamedColor::LightCyan] = colors.light_cyan;
        self[NamedColor::LightWhite] = colors.light_white;

        if let Some(color) = colors.light_foreground {
            self[NamedColor::LightForeground] = color;
        } else {
            self[NamedColor::LightForeground] =
                (ColorRgb::from_color_arr(colors.foreground)).to_arr();
        }

        // Foreground and background.
        self[NamedColor::Foreground] = colors.foreground;
        self[NamedColor::Background] = colors.background.0;

        // Dims.
        if let Some(color) = colors.dim_foreground {
            self[NamedColor::DimForeground] = color;
        } else {
            self[NamedColor::DimForeground] =
                (ColorRgb::from_color_arr(colors.foreground) * DIM_FACTOR).to_arr();
        }

        if let Some(color) = colors.dim_black {
            self[NamedColor::DimBlack] = color;
        } else {
            self[NamedColor::DimBlack] =
                (ColorRgb::from_color_arr(colors.black) * DIM_FACTOR).to_arr();
        }

        if let Some(color) = colors.dim_red {
            self[NamedColor::DimRed] = color;
        } else {
            self[NamedColor::DimRed] =
                (ColorRgb::from_color_arr(colors.red) * DIM_FACTOR).to_arr();
        }

        if let Some(color) = colors.dim_green {
            self[NamedColor::DimGreen] = color;
        } else {
            self[NamedColor::DimGreen] =
                (ColorRgb::from_color_arr(colors.green) * DIM_FACTOR).to_arr();
        }

        if let Some(color) = colors.dim_yellow {
            self[NamedColor::DimYellow] = color;
        } else {
            self[NamedColor::DimYellow] =
                (ColorRgb::from_color_arr(colors.yellow) * DIM_FACTOR).to_arr();
        }

        if let Some(color) = colors.dim_blue {
            self[NamedColor::DimBlue] = color;
        } else {
            self[NamedColor::DimBlue] =
                (ColorRgb::from_color_arr(colors.blue) * DIM_FACTOR).to_arr();
        }

        if let Some(color) = colors.dim_magenta {
            self[NamedColor::DimMagenta] = color;
        } else {
            self[NamedColor::DimMagenta] =
                (ColorRgb::from_color_arr(colors.magenta) * DIM_FACTOR).to_arr();
        }

        if let Some(color) = colors.dim_cyan {
            self[NamedColor::DimCyan] = color;
        } else {
            self[NamedColor::DimCyan] =
                (ColorRgb::from_color_arr(colors.cyan) * DIM_FACTOR).to_arr();
        }

        if let Some(color) = colors.dim_white {
            self[NamedColor::DimWhite] = color;
        } else {
            self[NamedColor::DimWhite] =
                (ColorRgb::from_color_arr(colors.white) * DIM_FACTOR).to_arr();
        }
    }

    /// Fold this resolved theme palette into the shape
    /// `Crosswords::set_default_colors` expects, so the terminal can
    /// answer OSC 4/10/11/12 color queries synchronously at parse
    /// time. Every slot is seeded except `NamedColor::Cursor`:
    /// cursor-color queries historically reply only when the guest
    /// program set an override (mirroring xterm-ignores-unknown and
    /// the old renderer-side behavior), and `List` never resolves a
    /// real cursor color — seeding it would answer `OSC 12;?` with
    /// black.
    pub fn as_default_colors(&self) -> neoism_terminal_core::colors::term::TermColors {
        let mut colors = neoism_terminal_core::colors::term::TermColors::default();
        for (index, value) in self.0.iter().enumerate() {
            if index == NamedColor::Cursor as usize {
                continue;
            }
            colors[index] = Some(*value);
        }
        colors
    }

    pub fn fill_cube(&mut self) {
        let mut index: usize = 16;
        // Build colors.
        for r in 0..6 {
            for g in 0..6 {
                for b in 0..6 {
                    let rgb = ColorRgb {
                        r: if r == 0 { 0 } else { r * 40 + 55 },
                        b: if b == 0 { 0 } else { b * 40 + 55 },
                        g: if g == 0 { 0 } else { g * 40 + 55 },
                    };

                    let arr = ColorBuilder::from_rgb(rgb, Format::SRGB0_1).to_arr();
                    self[index] = arr;
                    index += 1;
                }
            }
        }

        debug_assert!(index == 232);
    }

    pub fn fill_gray_ramp(&mut self) {
        let mut index: usize = 232;

        for i in 0..24 {
            let value = i * 10 + 8;
            let rgb = ColorRgb {
                r: value,
                g: value,
                b: value,
            };
            let arr = ColorBuilder::from_rgb(rgb, Format::SRGB0_1).to_arr();
            self[index] = arr;
            index += 1;
        }

        debug_assert!(index == 256);
    }
}

impl Index<usize> for List {
    type Output = ColorArray;

    #[inline]
    fn index(&self, idx: usize) -> &Self::Output {
        &self.0[idx]
    }
}

impl IndexMut<usize> for List {
    #[inline]
    fn index_mut(&mut self, idx: usize) -> &mut Self::Output {
        &mut self.0[idx]
    }
}

impl Index<NamedColor> for List {
    type Output = ColorArray;

    #[inline]
    fn index(&self, idx: NamedColor) -> &Self::Output {
        &self.0[idx as usize]
    }
}

impl IndexMut<NamedColor> for List {
    #[inline]
    fn index_mut(&mut self, idx: NamedColor) -> &mut Self::Output {
        &mut self.0[idx as usize]
    }
}
