//! Original composer tray painter shared by desktop and native GPU hosts.
use super::{AgentStreamingStatus,ComposerVisualPolicy,composer_visual_policy,composer_help_panel_color,composer_system_layout,checkout_context_width,checkout_branch_fits,visible_footer_checkout_context,themed_activity_scanner_color,INPUT_COMMAND_HINT,COMPOSER_TRAY_RADIUS,DEPTH,ORDER_PANEL,ORDER_TEXT,AgentCheckoutContext};
use crate::panels::agent_pane::composer_interactions::{ComposerInteractions,ComposerSystemControl};
use crate::panels::agent_pane::view::draw::{draw_text_clipped,draw_rect_clipped,opts_with_clip};
use crate::primitives::{paint::Paint,ide_theme::IdeTheme};
use crate::render_policy::opencode_scanner_frame;
use neoism_paint::text::DrawOpts;

pub struct FooterSnapshot {
    pub help_visible:bool,pub no_providers:bool,pub streaming:AgentStreamingStatus,
    pub system:Option<ComposerSystemControl>,pub interactions:ComposerInteractions,
}
impl FooterSnapshot {
    fn input_help_visible(&self)->bool{self.help_visible}
    fn no_providers_connected(&self)->bool{self.no_providers}
    fn streaming_state(&self)->AgentStreamingStatus{self.streaming}
    fn composer_system(&self)->Option<ComposerSystemControl>{self.system.clone()}
    fn composer_interactions(&mut self)->&mut ComposerInteractions{&mut self.interactions}
}
#[derive(serde::Serialize)]
pub struct ComposerControls { pub visual_rect:[f32;4],pub hit_rect:[f32;4],pub chips_rect:[f32;4],pub text_rect:[f32;4],pub narrow:bool,pub action:&'static str }
pub fn controls(card:[f32;4],scale:f32,home:bool,has_draft:bool,interruptible:bool,images:usize)->ComposerControls{
 let policy=composer_visual_policy(card[2],scale,home,has_draft,interruptible,interruptible);
 let box_h=(card[3]-super::CHIPS_BAND_H*scale).max(44.0*scale);
 let geometry=super::composer_footer_geometry([card[0],card[1]+box_h,card[2],super::CHIPS_BAND_H*scale],scale,policy);
 let (top,bottom)=super::composer_text_padding(home);let image_h=if images>0{super::super::INPUT_IMAGE_RAIL_H*scale}else{0.0};
 ComposerControls{visual_rect:geometry.visual_rect,hit_rect:geometry.hit_rect,chips_rect:geometry.chips_rect,
 text_rect:[card[0]+18.0*scale,card[1]+top*scale+image_h,(card[2]-36.0*scale).max(20.0*scale),(box_h-top*scale-bottom*scale-image_h).max(super::INPUT_LINE_H*scale)],narrow:policy.narrow,
 action:match policy.action{super::ComposerControlAction::Disabled=>"Disabled",super::ComposerControlAction::Send=>"Send",super::ComposerControlAction::Interrupt=>"Interrupt"}}
}

#[allow(clippy::too_many_arguments)]
pub fn render(paint:&mut (impl Paint + ?Sized),pane:&mut FooterSnapshot,rect:[f32;4],available_width:f32,theme:&IdeTheme,scale:f32,home:bool,has_draft:bool,interruptible:bool,seconds:f32,checkout:Option<&AgentCheckoutContext>){
    let policy=composer_visual_policy(available_width,scale,home,has_draft,pane.streaming!=AgentStreamingStatus::Idle,interruptible);
    render_with_policy(paint,pane,policy,rect,theme,scale,seconds,&[],checkout)
}

pub(super) fn render_with_policy(
    sugarloaf: &mut (impl Paint + ?Sized),
    pane: &mut FooterSnapshot,
    policy: ComposerVisualPolicy,
    rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    occlusion_rects: &[[f32; 4]],
    checkout_context: Option<&AgentCheckoutContext>,
) {
    let [x, y, w, h] = rect;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let radius = (COMPOSER_TRAY_RADIUS * s).min(w * 0.5).min(h);
    let stroke = s.min(w * 0.5).min(h);
    let inner = [x + stroke, y, (w - 2.0 * stroke).max(0.0), (h - stroke).max(0.0)];
    let inner_radius = (radius - stroke).max(0.0);
    crate::widgets::frame::draw_background_border(
        sugarloaf, rect, inner, [0.0, 0.0, radius, radius],
        [0.0, 0.0, inner_radius, inner_radius],
        theme.f32_alpha(theme.border, 0.4), DEPTH, ORDER_PANEL,
    );
    sugarloaf.quad(
        None, inner[0], inner[1], inner[2], inner[3], composer_help_panel_color(theme),
        [0.0, 0.0, inner_radius, inner_radius], DEPTH, ORDER_PANEL,
    );
    let padding = (14.0 * s).min(w * 0.5);
    let rect = [x + padding, y, (w - 2.0 * padding).max(0.0), h];
    let [x, y, w, h] = rect;
    let clip = rect;
    let key_opts = DrawOpts {
        font_size: 12.5 * s,
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let label_opts = DrawOpts {
        font_size: 12.5 * s,
        color: theme.u8(theme.muted),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let baseline_y = y + (h - key_opts.font_size) * 0.5;

    // Reserve the right-hand hints before deciding whether the branch fits.
    let needs_connect = pane.no_providers_connected();
    let (slash, command_label) = if needs_connect {
        ("/connect", "to connect a provider")
    } else {
        INPUT_COMMAND_HINT
    };
    let command_label_w = sugarloaf.text_mut().measure(command_label, &label_opts);
    let slash_w = sugarloaf.text_mut().measure(slash, &key_opts);
    let inner_gap = 7.0 * s;
    let right_w = slash_w + inner_gap + command_label_w;
    let right_x = x + (w - right_w).max(0.0);
    let hint_left = if pane.input_help_visible() && policy.show_command_hint {
        right_x
    } else {
        x + w
    };

    // A settled conversation mirrors New Chat's checkout context. While a run
    // is active, the activity scanner and interrupt hint own this same slot.
    let visible_checkout = visible_footer_checkout_context(policy, checkout_context);
    let checkout_hint_left = hint_left;
    let mut checkout_rect = rect;
    pane.composer_interactions().system_rects.clear();
    if !policy.show_activity {
        if let Some(system) = pane.composer_system() {
            let icon = crate::primitives::look::themed_glyph("status.system", "\u{f108}");
            let label = if system.selectable {
                format!("{icon}  {}  \u{f107}", system.label)
            } else {
                format!("{icon}  {}", system.label)
            };
            let measured = sugarloaf.text_mut().measure(&label, &label_opts);
            let (system_rect, remaining) = composer_system_layout(
                rect,
                hint_left,
                measured,
                visible_checkout.is_some(),
                s,
            );
            if let Some(opts) = opts_with_clip(label_opts.clone(), system_rect) {
                draw_text_clipped(
                    sugarloaf,
                    system_rect[0],
                    baseline_y,
                    &label,
                    &opts,
                    occlusion_rects,
                );
            }
            if system.selectable {
                pane.composer_interactions().system_rects =
                    crate::panels::agent_pane::composer_interactions::cwd_visible_hit_rects(
                        system_rect, measured, system_rect[2], occlusion_rects,
                    );
            }
            checkout_rect = remaining;
        }
    }
    let mut activity_guard = visible_checkout.map_or(checkout_rect[0], |context| {
        render_checkout_context(
            sugarloaf,
            context,
            checkout_rect,
            baseline_y,
            &key_opts,
            &label_opts,
            checkout_hint_left,
            s,
            occlusion_rects,
        )
    });
    if let Some(cwd) = visible_checkout.and_then(|context| context.cwd.as_deref()) {
        let glyph = crate::primitives::look::themed_glyph("status.folder", "\u{f07b}");
        let measured_cwd = sugarloaf.text_mut().measure(glyph, &label_opts)
            + 6.0 * s
            + sugarloaf.text_mut().measure(cwd, &key_opts);
        let clip_width = checkout_context_width(checkout_rect, checkout_hint_left, s);
        let hits =
            crate::panels::agent_pane::composer_interactions::cwd_visible_hit_rects(
                checkout_rect,
                measured_cwd,
                clip_width,
                occlusion_rects,
            );
        let interactions = pane.composer_interactions();
        interactions.cwd_rect = (hits.len() == 1).then(|| hits[0]);
        interactions.cwd_rects = hits;
    }
    if !pane.input_help_visible() {
        return;
    }
    if policy.show_activity {
        let activity_w = draw_opencode_activity_scanner(
            sugarloaf,
            activity_guard,
            baseline_y,
            12.5 * s,
            now_seconds,
            theme,
            pane.streaming_state(),
            clip,
            occlusion_rects,
        );
        activity_guard += activity_w + 10.0 * s;
        if policy.show_interrupt_hint {
            let esc_x = activity_guard;
            draw_text_clipped(
                sugarloaf,
                esc_x,
                baseline_y,
                "esc",
                &key_opts,
                occlusion_rects,
            );
            let esc_w = sugarloaf.text_mut().measure("esc", &key_opts);
            draw_text_clipped(
                sugarloaf,
                esc_x + esc_w + 7.0 * s,
                baseline_y,
                "interrupt",
                &label_opts,
                occlusion_rects,
            );
        }
    }

    if !policy.show_command_hint {
        return;
    }
    let slash_x = right_x;
    draw_text_clipped(
        sugarloaf,
        slash_x,
        baseline_y,
        slash,
        &key_opts,
        occlusion_rects,
    );
    draw_text_clipped(
        sugarloaf,
        slash_x + slash_w + inner_gap,
        baseline_y,
        command_label,
        &label_opts,
        occlusion_rects,
    );
}

fn render_checkout_context(
    sugarloaf: &mut (impl Paint + ?Sized),
    context: &AgentCheckoutContext,
    rect: [f32; 4],
    baseline_y: f32,
    value_opts: &DrawOpts,
    icon_opts: &DrawOpts,
    hint_left: f32,
    s: f32,
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    let [x, y, _, h] = rect;
    let max_w = checkout_context_width(rect, hint_left, s);
    if max_w <= 0.0 {
        return x;
    }
    let clip = [x, y, max_w, h];
    let mut value_opts = value_opts.clone();
    value_opts.clip_rect = Some(clip);
    let mut icon_opts = icon_opts.clone();
    icon_opts.clip_rect = Some(clip);
    let inner_gap = 6.0 * s;
    let group_gap = 18.0 * s;
    let mut cursor_x = x;

    if let Some(cwd) = context.cwd.as_deref() {
        let glyph = crate::primitives::look::themed_glyph("status.folder", "\u{f07b}");
        draw_text_clipped(
            sugarloaf,
            cursor_x,
            baseline_y,
            glyph,
            &icon_opts,
            occlusion_rects,
        );
        cursor_x += sugarloaf.text_mut().measure(glyph, &icon_opts) + inner_gap;
        draw_text_clipped(
            sugarloaf,
            cursor_x,
            baseline_y,
            cwd,
            &value_opts,
            occlusion_rects,
        );
        cursor_x += sugarloaf.text_mut().measure(cwd, &value_opts) + group_gap;
    }

    if let Some(branch) = context.branch.as_deref() {
        let glyph = crate::primitives::look::themed_glyph("status.branch", "\u{e725}");
        let glyph_w = sugarloaf.text_mut().measure(glyph, &icon_opts);
        let branch_w = sugarloaf.text_mut().measure(branch, &value_opts);
        if checkout_branch_fits(
            cursor_x,
            glyph_w + inner_gap + branch_w,
            x + max_w,
            hint_left,
            s,
        ) {
            draw_text_clipped(
                sugarloaf,
                cursor_x,
                baseline_y,
                glyph,
                &icon_opts,
                occlusion_rects,
            );
            cursor_x += glyph_w + inner_gap;
            draw_text_clipped(
                sugarloaf,
                cursor_x,
                baseline_y,
                branch,
                &value_opts,
                occlusion_rects,
            );
            cursor_x += branch_w;
        }
    }

    cursor_x.min(x + max_w) + 12.0 * s
}


pub(super) fn draw_opencode_activity_scanner(
    sugarloaf: &mut (impl Paint + ?Sized),
    x: f32,
    y: f32,
    font_size: f32,
    now_seconds: f32,
    theme: &IdeTheme,
    state: AgentStreamingStatus,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    // The scanner cells are drawn as RECTS, not as text.
    //
    // They used to be the glyphs `\u{25A0}` (BLACK SQUARE) and `\u{2B1D}`
    // (BLACK VERY SMALL SQUARE). Those live in Geometric Shapes /
    // Miscellaneous Symbols, NOT in the Private Use Area — so on web they
    // fall outside the three PUA symbol ranges the wasm host maps to the
    // Nerd Font, and the browser build has no system-font fallback to
    // resolve them from. The whole scanner rendered as tofu boxes there
    // while desktop found the glyphs through fontconfig. They are literally
    // squares, so drawing them as rects removes the font dependency
    // entirely and makes every platform paint the same thing.
    let cell_w = font_size * 0.68;
    let active_size = font_size * 0.52;
    let idle_size = font_size * 0.22;
    let frame = opencode_scanner_frame(now_seconds);

    for (index, cell) in frame.into_iter().enumerate() {
        let color = themed_activity_scanner_color(
            theme,
            state,
            cell.active,
            cell.brightness,
            cell.alpha,
        );
        let size = if cell.active { active_size } else { idle_size };
        let cell_x = x + index as f32 * cell_w;
        // Centre the square in the cell and on the text's em box, so the
        // run sits on the same optical line as the `esc` label beside it.
        let left = cell_x + (cell_w - size) * 0.5;
        let top = y + (font_size - size) * 0.5;
        if occlusion_rects
            .iter()
            .any(|occ| crate::primitives::rects_intersect([left, top, size, size], *occ))
        {
            continue;
        }
        // Same three depth layers the glyph version stacked: a background-
        // tinted drop behind, a dim mid layer, then the lit face.
        let mut far = theme.f32(theme.bg);
        far[3] = color[3] as f32 / 255.0 * (2.0 / 3.0);
        draw_rect_clipped(
            sugarloaf,
            [left + 3.0, top + 3.0, size, size],
            far,
            ORDER_TEXT,
            clip,
        );
        let mut near = theme.f32(theme.dim);
        near[3] = color[3] as f32 / 255.0 * (7.0 / 8.0);
        draw_rect_clipped(
            sugarloaf,
            [left + 1.5, top + 1.5, size, size],
            near,
            ORDER_TEXT,
            clip,
        );
        draw_rect_clipped(
            sugarloaf,
            [left, top, size, size],
            [
                color[0] as f32 / 255.0,
                color[1] as f32 / 255.0,
                color[2] as f32 / 255.0,
                color[3] as f32 / 255.0,
            ],
            ORDER_TEXT,
            clip,
        );
    }
    cell_w * frame.len() as f32
}
