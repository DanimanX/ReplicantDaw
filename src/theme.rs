//! Neon-noir visual language for Replicant DAW.
//!
//! Palette anchors from the brief: background `#070b12`, cyan `#00e5ff`,
//! magenta `#e024c3`, hairline borders and a lot of restrained glow.

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, Painter,
    Pos2, Rect, Stroke, TextStyle, Vec2,
};

pub const BG: Color32 = Color32::from_rgb(0x07, 0x0b, 0x12);
pub const BG_DEEP: Color32 = Color32::from_rgb(0x04, 0x07, 0x0d);
pub const PANEL: Color32 = Color32::from_rgb(0x0a, 0x11, 0x1c);
pub const PANEL_RAISED: Color32 = Color32::from_rgb(0x0d, 0x17, 0x26);
pub const PANEL_SUNK: Color32 = Color32::from_rgb(0x06, 0x0c, 0x15);
pub const HEADER: Color32 = Color32::from_rgb(0x0b, 0x16, 0x24);
pub const BORDER: Color32 = Color32::from_rgb(0x16, 0x2a, 0x3c);
pub const BORDER_SOFT: Color32 = Color32::from_rgb(0x11, 0x1f, 0x2e);
pub const CYAN: Color32 = Color32::from_rgb(0x00, 0xe5, 0xff);
pub const MAGENTA: Color32 = Color32::from_rgb(0xe0, 0x24, 0xc3);
pub const BLUE: Color32 = Color32::from_rgb(0x2f, 0x8b, 0xff);
pub const GREEN: Color32 = Color32::from_rgb(0x39, 0xff, 0x88);
pub const AMBER: Color32 = Color32::from_rgb(0xff, 0xb0, 0x3d);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x3b, 0x5c);
pub const TEXT: Color32 = Color32::from_rgb(0xc9, 0xe6, 0xf3);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x6c, 0x8a, 0x9e);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x3c, 0x56, 0x69);

pub const HEADER_H: f32 = 27.0;

const DISPLAY_FAMILY: &str = "rd_display";
const MONO_FAMILY: &str = "rd_mono";

pub fn display_family() -> FontFamily {
    FontFamily::Name(DISPLAY_FAMILY.into())
}

pub fn mono_family() -> FontFamily {
    FontFamily::Name(MONO_FAMILY.into())
}

pub fn display(size: f32) -> FontId {
    FontId::new((size * 1.30).max(9.0), display_family())
}

pub fn mono(size: f32) -> FontId {
    FontId::new((size * 1.30).max(9.0), mono_family())
}

pub fn body(size: f32) -> FontId {
    FontId::new((size * 1.25).max(11.0), FontFamily::Proportional)
}

/// Shrink a font until `text` fits `max_width`, with a readable floor.
///
/// Used for user-editable track names, which can be longer than the fixed
/// slots that display them (session headers and mixer strips).
pub fn fitted_font(painter: &Painter, text: &str, font: FontId, max_width: f32) -> FontId {
    if max_width <= 1.0 {
        return font;
    }
    let width = painter
        .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x;
    if width <= max_width || width <= 0.0 {
        return font;
    }
    let size = (font.size * max_width / width).max(8.0);
    FontId::new(size, font.family)
}

pub fn accent_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

pub fn track_accent(track: usize) -> Color32 {
    let c = crate::audio::tracks::TRACKS[track.min(crate::audio::TRACK_COUNT - 1)].accent;
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Install fonts, dark visuals and the custom widget style.
pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_fonts(fonts());

    ctx.all_styles_mut(|style| {
        style.visuals = visuals();
        style.spacing.item_spacing = Vec2::new(6.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 5.0);
        style.spacing.slider_width = 120.0;
        style.spacing.interact_size = Vec2::new(38.0, 24.0);
        style.spacing.window_margin = Margin::same(0);
        style.animation_time = 0.12;
        style.text_styles.extend([
            (TextStyle::Small, body(12.5)),
            (TextStyle::Body, body(15.0)),
            (TextStyle::Button, display(12.0)),
            (TextStyle::Monospace, mono(13.0)),
            (TextStyle::Heading, display(20.0)),
            (TextStyle::Name("display".into()), display(15.5)),
            (TextStyle::Name("mono_small".into()), mono(11.0)),
        ]);
    });
}

fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        "orbitron".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Orbitron-Variable.ttf"
        ))),
    );
    fonts.font_data.insert(
        "rajdhani".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Rajdhani-Medium.ttf"
        ))),
    );
    fonts.font_data.insert(
        "rajdhani_semibold".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Rajdhani-SemiBold.ttf"
        ))),
    );
    fonts.font_data.insert(
        "sharetech".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/ShareTechMono-Regular.ttf"
        ))),
    );

    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "rajdhani".to_owned());
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push("rajdhani_semibold".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "sharetech".to_owned());

    fonts.families.insert(
        display_family(),
        vec![
            "orbitron".to_owned(),
            "rajdhani_semibold".to_owned(),
            "rajdhani".to_owned(),
        ],
    );
    fonts.families.insert(
        mono_family(),
        vec!["sharetech".to_owned(), "Hack".to_owned()],
    );

    fonts
}

fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG_DEEP;
    v.faint_bg_color = PANEL_RAISED;
    v.code_bg_color = PANEL_SUNK;
    v.override_text_color = Some(TEXT);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(2);
    v.menu_corner_radius = CornerRadius::same(2);
    v.hyperlink_color = CYAN;
    v.warn_fg_color = AMBER;
    v.error_fg_color = RED;
    v.selection.bg_fill = accent_alpha(CYAN, 45);
    v.selection.stroke = Stroke::new(1.0, CYAN);

    let base = egui::style::WidgetVisuals {
        bg_fill: PANEL_RAISED,
        weak_bg_fill: PANEL_RAISED,
        bg_stroke: Stroke::new(1.0, BORDER),
        corner_radius: CornerRadius::same(2),
        fg_stroke: Stroke::new(1.0, TEXT),
        expansion: 0.0,
    };
    v.widgets.noninteractive = egui::style::WidgetVisuals {
        bg_fill: PANEL,
        weak_bg_fill: PANEL,
        bg_stroke: Stroke::new(1.0, BORDER_SOFT),
        corner_radius: CornerRadius::same(2),
        fg_stroke: Stroke::new(1.0, TEXT_DIM),
        expansion: 0.0,
    };
    v.widgets.inactive = base;
    v.widgets.hovered = egui::style::WidgetVisuals {
        bg_fill: Color32::from_rgb(0x10, 0x22, 0x32),
        weak_bg_fill: Color32::from_rgb(0x10, 0x22, 0x32),
        bg_stroke: Stroke::new(1.0, accent_alpha(CYAN, 120)),
        corner_radius: CornerRadius::same(2),
        fg_stroke: Stroke::new(1.0, CYAN),
        expansion: 0.0,
    };
    v.widgets.active = egui::style::WidgetVisuals {
        bg_fill: Color32::from_rgb(0x12, 0x2a, 0x38),
        weak_bg_fill: Color32::from_rgb(0x12, 0x2a, 0x38),
        bg_stroke: Stroke::new(1.0, CYAN),
        corner_radius: CornerRadius::same(2),
        fg_stroke: Stroke::new(1.0, CYAN),
        expansion: 0.0,
    };
    v.widgets.open = v.widgets.active;
    v
}

pub fn panel_frame() -> Frame {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::ZERO)
}

/// Paint the standard panel chrome and return the content rectangle.
pub fn paint_panel(painter: &Painter, rect: Rect, title: &str, tag: &str, accent: Color32) -> Rect {
    painter.rect_filled(rect, CornerRadius::same(3), PANEL);
    painter.rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );

    let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_H));
    painter.rect_filled(header, CornerRadius::same(3), HEADER);
    painter.rect_filled(
        Rect::from_min_size(header.min, Vec2::new(header.width(), HEADER_H - 3.0)),
        CornerRadius::same(3),
        HEADER,
    );
    painter.line_segment(
        [header.left_bottom(), header.right_bottom()],
        Stroke::new(1.0, BORDER),
    );
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(header.left() + 8.0, header.center().y - 5.0),
            Vec2::new(2.0, 10.0),
        ),
        CornerRadius::same(1),
        accent,
    );
    painter.text(
        Pos2::new(header.left() + 16.0, header.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        display(9.5),
        accent,
    );
    if !tag.is_empty() {
        painter.text(
            Pos2::new(header.right() - 10.0, header.center().y),
            egui::Align2::RIGHT_CENTER,
            tag,
            mono(9.0),
            TEXT_FAINT,
        );
    }

    paint_corners(painter, rect, accent_alpha(accent, 90), 7.0);

    panel_content(rect)
}

/// Interior of a panel painted by [`paint_panel`].
///
/// Shared with the layout helpers so hit-testing (drag and drop) uses exactly
/// the rectangles the paint code uses.
pub fn panel_content(rect: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.left() + 8.0, rect.top() + HEADER_H + 7.0),
        Pos2::new(rect.right() - 8.0, rect.bottom() - 7.0),
    )
}

pub fn paint_corners(painter: &Painter, rect: Rect, color: Color32, len: f32) {
    let s = Stroke::new(1.0, color);
    let r = rect.shrink(1.5);
    painter.line_segment([r.left_top(), Pos2::new(r.left() + len, r.top())], s);
    painter.line_segment([r.left_top(), Pos2::new(r.left(), r.top() + len)], s);
    painter.line_segment([r.right_top(), Pos2::new(r.right() - len, r.top())], s);
    painter.line_segment([r.right_top(), Pos2::new(r.right(), r.top() + len)], s);
    painter.line_segment([r.left_bottom(), Pos2::new(r.left() + len, r.bottom())], s);
    painter.line_segment([r.left_bottom(), Pos2::new(r.left(), r.bottom() - len)], s);
    painter.line_segment(
        [r.right_bottom(), Pos2::new(r.right() - len, r.bottom())],
        s,
    );
    painter.line_segment(
        [r.right_bottom(), Pos2::new(r.right(), r.bottom() - len)],
        s,
    );
}

/// Faint technical grid, used behind clip slots and the timeline.
pub fn paint_grid(
    painter: &Painter,
    rect: Rect,
    step: Vec2,
    color: Color32,
    major_every_x: usize,
    major_color: Color32,
) {
    let mut i = 0;
    let mut x = rect.left();
    while x <= rect.right() {
        let c = if major_every_x > 0 && i % major_every_x == 0 {
            major_color
        } else {
            color
        };
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.0, c),
        );
        x += step.x;
        i += 1;
    }
    let mut y = rect.top();
    while y <= rect.bottom() {
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(1.0, color),
        );
        y += step.y;
    }
}

pub fn paint_backdrop(painter: &Painter, rect: Rect) {
    use egui::{Color32 as C, Mesh, Shape};
    // Vertical gradient from midnight blue to near-black.
    let mut mesh = Mesh::default();
    let top = Color32::from_rgb(0x08, 0x10, 0x1b);
    let bottom = Color32::from_rgb(0x04, 0x07, 0x0c);
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(Shape::mesh(mesh));

    // Scanlines.
    let line = C::from_rgba_unmultiplied(0x00, 0xe5, 0xff, 5);
    let mut y = rect.top();
    while y < rect.bottom() {
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(1.0, line),
        );
        y += 4.0;
    }

    // Corner vignette strokes.
    let edge = Color32::from_rgba_unmultiplied(0x00, 0xe5, 0xff, 18);
    painter.rect_stroke(
        rect.shrink(1.0),
        CornerRadius::same(0),
        Stroke::new(1.0, edge),
        egui::StrokeKind::Inside,
    );
}

pub fn tiny_label(painter: &Painter, pos: Pos2, text: &str, color: Color32) {
    painter.text(pos, egui::Align2::LEFT_TOP, text, mono(9.0), color);
}

pub fn tag(painter: &Painter, pos: Pos2, text: &str, color: Color32) {
    painter.text(pos, egui::Align2::LEFT_CENTER, text, display(8.5), color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_colors_are_in_palette() {
        let c = track_accent(0);
        assert_eq!((c.r(), c.g(), c.b()), (0, 229, 255));
        assert_eq!(
            track_accent(99),
            track_accent(crate::audio::TRACK_COUNT - 1)
        );
    }

    #[test]
    fn alpha_helper_keeps_rgb() {
        let c = accent_alpha(MAGENTA, 20);
        // Color32 stores premultiplied sRGB; alpha survives verbatim.
        assert_eq!(c.a(), 20);
        let expected = Color32::from_rgba_unmultiplied(224, 36, 195, 20);
        assert_eq!(c, expected);
    }
}
