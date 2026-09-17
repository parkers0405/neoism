use super::{DocumentationNotebook, NotebookAction};
use crate::primitives::{draw_text_with_occlusion, IdeTheme};
use sugarloaf::{text::DrawOpts, Sugarloaf};

/// Returns the document's rectangle. Navigation occupies real layout space,
/// not an overlay painted across selectable Markdown text.
pub fn render_navigation(
    sugarloaf: &mut Sugarloaf,
    book: &mut DocumentationNotebook,
    rect: [f32; 4],
    theme: &IdeTheme,
    mouse: Option<[f32; 2]>,
    occlusions: &[[f32; 4]],
) -> [f32; 4] {
    let [x, y, w, h] = rect;
    book.hit_regions.clear();
    book.rail_rect = None;
    let header_h = 38.0_f32.min(h);
    let footer_h = 42.0_f32.min((h - header_h).max(0.0));
    let wide = w >= 850.0;
    let show_rail = wide || book.contents_open;
    let rail_w = if show_rail {
        216.0_f32.min(w * 0.55)
    } else {
        0.0
    };
    let body = [
        x + rail_w,
        y + header_h,
        (w - rail_w).max(1.0),
        (h - header_h - footer_h).max(1.0),
    ];
    sugarloaf.rect(None, x, y, w, h, theme.f32(theme.bg), 0.0, 1);
    let mut label =
        |text: &str, bounds: [f32; 4], active: bool, action: Option<NotebookAction>| {
            let hovered = mouse.is_some_and(|[mx, my]| super::contains(bounds, mx, my));
            if active || (hovered && action.is_some()) {
                sugarloaf.rect(
                    None,
                    bounds[0],
                    bounds[1],
                    bounds[2],
                    bounds[3],
                    theme.f32(theme.surface),
                    0.0,
                    2,
                );
            }
            let opts = DrawOpts {
                font_size: 12.5,
                color: theme.u8(if active { theme.fg } else { theme.muted }),
                bold: active,
                clip_rect: Some(bounds),
                ..DrawOpts::default()
            };
            draw_text_with_occlusion(
                sugarloaf,
                bounds[0] + 10.0,
                bounds[1] + 8.0,
                text,
                &opts,
                occlusions,
            );
            if let Some(action) = action {
                book.hit_regions.push((bounds, action));
            }
        };
    label(
        "Pages",
        [x, y, 65.0, header_h],
        show_rail,
        Some(NotebookAction::ToggleContents),
    );
    label(
        "<",
        [x + 65.0, y, 30.0, header_h],
        false,
        (book.history_index > 0).then_some(NotebookAction::Back),
    );
    label(
        ">",
        [x + 95.0, y, 30.0, header_h],
        false,
        (book.history_index + 1 < book.history.len()).then_some(NotebookAction::Forward),
    );
    let title = format!(
        "{} / {}",
        book.manifest.title,
        book.manifest.pages[book.current].title()
    );
    label(
        &title,
        [x + 130.0, y, (w - 140.0).max(0.0), header_h],
        true,
        None,
    );
    if show_rail {
        let rail = [x, body[1], rail_w, body[3]];
        book.rail_rect = Some(rail);
        let list_bottom = body[1] + body[3] - 96.0;
        let mut row_y = body[1] + 10.0;
        let mut section = None;
        for (index, page) in book
            .manifest
            .pages
            .iter()
            .enumerate()
            .skip(book.scroll_rows)
        {
            if row_y + 30.0 > list_bottom {
                break;
            }
            if page.section() != section {
                section = page.section();
                if let Some(section) = section {
                    let prefix = if book.collapsed_sections.contains(section) {
                        ">"
                    } else {
                        "v"
                    };
                    label(
                        &format!("{prefix} {section}"),
                        [x + 6.0, row_y, rail_w - 12.0, 28.0],
                        true,
                        Some(NotebookAction::ToggleSection(section.to_string())),
                    );
                    row_y += 28.0;
                }
            }
            if row_y + 30.0 > list_bottom {
                break;
            }
            if page
                .section()
                .is_some_and(|section| book.collapsed_sections.contains(section))
            {
                continue;
            }
            let path =
                super::normalize_path(&book.path.parent().unwrap().join(page.path()));
            let title = if book.modified_pages.contains(&path) {
                format!("{} *", page.title())
            } else {
                page.title()
            };
            label(
                &title,
                [x + 6.0, row_y, rail_w - 12.0, 30.0],
                index == book.current,
                Some(NotebookAction::Page(index)),
            );
            row_y += 32.0;
        }
        let bottom = (body[1] + body[3] - 92.0).max(body[1]);
        label(
            "+ Add page",
            [x + 6.0, bottom, rail_w - 12.0, 28.0],
            false,
            Some(NotebookAction::AddPage),
        );
        label(
            "+ Link existing file",
            [x + 6.0, bottom + 28.0, rail_w - 12.0, 28.0],
            false,
            Some(NotebookAction::AddExisting),
        );
        label(
            "Move up",
            [x + 6.0, bottom + 56.0, (rail_w - 12.0) * 0.5, 28.0],
            false,
            Some(NotebookAction::MoveUp),
        );
        label(
            "Move down",
            [x + rail_w * 0.5, bottom + 56.0, (rail_w - 12.0) * 0.5, 28.0],
            false,
            Some(NotebookAction::MoveDown),
        );
    }
    let footer_y = y + h - footer_h;
    let position_w = 70.0;
    let button_w = ((body[2] - position_w) * 0.5).max(0.0);
    if book.current > 0 {
        let prev = book.current - 1;
        label(
            &format!("< {}", book.manifest.pages[prev].title()),
            [body[0], footer_y, button_w, footer_h],
            false,
            Some(NotebookAction::Page(prev)),
        );
    }
    label(
        &format!("{} / {}", book.current + 1, book.manifest.pages.len()),
        [body[0] + button_w, footer_y, position_w, footer_h],
        false,
        None,
    );
    if book.current + 1 < book.manifest.pages.len() {
        let next = book.current + 1;
        label(
            &format!("{} >", book.manifest.pages[next].title()),
            [
                body[0] + button_w + position_w,
                footer_y,
                button_w,
                footer_h,
            ],
            false,
            Some(NotebookAction::Page(next)),
        );
    }
    if show_rail {
        sugarloaf.rect(
            None,
            body[0] - 1.0,
            body[1],
            1.0,
            body[3],
            theme.f32(theme.surface),
            0.0,
            2,
        );
    }
    body
}
