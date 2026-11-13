#!/usr/bin/env python3
"""Generate Rune palettes directly from the shared Rust catalogs; --check never writes."""
import difflib
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
SHARED = ROOT / "neoism-frontend/shared/src/primitives"
TARGET = ROOT / "rune/app/src/ui/palettes.rs"


def generate():
    source = (SHARED / "ide_theme.rs").read_text()
    palettes = {}
    for name in ("pastel_dark", "nvchad_one", "tokyo_night", "catppuccin_mocha"):
        body = re.search(r"pub fn " + name + r"\(\) -> Self \{(.*?)\n    \}", source, re.S)[1]
        palettes[name] = dict(re.findall(r"(\w+): 0x([0-9a-f]+),", body))
        assert len(palettes[name]) == 27, name
    keys = list(palettes["pastel_dark"])
    packed_keys = [key for key in keys if key not in ("white", "black")]
    packed_keys.insert(15, "black")
    packed = (SHARED / "nvchad_themes.rs").read_text()
    for name, colors in re.findall(r'\("([^"]+)", "([a-f\d]+)"\)', packed):
        palette = {key: colors[i * 6:i * 6 + 6] for i, key in enumerate(packed_keys)}
        palette["white"] = palette["fg"]
        palettes.setdefault(name, palette)
    # The old default remains a lookup alias, not a duplicate picker entry.
    palettes = {"neoism_dark": palettes.pop("pastel_dark"), "neoism_light": palettes["one_light"], **palettes}
    header = (
        '//! Generated from the shared Rust theme catalogs by `rune/generate-palettes.py`.\n'
        '//! Regenerate rather than editing by hand.\n\n'
        '#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub struct Palette {\n'
        "    pub name: &'static str,\n"
        + ''.join(f'    pub {key}: u32,\n' for key in keys)
        + '}\n\npub const PALETTES: &[Palette] = &[\n'
    )
    rows = [f'    Palette {{ name: "{name}", ' + ', '.join(f'{key}: 0x{palette[key]}' for key in keys) + ' },\n' for name, palette in palettes.items()]
    return header + ''.join(rows) + '];\n'


if __name__ == "__main__":
    generated = generate()
    if "--check" in sys.argv:
        current = TARGET.read_text()
        if current != generated:
            sys.stdout.writelines(difflib.unified_diff(current.splitlines(True), generated.splitlines(True), fromfile=str(TARGET), tofile="generated"))
            sys.exit(1)
        print("Rune palettes match shared catalogs (Neoism aliases included).")
    else:
        TARGET.write_text(generated)