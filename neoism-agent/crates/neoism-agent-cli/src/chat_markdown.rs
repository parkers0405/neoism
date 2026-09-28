use std::io::Write;

use serde::Deserialize;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::chat_terminal::terminal_size;
use crate::{BOLD, CYAN, DIM, GREEN, ITALIC, ORANGE, PURPLE, RESET, WHITE, YELLOW};

const BORDER_H: &str = "─";
const STRIKE: &str = "\x1b[9m";
const UNDERLINE: &str = "\x1b[4m";

const MARKDOWN_LINK_TEXT: &str = "\x1b[38;2;86;182;194m";
const MARKDOWN_LINK_URL: &str = "\x1b[38;2;92;156;245m";
const MARKDOWN_CODE: &str = "\x1b[38;2;127;216;143m";

const SYNTAX_COMMENT: &str = "\x1b[38;2;127;132;142m";
const SYNTAX_KEYWORD: &str = "\x1b[38;2;198;120;221m";
const SYNTAX_FUNCTION: &str = "\x1b[38;2;97;175;239m";
const SYNTAX_STRING: &str = "\x1b[38;2;152;195;121m";
const SYNTAX_NUMBER: &str = "\x1b[38;2;229;192;123m";
const SYNTAX_TYPE: &str = "\x1b[38;2;86;182;194m";
const SYNTAX_OPERATOR: &str = "\x1b[38;2;86;182;194m";
const SYNTAX_PROPERTY: &str = "\x1b[38;2;224;108;117m";
const SYNTAX_CONSTANT: &str = "\x1b[38;2;209;154;102m";
const SYNTAX_PUNCTUATION: &str = "\x1b[38;2;171;178;191m";
const SYNTAX_ESCAPE: &str = "\x1b[38;2;86;182;194m";
const COMMENT_TODO: &str = "\x1b[38;2;229;192;123m";
const COMMENT_ERROR: &str = "\x1b[38;2;224;108;117m";

const TREE_SITTER_HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "character",
    "character.special",
    "comment",
    "comment.documentation",
    "comment.error",
    "comment.note",
    "comment.todo",
    "comment.warning",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "field",
    "function",
    "function.builtin",
    "function.call",
    "function.method",
    "function.method.call",
    "keyword.coroutine",
    "keyword",
    "keyword.conditional",
    "keyword.conditional.ternary",
    "keyword.directive",
    "keyword.exception",
    "keyword.export",
    "keyword.function",
    "keyword.import",
    "keyword.modifier",
    "keyword.operator",
    "keyword.repeat",
    "keyword.return",
    "keyword.type",
    "module",
    "module.builtin",
    "namespace",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.special",
    "string.regexp",
    "symbol",
    "tag",
    "tag.attribute",
    "tag.delimiter",
    "type",
    "type.builtin",
    "type.definition",
    "annotation",
    "variable",
    "variable.builtin",
    "variable.member",
    "variable.parameter",
    "variable.super",
];

#[derive(Default)]
pub(crate) struct MarkdownStreamRenderer {
    line: String,
    code_lang: Option<String>,
    code_line: usize,
    code_buffer: Vec<String>,
    streaming: bool,
    first_line_prefix: Option<String>,
    line_style: Option<String>,
    pending_table_header: Option<String>,
    in_table: bool,
}

impl MarkdownStreamRenderer {
    pub(crate) fn set_first_line_prefix(&mut self, prefix: String) {
        self.first_line_prefix = Some(prefix);
    }

    pub(crate) fn set_line_style(&mut self, style: String) {
        self.line_style = Some(style);
    }

    pub(crate) fn push(&mut self, delta: &str) -> anyhow::Result<()> {
        for ch in delta.chars() {
            if ch == '\n' {
                self.flush_line()?;
            } else {
                // Emit the bullet/prefix once, right before the very first streamed char.
                if !self.streaming
                    && self.line.is_empty()
                    && self.code_lang.is_none()
                    && self.first_line_prefix.is_some()
                {
                    if let Some(prefix) = self.first_line_prefix.as_deref() {
                        print!("{prefix}");
                    }
                }
                self.line.push(ch);
                if self.code_lang.is_none() && !self.line.trim_start().starts_with("```")
                {
                    if let Some(style) = self.line_style.as_deref() {
                        print!("{style}{ch}{RESET}");
                    } else {
                        print!("{ch}");
                    }
                    std::io::stdout().flush()?;
                    self.streaming = true;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> anyhow::Result<()> {
        if !self.line.is_empty() {
            self.flush_line()?;
        }
        self.flush_pending_table_header("")?;
        Ok(())
    }

    fn flush_line(&mut self) -> anyhow::Result<()> {
        let line = std::mem::take(&mut self.line);
        let trimmed = line.trim_start();
        let was_streaming = std::mem::take(&mut self.streaming);
        let prefix = self.first_line_prefix.take();
        let prefix_str = prefix.as_deref().unwrap_or("");

        if let Some(header) = self.pending_table_header.take() {
            if is_table_delimiter(trimmed) {
                if was_streaming {
                    print!("\r\x1b[K");
                }
                self.in_table = true;
                let header_cells = parse_table_row(&header).unwrap_or_default();
                println!(
                    "{prefix_str}{}",
                    self.render_line(&render_table_row(&header_cells, true))
                );
                println!(
                    "{prefix_str}{}",
                    self.render_line(&render_table_rule(&header_cells))
                );
                std::io::stdout().flush()?;
                return Ok(());
            }
            self.in_table = false;
            println!(
                "{prefix_str}{}",
                self.render_line(&render_markdown_line(&header))
            );
            self.render_regular_line(&line, trimmed, was_streaming, "")?;
            return Ok(());
        }

        if self.code_lang.is_none() {
            if let Some(cells) = parse_table_row(&line) {
                if self.in_table {
                    if was_streaming {
                        print!("\r\x1b[K");
                    }
                    println!(
                        "{prefix_str}{}",
                        self.render_line(&render_table_row(&cells, false))
                    );
                    std::io::stdout().flush()?;
                    return Ok(());
                }
                if !is_table_delimiter(trimmed) {
                    if was_streaming {
                        print!("\r\x1b[K");
                    }
                    self.pending_table_header = Some(line);
                    std::io::stdout().flush()?;
                    return Ok(());
                }
            } else {
                self.in_table = false;
            }
        }

        self.render_regular_line(&line, trimmed, was_streaming, prefix_str)
    }

    fn flush_pending_table_header(&mut self, prefix_str: &str) -> anyhow::Result<()> {
        if let Some(header) = self.pending_table_header.take() {
            self.in_table = false;
            println!(
                "{prefix_str}{}",
                self.render_line(&render_markdown_line(&header))
            );
            std::io::stdout().flush()?;
        }
        Ok(())
    }

    fn render_regular_line(
        &mut self,
        line: &str,
        trimmed: &str,
        was_streaming: bool,
        prefix_str: &str,
    ) -> anyhow::Result<()> {
        if let Some(lang) = trimmed.strip_prefix("```") {
            if was_streaming {
                print!("\r\x1b[K");
            }
            if self.code_lang.is_some() {
                if let Some(rendered) = self.render_special_code_block(prefix_str) {
                    print!("{rendered}");
                } else if self
                    .code_lang
                    .as_deref()
                    .is_some_and(|lang| is_special_suppressed_code_lang(lang))
                {
                    print!("{}", self.render_buffered_code_block(prefix_str));
                } else {
                    println!("{prefix_str}{DIM}  ╰{}╯{RESET}", BORDER_H.repeat(2));
                }
                self.code_lang = None;
                self.code_line = 0;
                self.code_buffer.clear();
            } else {
                let lang = lang.trim().to_string();
                self.code_lang = Some(lang.clone());
                self.code_line = 1;
                self.code_buffer.clear();
                if !is_special_suppressed_code_lang(&lang) {
                    let label = if lang.is_empty() {
                        "code".to_string()
                    } else {
                        lang.clone()
                    };
                    println!(
                        "{prefix_str}{DIM}  ╭─ {RESET}{CYAN}{label}{RESET}{DIM} {}{RESET}",
                        BORDER_H.repeat(1)
                    );
                }
            }
            std::io::stdout().flush()?;
            return Ok(());
        }
        if let Some(lang) = self.code_lang.as_deref() {
            self.code_buffer.push(line.to_string());
            if !is_special_suppressed_code_lang(lang) {
                println!(
                    "{prefix_str}{DIM}  │{RESET}{DIM}{:>4}{RESET}{DIM} │{RESET} {}",
                    self.code_line,
                    self.render_line(&highlight_code_line(lang, &line))
                );
            }
            self.code_line += 1;
        } else if was_streaming {
            let (term_width, _) = terminal_size();
            clear_streamed_physical_rows(
                ansi_visible_width(prefix_str) + line.chars().count(),
                term_width as usize,
            );
            print!("{prefix_str}");
            println!("{}", self.render_line(&render_markdown_line(&line)));
        } else {
            println!(
                "{prefix_str}{}",
                self.render_line(&render_markdown_line(&line))
            );
        }
        std::io::stdout().flush()?;
        Ok(())
    }

    fn render_line(&self, line: &str) -> String {
        if let Some(style) = self.line_style.as_deref() {
            apply_line_style(line, style)
        } else {
            line.to_string()
        }
    }

    fn render_special_code_block(&self, prefix_str: &str) -> Option<String> {
        let lang = self.code_lang.as_deref()?;
        if lang.eq_ignore_ascii_case("stock") {
            let spec: CliStockSpec =
                serde_json::from_str(self.code_buffer.join("\n").as_str()).ok()?;
            return Some(render_stock_fallback(&spec, prefix_str));
        }
        if lang.eq_ignore_ascii_case("mermaid") {
            return Some(render_mermaid_fallback(&self.code_buffer, prefix_str));
        }
        None
    }

    fn render_buffered_code_block(&self, prefix_str: &str) -> String {
        let lang = self.code_lang.as_deref().unwrap_or_default();
        let label = if lang.trim().is_empty() { "code" } else { lang };
        let mut out = String::new();
        out.push_str(&format!(
            "{prefix_str}{DIM}  ╭─ {RESET}{CYAN}{label}{RESET}{DIM} {}{RESET}\n",
            BORDER_H.repeat(1)
        ));
        for (ix, line) in self.code_buffer.iter().enumerate() {
            out.push_str(&format!(
                "{prefix_str}{DIM}  │{RESET}{DIM}{:>4}{RESET}{DIM} │{RESET} {}\n",
                ix + 1,
                self.render_line(&highlight_code_line(lang, line))
            ));
        }
        out.push_str(&format!(
            "{prefix_str}{DIM}  ╰{}╯{RESET}\n",
            BORDER_H.repeat(2)
        ));
        out
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliStockSpec {
    symbol: String,
    #[serde(default)]
    name: String,
    price: f64,
    #[serde(default = "default_currency")]
    currency: String,
    #[serde(default)]
    change: Option<f64>,
    #[serde(default)]
    change_percent: Option<f64>,
    #[serde(default)]
    period: Option<String>,
    #[serde(default)]
    range: Option<String>,
    #[serde(default)]
    stats: CliStockStats,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliStockStats {
    open: Option<String>,
    day_low: Option<String>,
    day_high: Option<String>,
    volume: Option<String>,
    market_cap: Option<String>,
    pe_ratio: Option<String>,
}

fn default_currency() -> String {
    "USD".to_string()
}

fn is_special_suppressed_code_lang(lang: &str) -> bool {
    lang.eq_ignore_ascii_case("stock") || lang.eq_ignore_ascii_case("mermaid")
}

fn render_mermaid_fallback(lines: &[String], prefix_str: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("{prefix_str}{BOLD}{CYAN}Mermaid diagram{RESET}\n"));
    for line in lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
    {
        if line.starts_with("%%") {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("graph") || lower.starts_with("flowchart") {
            out.push_str(&format!("{prefix_str}{DIM}{line}{RESET}\n"));
            continue;
        }
        let rendered = line
            .replace("-->", " -> ")
            .replace("---", " - ")
            .replace('|', " ")
            .replace('[', " ")
            .replace(']', " ")
            .replace('{', " ")
            .replace('}', " ")
            .replace('(', " ")
            .replace(')', " ");
        out.push_str(&format!(
            "{prefix_str}{DIM}  •{RESET} {}\n",
            rendered.trim()
        ));
    }
    out
}

fn render_stock_fallback(spec: &CliStockSpec, prefix_str: &str) -> String {
    let mut out = String::new();
    let title = if spec.name.trim().is_empty() {
        spec.symbol.trim().to_string()
    } else {
        format!("{} ({})", spec.name.trim(), spec.symbol.trim())
    };
    out.push_str(&format!("{prefix_str}{BOLD}{WHITE}{title}{RESET}\n"));
    out.push_str(&format!(
        "{prefix_str}{BOLD}{WHITE}{}{RESET}\n",
        format_price(spec.price, spec.currency.as_str())
    ));
    let positive = spec.change.unwrap_or(0.0) >= 0.0;
    let color = if positive { GREEN } else { ORANGE };
    out.push_str(&format!(
        "{prefix_str}{color}{}{RESET}\n",
        format_change(spec)
    ));
    if let Some(range) = spec
        .range
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        out.push_str(&format!("{prefix_str}{DIM}Range {range}{RESET}\n"));
    }
    let stats = [
        ("Open", spec.stats.open.as_deref()),
        ("Day Low", spec.stats.day_low.as_deref()),
        ("Day High", spec.stats.day_high.as_deref()),
        ("Volume", spec.stats.volume.as_deref()),
        ("Market Cap", spec.stats.market_cap.as_deref()),
        ("P/E", spec.stats.pe_ratio.as_deref()),
    ];
    let stat_line = stats
        .into_iter()
        .filter_map(|(label, value)| {
            let value = value?.trim();
            (!value.is_empty()).then(|| format!("{label} {value}"))
        })
        .collect::<Vec<_>>()
        .join(" · ");
    if !stat_line.is_empty() {
        out.push_str(&format!("{prefix_str}{DIM}{stat_line}{RESET}\n"));
    }
    out
}

fn format_price(price: f64, currency: &str) -> String {
    let prefix = match currency.trim().to_ascii_uppercase().as_str() {
        "USD" | "CAD" | "AUD" => "$",
        "GBP" => "£",
        "EUR" => "€",
        "JPY" => "¥",
        _ => "",
    };
    format!("{prefix}{price:.2}")
}

fn format_change(spec: &CliStockSpec) -> String {
    let change = spec.change.unwrap_or(0.0);
    let pct = spec.change_percent.unwrap_or(0.0);
    let sign = if change >= 0.0 { "+" } else { "" };
    let period = spec.period.as_deref().unwrap_or("Today");
    format!("{sign}{change:.2} ({sign}{pct:.2}%) · {period}")
}

fn clear_streamed_physical_rows(visible_width: usize, terminal_width: usize) {
    let width = terminal_width.max(1);
    let rows = visible_width.saturating_sub(1) / width + 1;
    print!("\r\x1b[K");
    for _ in 1..rows {
        print!("\x1b[1A\r\x1b[K");
    }
}

pub(crate) fn ansi_visible_width(s: &str) -> usize {
    let mut count = 0;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if let Some(&'[') = chars.peek() {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            count += 1;
        }
    }
    count
}

fn render_markdown_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent = &line[..line.len().saturating_sub(trimmed.len())];
    if let Some((level, rest)) = heading(trimmed) {
        let color = match level {
            1 => CYAN,
            2 => PURPLE,
            _ => ORANGE,
        };
        return format!(
            "{indent}{BOLD}{color}{} {RESET}{BOLD}{color}{}{RESET}",
            "#".repeat(level),
            inline_code(rest)
        );
    }
    if trimmed == "---" || trimmed == "***" || trimmed == "___" {
        let (width, _) = terminal_size();
        let len = (width as usize).saturating_sub(4).min(60);
        return format!("{indent}{DIM}{}{RESET}", BORDER_H.repeat(len));
    }
    if let Some(rest) = trimmed.strip_prefix("> ") {
        return format!(
            "{indent}{YELLOW}│ {RESET}{DIM}{ITALIC}{}{RESET}",
            inline_code(rest)
        );
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            if let Some(task) = checkbox_marker(rest) {
                let (mark, body) = task;
                return format!("{indent}  {GREEN}{mark}{RESET} {}", inline_code(body));
            }
            return format!("{indent}{CYAN}•{RESET} {}", inline_code(rest));
        }
    }
    if let Some((number, rest)) = ordered_list(trimmed) {
        return format!(
            "{indent}{MARKDOWN_LINK_TEXT}{number}{RESET}{}",
            inline_code(rest)
        );
    }
    if trimmed.is_empty() {
        String::new()
    } else {
        inline_code(line)
    }
}

fn checkbox_marker(rest: &str) -> Option<(&'static str, &str)> {
    if let Some(after) = rest.strip_prefix("[ ] ") {
        return Some(("☐", after));
    }
    if let Some(after) = rest
        .strip_prefix("[x] ")
        .or_else(|| rest.strip_prefix("[X] "))
    {
        return Some(("☑", after));
    }
    None
}

fn ordered_list(line: &str) -> Option<(&str, &str)> {
    let dot = line.find(". ")?;
    if dot == 0 || !line[..dot].chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some((&line[..dot + 2], &line[dot + 2..]))
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let marker = line.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&marker) {
        return None;
    }
    line.get(marker..)
        .and_then(|rest| rest.strip_prefix(' '))
        .map(|rest| (marker, rest))
}

fn parse_table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }
    let trimmed = trimmed.trim_matches('|');
    let cells = split_table_cells(trimmed)
        .into_iter()
        .map(|cell| cell.trim().to_string())
        .collect::<Vec<_>>();
    if cells.len() < 2 || cells.iter().all(|cell| cell.is_empty()) {
        None
    } else {
        Some(cells)
    }
}

fn split_table_cells(row: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for ch in row.chars() {
        if escaped {
            cell.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            cell.push(ch);
            escaped = true;
            continue;
        }
        if ch == '|' {
            cells.push(std::mem::take(&mut cell));
        } else {
            cell.push(ch);
        }
    }
    cells.push(cell);
    cells
}

fn is_table_delimiter(line: &str) -> bool {
    let Some(cells) = parse_table_row(line) else {
        return false;
    };
    cells.iter().all(|cell| {
        let marker = cell.trim();
        let marker = marker.strip_prefix(':').unwrap_or(marker);
        let marker = marker.strip_suffix(':').unwrap_or(marker);
        marker.len() >= 3 && marker.chars().all(|ch| ch == '-')
    })
}

fn render_table_row(cells: &[String], header: bool) -> String {
    let mut output = format!("{DIM}│{RESET}");
    for cell in cells {
        if header {
            output.push_str(&format!(
                " {BOLD}{CYAN}{}{RESET} {DIM}│{RESET}",
                inline_code(cell)
            ));
        } else {
            output.push_str(&format!(" {} {DIM}│{RESET}", inline_code(cell)));
        }
    }
    output
}

fn render_table_rule(cells: &[String]) -> String {
    let mut output = format!("{DIM}├");
    for (index, cell) in cells.iter().enumerate() {
        let width = ansi_visible_width(cell).max(3) + 2;
        output.push_str(&BORDER_H.repeat(width));
        if index + 1 == cells.len() {
            output.push('┤');
        } else {
            output.push('┼');
        }
    }
    output.push_str(RESET);
    output
}

const INLINE_CODE_BLUE: &str = MARKDOWN_CODE;

fn inline_code(line: &str) -> String {
    let mut output = String::new();
    let mut in_code = false;
    for chunk in line.split('`') {
        if in_code {
            output.push_str(INLINE_CODE_BLUE);
            output.push_str(chunk);
            output.push_str(RESET);
        } else {
            output.push_str(&inline_emphasis(chunk));
        }
        in_code = !in_code;
    }
    output
}

fn inline_emphasis(line: &str) -> String {
    let linked = inline_links(line);
    let bold = replace_delimited(&linked, "**", &format!("{BOLD}{WHITE}"), RESET);
    let strike = replace_delimited(&bold, "~~", STRIKE, RESET);
    replace_delimited(&strike, "*", &format!("{ITALIC}{YELLOW}"), RESET)
}

fn inline_links(line: &str) -> String {
    let mut output = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('[') {
        let after_start = &rest[start + 1..];
        let Some(label_end) = after_start.find("](") else {
            break;
        };
        let url_start = start + 1 + label_end + 2;
        let Some(url_end) = rest[url_start..].find(')') else {
            break;
        };
        let label = &rest[start + 1..start + 1 + label_end];
        let url = &rest[url_start..url_start + url_end];
        output.push_str(&rest[..start]);
        output.push_str(UNDERLINE);
        output.push_str(MARKDOWN_LINK_TEXT);
        output.push_str(label);
        output.push_str(RESET);
        if !url.is_empty() {
            output.push_str(DIM);
            output.push_str(" (");
            output.push_str(UNDERLINE);
            output.push_str(MARKDOWN_LINK_URL);
            output.push_str(url);
            output.push_str(RESET);
            output.push_str(DIM);
            output.push(')');
            output.push_str(RESET);
        }
        rest = &rest[url_start + url_end + 1..];
    }
    output.push_str(rest);
    output
}

fn replace_delimited(input: &str, marker: &str, open: &str, close: &str) -> String {
    let mut output = String::new();
    let mut rest = input;
    loop {
        let Some(start) = rest.find(marker) else {
            break;
        };
        let content_start = start + marker.len();
        let Some(end) = rest[content_start..].find(marker) else {
            break;
        };
        output.push_str(&rest[..start]);
        output.push_str(open);
        output.push_str(&rest[content_start..content_start + end]);
        output.push_str(close);
        rest = &rest[content_start + end + marker.len()..];
    }
    output.push_str(rest);
    output
}

fn apply_line_style(line: &str, style: &str) -> String {
    if style.is_empty() || line.is_empty() {
        return line.to_string();
    }
    let mut output = String::with_capacity(line.len() + style.len() + RESET.len());
    output.push_str(style);
    let mut rest = line;
    while let Some(index) = rest.find(RESET) {
        let end = index + RESET.len();
        output.push_str(&rest[..end]);
        rest = &rest[end..];
        if !rest.is_empty() {
            output.push_str(style);
        }
    }
    output.push_str(rest);
    output.push_str(RESET);
    output
}

pub(crate) fn truncate_for_terminal(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_style_is_reapplied_after_inline_markdown_resets() {
        let rendered = render_markdown_line("keep **bold** dim");
        let styled = apply_line_style(&rendered, DIM);

        assert!(styled.starts_with(DIM));
        assert!(styled.contains(&format!("{RESET}{DIM} dim")));
        assert!(styled.ends_with(RESET));
    }

    #[test]
    fn markdown_tables_are_detected_and_rendered() {
        assert!(is_table_delimiter("| --- | :---: | ---: |"));
        let cells = parse_table_row("| Name | Status |").unwrap();
        let header = render_table_row(&cells, true);

        assert!(header.contains("Name"));
        assert!(header.contains("Status"));
        assert!(render_table_rule(&cells).contains('┼'));
    }

    #[test]
    fn inline_markdown_handles_common_emphasis_and_links() {
        let rendered =
            render_markdown_line("**bold** *ital* ~~gone~~ [site](https://example.com)");

        assert!(rendered.contains(BOLD));
        assert!(rendered.contains(ITALIC));
        assert!(rendered.contains(STRIKE));
        assert!(rendered.contains(UNDERLINE));
        assert!(rendered.contains(MARKDOWN_LINK_TEXT));
        assert!(rendered.contains(MARKDOWN_LINK_URL));
        assert!(rendered.contains("https://example.com"));
    }

    #[test]
    fn inline_markdown_handles_numeric_bold_ranges() {
        let rendered = render_markdown_line("**250 to 400+ years**");

        assert!(rendered.contains(BOLD));
        assert!(rendered.contains("250 to 400+ years"));
        assert!(!rendered.contains("**250"));
    }

    #[test]
    fn inline_code_uses_plain_light_blue_without_background() {
        let rendered = render_markdown_line("run `cargo test --workspace` now");

        assert!(rendered
            .contains(&format!("{INLINE_CODE_BLUE}cargo test --workspace{RESET}")));
        assert!(!rendered.contains("\x1b[48;"));
        assert!(!rendered.contains(" cargo test --workspace "));
    }

    #[test]
    fn code_highlighting_uses_language_token_buckets() {
        let rendered = highlight_code_line(
            "typescript",
            "const HTTP_OK = client.fetch<User>(url, true) // TODO: verify",
        );

        assert!(rendered.contains(&format!("{SYNTAX_KEYWORD}const{RESET}")));
        assert!(rendered.contains(&format!("{SYNTAX_TYPE}HTTP_OK{RESET}")));
        assert!(rendered.contains(&format!("{SYNTAX_FUNCTION}fetch{RESET}")));
        assert!(rendered.contains(&format!("{SYNTAX_TYPE}User{RESET}")));
        assert!(rendered.contains(&format!("{SYNTAX_NUMBER}true{RESET}")));
        assert!(rendered.contains(COMMENT_TODO));
    }

    #[test]
    fn code_highlighting_colors_operators_punctuation_and_escapes() {
        let rendered = highlight_code_line("rust", "let sum = left + right;");

        assert!(rendered.contains(&format!("{SYNTAX_KEYWORD}let{RESET}")));
        assert!(rendered.contains(SYNTAX_OPERATOR));
        assert!(rendered.contains(SYNTAX_PUNCTUATION));

        let string_rendered =
            highlight_code_line("rust", "let path = format!(\"a\\nb\");");
        assert!(string_rendered.contains(&format!("{SYNTAX_FUNCTION}format{RESET}")));
        assert!(string_rendered.contains(SYNTAX_ESCAPE));
    }

    #[test]
    fn mermaid_fallback_summarizes_diagram() {
        let rendered = render_mermaid_fallback(
            &["flowchart LR".into(), "A[Start] -->|go| B{Done}".into()],
            "",
        );

        assert!(rendered.contains("Mermaid diagram"));
        assert!(rendered.contains("flowchart LR"));
        assert!(rendered.contains("A Start"));
    }
}

pub(crate) fn highlight_code_line(lang: &str, line: &str) -> String {
    let normalized = lang.to_ascii_lowercase();
    if let Some(rendered) = tree_sitter_highlight_line(&normalized, line) {
        return rendered;
    }

    let trimmed = line.trim_start();
    if trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || (is_hash_comment_lang(&normalized) && trimmed.starts_with('#'))
    {
        return highlight_comment(line);
    }
    let keywords = keywords_for_lang(&normalized);
    syntax_highlight(&normalized, line, keywords)
}

fn tree_sitter_highlight_line(lang: &str, line: &str) -> Option<String> {
    let mut config = tree_sitter_config_for_lang(lang)?;
    config.configure(TREE_SITTER_HIGHLIGHT_NAMES);

    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(&config, line.as_bytes(), None, |_| None)
        .ok()?;

    let mut output = String::new();
    let mut active_styles: Vec<&'static str> = Vec::new();
    let mut active_captures: Vec<&'static str> = Vec::new();
    let mut had_highlight = false;

    for event in events {
        match event.ok()? {
            HighlightEvent::Source { start, end } => {
                let source = &line[start..end];
                if active_styles.is_empty() {
                    let rendered =
                        syntax_highlight(lang, source, keywords_for_lang(lang));
                    had_highlight |= rendered != source;
                    output.push_str(&rendered);
                } else if let Some(capture) = active_captures.last() {
                    if capture.starts_with("comment") {
                        output.push_str(&highlight_comment(source));
                    } else if matches!(*capture, "variable" | "variable.parameter") {
                        output.push_str(&syntax_highlight(
                            lang,
                            source,
                            keywords_for_lang(lang),
                        ));
                    } else {
                        output.push_str(source);
                    }
                } else {
                    output.push_str(source);
                }
            }
            HighlightEvent::HighlightStart(highlight) => {
                had_highlight = true;
                let capture = TREE_SITTER_HIGHLIGHT_NAMES
                    .get(highlight.0)
                    .copied()
                    .unwrap_or("variable");
                let style = style_for_tree_sitter_capture(capture).unwrap_or(WHITE);
                output.push_str(style);
                active_styles.push(style);
                active_captures.push(capture);
            }
            HighlightEvent::HighlightEnd => {
                if active_styles.pop().is_some() {
                    active_captures.pop();
                    output.push_str(RESET);
                    if let Some(style) = active_styles.last() {
                        output.push_str(style);
                    }
                }
            }
        }
    }

    if active_styles.is_empty() {
        if had_highlight {
            Some(output)
        } else {
            None
        }
    } else {
        output.push_str(RESET);
        Some(output)
    }
}

fn tree_sitter_config_for_lang(lang: &str) -> Option<HighlightConfiguration> {
    match lang {
        "rs" | "rust" => HighlightConfiguration::new(
            tree_sitter_rust::LANGUAGE.into(),
            "rust",
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            tree_sitter_rust::INJECTIONS_QUERY,
            "",
        )
        .ok(),
        "js" | "mjs" | "cjs" | "javascript" => HighlightConfiguration::new(
            tree_sitter_javascript::LANGUAGE.into(),
            "javascript",
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_javascript::LOCALS_QUERY,
        )
        .ok(),
        "jsx" | "javascriptreact" => HighlightConfiguration::new(
            tree_sitter_javascript::LANGUAGE.into(),
            "javascript",
            &format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
            ),
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_javascript::LOCALS_QUERY,
        )
        .ok(),
        "ts" | "typescript" => HighlightConfiguration::new(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "typescript",
            &format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_typescript::LOCALS_QUERY,
        )
        .ok(),
        "tsx" | "typescriptreact" => HighlightConfiguration::new(
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            "tsx",
            &format!(
                "{}\n{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_typescript::LOCALS_QUERY,
        )
        .ok(),
        _ => None,
    }
}

fn style_for_tree_sitter_capture(capture: &str) -> Option<&'static str> {
    let style = match capture {
        "comment.error" => format_style(BOLD, COMMENT_ERROR),
        "comment.todo" | "comment.note" | "comment.warning" => {
            format_style(BOLD, COMMENT_TODO)
        }
        "comment" | "comment.documentation" => format_style(SYNTAX_COMMENT, ITALIC),
        "string" | "symbol" | "character" => SYNTAX_STRING,
        "escape" | "string.escape" | "character.special" => SYNTAX_ESCAPE,
        "string.regexp" | "string.special" => SYNTAX_STRING,
        "number" | "boolean" => SYNTAX_NUMBER,
        "keyword.return"
        | "keyword.conditional"
        | "keyword.repeat"
        | "keyword.coroutine"
        | "keyword"
        | "keyword.import"
        | "keyword.function"
        | "keyword.directive"
        | "keyword.modifier"
        | "keyword.exception"
        | "keyword.export" => SYNTAX_KEYWORD,
        "keyword.type" => SYNTAX_TYPE,
        "operator" | "keyword.operator" | "keyword.conditional.ternary" => {
            SYNTAX_OPERATOR
        }
        "function"
        | "function.builtin"
        | "function.call"
        | "function.method"
        | "function.method.call" => SYNTAX_FUNCTION,
        "constructor" | "type" | "type.builtin" | "type.definition" | "class" => {
            SYNTAX_TYPE
        }
        "constant" => SYNTAX_CONSTANT,
        "constant.builtin" => SYNTAX_NUMBER,
        "property" | "field" | "variable.member" | "tag.attribute" | "attribute" => {
            SYNTAX_PROPERTY
        }
        "punctuation"
        | "punctuation.bracket"
        | "punctuation.delimiter"
        | "punctuation.special"
        | "tag.delimiter" => SYNTAX_PUNCTUATION,
        "tag" | "module" | "module.builtin" | "namespace" | "annotation" => SYNTAX_TYPE,
        "variable.builtin" | "variable.super" => SYNTAX_CONSTANT,
        "variable" | "variable.parameter" => WHITE,
        _ => return None,
    };
    Some(style)
}

fn format_style(first: &'static str, second: &'static str) -> &'static str {
    if first == BOLD && second == COMMENT_ERROR {
        "\x1b[1m\x1b[38;2;224;108;117m"
    } else if first == BOLD && second == COMMENT_TODO {
        "\x1b[1m\x1b[38;2;229;192;123m"
    } else if first == SYNTAX_COMMENT && second == ITALIC {
        "\x1b[38;2;127;132;142m\x1b[3m"
    } else {
        first
    }
}

fn keywords_for_lang(lang: &str) -> &'static [&'static str] {
    if matches!(lang, "rs" | "rust") {
        &[
            "Self", "as", "async", "await", "break", "const", "continue", "crate", "dyn",
            "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let",
            "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
            "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
            "where", "while",
        ]
    } else if matches!(
        lang,
        "ts" | "tsx" | "js" | "jsx" | "typescript" | "javascript"
    ) {
        &[
            "async",
            "await",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "debugger",
            "default",
            "delete",
            "do",
            "else",
            "export",
            "extends",
            "false",
            "for",
            "function",
            "if",
            "import",
            "interface",
            "let",
            "new",
            "null",
            "of",
            "package",
            "private",
            "protected",
            "public",
            "return",
            "static",
            "super",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "type",
            "typeof",
            "var",
            "void",
            "while",
            "yield",
        ]
    } else if matches!(lang, "sh" | "bash" | "zsh") {
        &[
            "case", "do", "done", "elif", "else", "esac", "export", "fi", "for",
            "function", "if", "in", "local", "readonly", "select", "set", "then", "trap",
            "until", "while",
        ]
    } else if matches!(lang, "py" | "python") {
        &[
            "and", "as", "async", "await", "break", "class", "continue", "def", "elif",
            "else", "except", "False", "finally", "for", "from", "global", "if",
            "import", "in", "is", "lambda", "None", "not", "or", "pass", "raise",
            "return", "True", "try", "while", "with", "yield",
        ]
    } else if matches!(lang, "go" | "golang") {
        &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "nil",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "type",
            "var",
        ]
    } else if matches!(lang, "c" | "h" | "cpp" | "cc" | "cxx" | "hpp") {
        &[
            "alignas",
            "alignof",
            "auto",
            "bool",
            "break",
            "case",
            "catch",
            "char",
            "class",
            "const",
            "constexpr",
            "continue",
            "decltype",
            "default",
            "delete",
            "do",
            "double",
            "else",
            "enum",
            "explicit",
            "extern",
            "false",
            "float",
            "for",
            "friend",
            "goto",
            "if",
            "inline",
            "int",
            "long",
            "namespace",
            "new",
            "noexcept",
            "nullptr",
            "operator",
            "private",
            "protected",
            "public",
            "return",
            "short",
            "signed",
            "sizeof",
            "static",
            "struct",
            "switch",
            "template",
            "this",
            "throw",
            "true",
            "try",
            "typedef",
            "typename",
            "union",
            "unsigned",
            "using",
            "virtual",
            "void",
            "volatile",
            "while",
        ]
    } else if matches!(lang, "java" | "kt" | "kotlin") {
        &[
            "abstract",
            "as",
            "boolean",
            "break",
            "byte",
            "case",
            "catch",
            "char",
            "class",
            "companion",
            "const",
            "constructor",
            "continue",
            "data",
            "default",
            "do",
            "double",
            "else",
            "enum",
            "extends",
            "false",
            "final",
            "finally",
            "float",
            "for",
            "fun",
            "if",
            "implements",
            "import",
            "in",
            "instanceof",
            "int",
            "interface",
            "is",
            "long",
            "new",
            "null",
            "object",
            "override",
            "package",
            "private",
            "protected",
            "public",
            "return",
            "sealed",
            "short",
            "static",
            "super",
            "switch",
            "this",
            "throw",
            "throws",
            "true",
            "try",
            "val",
            "var",
            "void",
            "when",
            "while",
        ]
    } else if matches!(lang, "json" | "jsonc") {
        &["false", "null", "true"]
    } else if matches!(lang, "toml" | "yaml" | "yml") {
        &["false", "null", "true"]
    } else if matches!(lang, "sql") {
        &[
            "and", "as", "asc", "by", "case", "create", "delete", "desc", "distinct",
            "drop", "else", "end", "false", "from", "group", "having", "in", "insert",
            "into", "is", "join", "left", "like", "limit", "not", "null", "on", "or",
            "order", "outer", "right", "select", "set", "table", "then", "true", "union",
            "update", "values", "when", "where",
        ]
    } else {
        &[]
    }
}

fn syntax_highlight(lang: &str, line: &str, keywords: &[&str]) -> String {
    let mut output = String::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];

        if starts_line_comment(lang, bytes, i) {
            output.push_str(&highlight_comment(&line[i..]));
            break;
        }

        if starts_block_comment(bytes, i) {
            let Some(end) = line[i + 2..].find("*/") else {
                output.push_str(&highlight_comment(&line[i..]));
                break;
            };
            let comment_end = i + 2 + end + 2;
            output.push_str(&highlight_comment(&line[i..comment_end]));
            i = comment_end;
            continue;
        }

        if ch == b'"' || ch == b'\'' || (ch == b'`' && is_backtick_string_lang(lang)) {
            let quote = ch;
            output.push_str(SYNTAX_STRING);
            output.push(quote as char);
            i += 1;
            while i < bytes.len() {
                let c = bytes[i];
                if c == b'\\' && i + 1 < bytes.len() {
                    output.push_str(RESET);
                    output.push_str(SYNTAX_ESCAPE);
                    output.push(c as char);
                    output.push(bytes[i + 1] as char);
                    output.push_str(RESET);
                    output.push_str(SYNTAX_STRING);
                    i += 2;
                    continue;
                }
                output.push(c as char);
                i += 1;
                if c == quote {
                    break;
                }
            }
            output.push_str(RESET);
            continue;
        }

        if is_number_start(bytes, i) {
            output.push_str(SYNTAX_NUMBER);
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || matches!(bytes[i], b'.' | b'_' | b'+' | b'-'))
            {
                output.push(bytes[i] as char);
                i += 1;
            }
            output.push_str(RESET);
            continue;
        }

        if is_identifier_start(ch) {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || bytes[i] == b'_'
                    || bytes[i] == b'$')
            {
                i += 1;
            }
            let token = &line[start..i];
            output.push_str(style_for_token(lang, line, start, i, token, keywords));
            output.push_str(token);
            output.push_str(RESET);
            continue;
        }

        if is_operator_char(ch) {
            output.push_str(SYNTAX_OPERATOR);
            while i < bytes.len() && is_operator_char(bytes[i]) {
                output.push(bytes[i] as char);
                i += 1;
            }
            output.push_str(RESET);
            continue;
        }

        if is_punctuation_char(ch) {
            output.push_str(SYNTAX_PUNCTUATION);
            output.push(ch as char);
            output.push_str(RESET);
            i += 1;
            continue;
        }

        output.push(ch as char);
        i += 1;
    }
    output
}

fn is_hash_comment_lang(lang: &str) -> bool {
    matches!(
        lang,
        "py" | "python" | "sh" | "bash" | "zsh" | "toml" | "yaml" | "yml" | "rb" | "ruby"
    )
}

fn starts_line_comment(lang: &str, bytes: &[u8], index: usize) -> bool {
    bytes
        .get(index..index + 2)
        .is_some_and(|pair| pair == b"//")
        || (is_hash_comment_lang(lang) && bytes.get(index).is_some_and(|ch| *ch == b'#'))
        || (matches!(lang, "sql")
            && bytes
                .get(index..index + 2)
                .is_some_and(|pair| pair == b"--"))
}

fn starts_block_comment(bytes: &[u8], index: usize) -> bool {
    bytes
        .get(index..index + 2)
        .is_some_and(|pair| pair == b"/*")
}

fn is_backtick_string_lang(lang: &str) -> bool {
    matches!(
        lang,
        "ts" | "tsx" | "js" | "jsx" | "typescript" | "javascript" | "sh" | "bash" | "zsh"
    )
}

fn is_number_start(bytes: &[u8], index: usize) -> bool {
    let Some(ch) = bytes.get(index) else {
        return false;
    };
    if !ch.is_ascii_digit() {
        return false;
    }
    index == 0 || !is_identifier_continue(bytes[index - 1])
}

fn is_identifier_start(ch: u8) -> bool {
    ch.is_ascii_alphabetic() || ch == b'_' || ch == b'$'
}

fn is_identifier_continue(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'$'
}

fn is_operator_char(ch: u8) -> bool {
    matches!(
        ch,
        b'+' | b'-'
            | b'*'
            | b'/'
            | b'%'
            | b'='
            | b'!'
            | b'<'
            | b'>'
            | b'&'
            | b'|'
            | b'^'
            | b'~'
            | b'?'
            | b':'
            | b'@'
    )
}

fn is_punctuation_char(ch: u8) -> bool {
    matches!(
        ch,
        b'{' | b'}' | b'[' | b']' | b'(' | b')' | b',' | b';' | b'.'
    )
}

fn style_for_token<'a>(
    lang: &str,
    line: &str,
    start: usize,
    end: usize,
    token: &str,
    keywords: &[&str],
) -> &'a str {
    if is_literal(token) {
        return SYNTAX_NUMBER;
    }
    if is_type_keyword(token) {
        return SYNTAX_TYPE;
    }
    if keywords.contains(&token) || is_keyword_alias(lang, token) {
        return SYNTAX_KEYWORD;
    }
    if is_builtin(token) {
        return SYNTAX_FUNCTION;
    }
    if is_constant_like(token) {
        return SYNTAX_CONSTANT;
    }
    if previous_non_space(line, start) == Some('.') {
        if next_non_space(line, end) == Some('(') {
            return SYNTAX_FUNCTION;
        }
        return SYNTAX_PROPERTY;
    }
    if next_non_space(line, end) == Some('(') {
        return SYNTAX_FUNCTION;
    }
    if next_non_space(line, end) == Some(':') && !matches!(lang, "rs" | "rust") {
        return SYNTAX_PROPERTY;
    }
    if token
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
    {
        return SYNTAX_TYPE;
    }
    WHITE
}

fn previous_non_space(line: &str, index: usize) -> Option<char> {
    line[..index].chars().rev().find(|ch| !ch.is_whitespace())
}

fn next_non_space(line: &str, index: usize) -> Option<char> {
    line[index..].chars().find(|ch| !ch.is_whitespace())
}

fn is_literal(token: &str) -> bool {
    matches!(
        token,
        "true" | "false" | "True" | "False" | "null" | "None" | "nil" | "nullptr"
    )
}

fn is_type_keyword(token: &str) -> bool {
    matches!(
        token,
        "bool"
            | "boolean"
            | "byte"
            | "char"
            | "double"
            | "float"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "int"
            | "long"
            | "number"
            | "short"
            | "str"
            | "string"
            | "String"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "void"
            | "type"
            | "interface"
            | "struct"
            | "enum"
            | "class"
            | "trait"
    )
}

fn is_keyword_alias(lang: &str, token: &str) -> bool {
    matches!(
        token,
        "async" | "await" | "return" | "import" | "export" | "from" | "where"
    ) || (matches!(lang, "json" | "jsonc" | "toml" | "yaml" | "yml") && is_literal(token))
}

fn is_builtin(token: &str) -> bool {
    matches!(
        token,
        "assert"
            | "bool"
            | "console"
            | "dict"
            | "eprintln"
            | "fetch"
            | "format"
            | "len"
            | "list"
            | "map"
            | "Math"
            | "Ok"
            | "Option"
            | "panic"
            | "print"
            | "println"
            | "Promise"
            | "range"
            | "Result"
            | "set"
            | "Some"
            | "str"
            | "String"
            | "Vec"
            | "vec"
    )
}

fn is_constant_like(token: &str) -> bool {
    let mut has_letter = false;
    let mut has_lower = false;
    let mut has_separator = false;
    for ch in token.chars() {
        if ch.is_ascii_alphabetic() {
            has_letter = true;
            has_lower |= ch.is_ascii_lowercase();
        }
        has_separator |= ch == '_';
    }
    has_letter && !has_lower && (has_separator || token.len() > 1)
}

fn highlight_comment(comment: &str) -> String {
    let mut output = String::new();
    output.push_str(SYNTAX_COMMENT);
    output.push_str(ITALIC);

    let bytes = comment.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            let token = &comment[start..i];
            let upper = token.to_ascii_uppercase();
            if matches!(upper.as_str(), "ERROR" | "ERR" | "BUG" | "FIXME") {
                output.push_str(RESET);
                output.push_str(BOLD);
                output.push_str(COMMENT_ERROR);
                output.push_str(token);
                output.push_str(RESET);
                output.push_str(SYNTAX_COMMENT);
                output.push_str(ITALIC);
            } else if matches!(
                upper.as_str(),
                "TODO" | "NOTE" | "WARN" | "WARNING" | "HACK"
            ) {
                output.push_str(RESET);
                output.push_str(BOLD);
                output.push_str(COMMENT_TODO);
                output.push_str(token);
                output.push_str(RESET);
                output.push_str(SYNTAX_COMMENT);
                output.push_str(ITALIC);
            } else {
                output.push_str(token);
            }
            continue;
        }
        output.push(bytes[i] as char);
        i += 1;
    }

    output.push_str(RESET);
    output
}
