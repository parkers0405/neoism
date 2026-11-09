//! Neo is a projection of the existing timeline, not another transcript store.
use super::*;
use super::super::{draw::{draw_rounded_rect_clipped, draw_text_clipped, measure_text_cached, opts_with_clip}, markdown::{measure_markdown_blocks_namespaced, render_markdown_blocks}, user_input::wrap_user_message_unlimited as wrap_user_message, ORDER_PANEL};
use neoism_paint::text::DrawOpts;

const PAD: f32 = 12.0;
const GAP: f32 = 8.0;

fn inner_width(width: f32, s: f32) -> f32 { (width * 0.84 - 2.0 * PAD * s).max(40.0 * s) }
fn opts(theme: &IdeTheme, s: f32) -> DrawOpts {
    DrawOpts { font_size: 14.0*s, color: theme.u8(theme.fg), ..DrawOpts::default() }
}

/// Split only plain prose at parsed paragraph boundaries. Structured Markdown
/// stays in one bubble: never break a fence, list, table, artifact or JSON UI.
fn ranges(blocks: &[AssistantMarkdownBlock]) -> Vec<Range<usize>> {
    if blocks.is_empty() { return vec![0..0]; } // Image-only assistant message.
    if blocks.iter().all(|b| matches!(b, AssistantMarkdownBlock::Paragraph(_) | AssistantMarkdownBlock::Blank)) {
        blocks.iter().enumerate().filter_map(|(i,b)| matches!(b, AssistantMarkdownBlock::Paragraph(_)).then_some(i..i+1)).collect()
    } else { vec![0..blocks.len()] }
}

fn bubble_width(p: &mut dyn crate::primitives::paint::Paint, blocks: &[AssistantMarkdownBlock], cap: f32, theme: &IdeTheme, s: f32) -> f32 {
    if blocks.iter().all(|b| matches!(b, AssistantMarkdownBlock::Paragraph(_) | AssistantMarkdownBlock::Blank)) {
        blocks.iter().filter_map(|b| if let AssistantMarkdownBlock::Paragraph(lines)=b {Some(lines)} else {None})
            .flatten().map(|line| measure_text_cached(p, line, &opts(theme,s))).fold(40.0*s, f32::max).min(cap)
    } else { cap }
}

pub(super) fn measure(p: &mut dyn crate::primitives::paint::Paint, pane: &NeoismAgentPane, message: &NeoismAgentMessage, width: f32, theme: &IdeTheme, s: f32) -> f32 {
    let cap=inner_width(width,s);
    let image_h=if message.images.is_empty(){0.0}else{164.0*s};
    if message.kind==NeoismAgentMessageKind::User {
        let lines=wrap_user_message(p,&message.text,cap,&opts(theme,s));
        return 2.0*PAD*s + lines.len() as f32*22.0*s + image_h;
    }
    let blocks=layout_assistant_markdown_cached(p,pane,&message.text,cap,theme,s);
    let parts=ranges(&blocks);
    parts.iter().enumerate().map(|(i,range)| {
        let body=&blocks[range.clone()];
        let namespace=format!("{}:neo:{i}",message.id);
        let inner=if image_h>0.0{cap}else{bubble_width(p,body,cap,theme,s)};
        measure_markdown_blocks_namespaced(body,inner,pane,s,&namespace)+2.0*PAD*s
    }).sum::<f32>() + GAP*s*parts.len().saturating_sub(1) as f32 + image_h
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render(p: &mut dyn crate::primitives::paint::Paint, pane: &mut NeoismAgentPane, message: &NeoismAgentMessage, [x,y,w,h]: [f32;4], theme: &IdeTheme, s: f32, now: f32, mouse: Option<(f32,f32)>, clip: [f32;4], holes: &[[f32;4]]) -> f32 {
    let cap=inner_width(w,s);
    let user=message.kind==NeoismAgentMessageKind::User;
    let color=theme.f32_alpha(if user {theme.accent} else {theme.fg},if user {0.16}else{0.055});
    let image_h=if message.images.is_empty(){0.0}else{164.0*s};
    let mut image_x=x+PAD*s;
    let mut image_w=cap;
    if user {
        let text_opts=opts(theme,s);
        let lines=wrap_user_message(p,&message.text,cap,&text_opts);
        let inner=lines.iter().map(|line|measure_text_cached(p,line,&text_opts)).fold(if image_h>0.0{cap}else{40.0*s},f32::max).min(cap);
        let bw=inner+2.0*PAD*s;
        let bx=x+w-bw;
        image_x=bx+PAD*s; image_w=inner;
        draw_rounded_rect_clipped(p,[bx,y,bw,h],color,16.0*s,ORDER_PANEL,clip);
        if let Some(text_opts)=opts_with_clip(text_opts,clip) {
            for (i,line) in lines.iter().enumerate() {
                let ly=y+PAD*s+i as f32*22.0*s;
                let lw=measure_text_cached(p,line,&text_opts).max(1.0);
                if !pane.suppress_markdown_interactions() {
                    let index=pane.register_selectable_line(&line.text,[image_x,ly-3.0*s,lw,22.0*s]);
                    pane.set_selectable_line_separator(index,line.separator);
                    if let Some((left,right))=pane.selectable_line_highlight(index) {
                        draw_rounded_rect_clipped(p,[left,ly-3.0*s,right-left,22.0*s],theme.f32_alpha(theme.accent,0.25),2.0*s,ORDER_PANEL+2,clip);
                    }
                }
                draw_text_clipped(p,image_x,ly,line,&text_opts,holes);
            }
        }
    } else {
        let blocks=layout_assistant_markdown_cached(p,pane,&message.text,cap,theme,s);
        let parts=ranges(&blocks);
        let mut by=y;
        for (i,range) in parts.iter().enumerate() {
            let body=&blocks[range.clone()];
            let inner=if image_h>0.0{cap}else{bubble_width(p,body,cap,theme,s)};
            let namespace=format!("{}:neo:{i}",message.id);
            let bh=measure_markdown_blocks_namespaced(body,inner,pane,s,&namespace)+2.0*PAD*s;
            let attachments=if i+1==parts.len(){image_h}else{0.0};
            draw_rounded_rect_clipped(p,[x,by,inner+2.0*PAD*s,bh+attachments],color,16.0*s,ORDER_PANEL,clip);
            render_markdown_blocks(p,body,&namespace,x+PAD*s,by+PAD*s,inner,bh-2.0*PAD*s,pane,theme,s,false,theme.fg,false,now,mouse,clip,holes);
            by+=bh+GAP*s;
        }
    }
    if image_h>0.0 {
        let hits=super::super::image_preview::render_image_strip(p,&message.images,image_x,y+h-image_h,image_w,theme,s,super::super::image_preview::MESSAGE_THUMB_SIZE,clip,holes);
        if !pane.suppress_markdown_interactions(){for hit in hits {pane.register_image_preview(hit);}}
    }
    h
}

pub(super) fn typing(p: &mut dyn crate::primitives::paint::Paint, [x,y,_,_]: [f32;4], theme: &IdeTheme, s: f32, clip: [f32;4], holes: &[[f32;4]]) {
    // Deliberately no model/tool/elapsed-time dump, and no copyable status text.
    if let Some(options)=opts_with_clip(DrawOpts{color:theme.u8(theme.muted),..opts(theme,s)},clip) {
        draw_text_clipped(p,x+PAD*s,y+4.0*s,"•••",&options,holes);
    }
}