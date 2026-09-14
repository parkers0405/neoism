import type { Theme } from "../generated/themes";
import "./theme-preview.css";

// Keep the sample and token boundaries in parity with native
// panels/command_palette/render.rs::draw_theme_preview.
const sample: [string, string][][] = [
    [["pub ", "syn_keyword"], ["struct ", "syn_statement"], ["Theme ", "syn_type"], ["{", "fg"]],
    [["    name", "syn_property"], [": ", "fg"], ["String", "syn_type"], [",", "fg"]],
    [["}", "fg"]],
    [["// Live syntax preview", "syn_comment"]],
    [["let ", "syn_keyword"], ["theme", "fg"], [" = ", "fg"], ["Theme", "syn_constructor"], ["::", "fg"], ["load", "syn_func"], ['("neoism"', "syn_string"], [");", "fg"]],
    [["if ", "syn_keyword"], ["theme", "fg"], [".", "fg"], ["contrast", "syn_property"], [" > ", "fg"], ["4.5", "syn_number"], [" {", "fg"]],
    [["    render", "syn_func"], ["(&", "fg"], ["theme", "fg"], [");", "fg"]],
    [["}", "fg"]],
];

export function ThemePreview({ theme }: { theme: Theme }) {
    const c = theme.colors;
    return (
        <section className="theme-preview" aria-label={`Preview of ${theme.name}`}
            style={{ backgroundColor: c.bg, color: c.fg, borderColor: c.border }}>
            <header style={{ borderColor: c.border }}>
                <h3>{theme.name}</h3>
                <span style={{ color: c.muted }}>↑↓ preview · Enter apply</span>
            </header>
            <pre><code>{sample.map((line, row) => (
                <span key={row}>{line.map(([text, token], index) => (
                    <span key={index} data-token={token} style={{ color: c[token] }}>{text}</span>
                ))}{row < sample.length - 1 ? "\n" : ""}</span>
            ))}</code></pre>
        </section>
    );
}
