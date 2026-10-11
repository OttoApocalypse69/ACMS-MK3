//! The one place colours, spacing and rounding are defined (see `PLAN.md`, section 5).
//!
//! Everything else reads [`Tokens`] or the egui style built from them; no other module should
//! hard-code a colour.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Vec2};

/// Space between widgets.
pub const SPACING: f32 = 8.0;
/// Padding inside panels.
pub const PANEL_PADDING: i8 = 12;
/// Corner radius of buttons, inputs and popups.
pub const RADIUS: u8 = 6;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeKind {
    #[default]
    Dark,
    Light,
}

impl ThemeKind {
    pub fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }

    pub fn tokens(self) -> Tokens {
        match self {
            Self::Dark => Tokens::DARK,
            Self::Light => Tokens::LIGHT,
        }
    }

    fn egui_theme(self) -> egui::Theme {
        match self {
            Self::Dark => egui::Theme::Dark,
            Self::Light => egui::Theme::Light,
        }
    }
}

/// Named colours of a theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    /// Behind everything: window background, tab bars, text inputs.
    pub surface: Color32,
    /// Panel bodies and popups.
    pub panel: Color32,
    /// Resting fill of buttons and other controls.
    pub control: Color32,
    /// Hairlines, separators, outlines.
    pub border: Color32,
    pub text: Color32,
    /// Secondary text: hints, inactive tabs, shortcuts.
    pub text_muted: Color32,
    /// The single accent colour: selection, focus, active controls.
    pub accent: Color32,
    /// Text drawn on top of `accent`.
    pub on_accent: Color32,
    /// Errors and destructive actions.
    pub danger: Color32,
    /// The two colours of the checkerboard shown behind transparent pixels.
    pub checker: [Color32; 2],
}

impl Tokens {
    pub const DARK: Self = Self {
        surface: Color32::from_rgb(0x16, 0x17, 0x1B),
        panel: Color32::from_rgb(0x1F, 0x21, 0x26),
        control: Color32::from_rgb(0x2A, 0x2D, 0x34),
        border: Color32::from_rgb(0x36, 0x39, 0x42),
        text: Color32::from_rgb(0xE6, 0xE8, 0xEC),
        text_muted: Color32::from_rgb(0x90, 0x96, 0xA2),
        accent: Color32::from_rgb(0x5B, 0x8D, 0xEF),
        on_accent: Color32::WHITE,
        danger: Color32::from_rgb(0xE5, 0x53, 0x4B),
        checker: [
            Color32::from_rgb(0x4B, 0x4E, 0x57),
            Color32::from_rgb(0x3C, 0x3F, 0x47),
        ],
    };

    pub const LIGHT: Self = Self {
        surface: Color32::from_rgb(0xE6, 0xE8, 0xEC),
        panel: Color32::from_rgb(0xF7, 0xF8, 0xFA),
        control: Color32::from_rgb(0xE2, 0xE5, 0xEA),
        border: Color32::from_rgb(0xCB, 0xCF, 0xD7),
        text: Color32::from_rgb(0x1D, 0x20, 0x26),
        text_muted: Color32::from_rgb(0x62, 0x69, 0x75),
        accent: Color32::from_rgb(0x2F, 0x6F, 0xEB),
        on_accent: Color32::WHITE,
        danger: Color32::from_rgb(0xD1, 0x24, 0x2F),
        checker: [
            Color32::from_rgb(0xFF, 0xFF, 0xFF),
            Color32::from_rgb(0xD6, 0xD9, 0xE0),
        ],
    };
}

/// Installs both themes into egui and activates `active`.
pub fn install(ctx: &egui::Context, active: ThemeKind) {
    for kind in [ThemeKind::Dark, ThemeKind::Light] {
        ctx.set_style_of(kind.egui_theme(), egui_style(kind));
    }
    // Ctrl+Plus / Minus / 0 zoom the canvas (through the keymap), not the whole interface.
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    set_active(ctx, active);
}

/// Switches between the themes installed by [`install`].
pub fn set_active(ctx: &egui::Context, active: ThemeKind) {
    ctx.set_theme(active.egui_theme());
}

fn egui_style(kind: ThemeKind) -> egui::Style {
    let t = kind.tokens();
    let radius = CornerRadius::same(RADIUS);

    let mut style = egui::Style {
        visuals: match kind {
            ThemeKind::Dark => egui::Visuals::dark(),
            ThemeKind::Light => egui::Visuals::light(),
        },
        ..Default::default()
    };

    style.text_styles = [
        (TextStyle::Heading, 20.0, FontFamily::Proportional),
        (TextStyle::Body, 14.0, FontFamily::Proportional),
        (TextStyle::Button, 14.0, FontFamily::Proportional),
        (TextStyle::Small, 11.5, FontFamily::Proportional),
        (TextStyle::Monospace, 13.0, FontFamily::Monospace),
    ]
    .into_iter()
    .map(|(text_style, size, family)| (text_style, FontId::new(size, family)))
    .collect();

    let spacing = &mut style.spacing;
    spacing.item_spacing = Vec2::splat(SPACING);
    spacing.button_padding = Vec2::new(10.0, 5.0);
    spacing.window_margin = Margin::same(PANEL_PADDING);
    spacing.menu_margin = Margin::same(8);
    spacing.interact_size.y = 28.0;
    spacing.indent = 18.0;

    let widget = |fill: Color32, outline: Color32, text: Color32| egui::style::WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: Stroke::new(1.0, outline),
        corner_radius: radius,
        fg_stroke: Stroke::new(1.0, text),
        expansion: 0.0,
    };

    let visuals = &mut style.visuals;
    visuals.widgets.noninteractive = widget(t.panel, t.border, t.text);
    visuals.widgets.inactive = widget(t.control, t.control, t.text);
    visuals.widgets.hovered = widget(t.border, t.accent, t.text);
    visuals.widgets.active = widget(t.accent, t.accent, t.on_accent);
    visuals.widgets.open = widget(t.border, t.border, t.text);

    visuals.selection.bg_fill = t.accent.gamma_multiply(0.4);
    visuals.selection.stroke = Stroke::new(1.0, t.accent);
    visuals.weak_text_color = Some(t.text_muted);
    visuals.hyperlink_color = t.accent;
    visuals.error_fg_color = t.danger;

    visuals.panel_fill = t.panel;
    visuals.window_fill = t.panel;
    visuals.window_stroke = Stroke::new(1.0, t.border);
    visuals.extreme_bg_color = t.surface;
    visuals.faint_bg_color = t.control;
    visuals.code_bg_color = t.control;
    visuals.window_corner_radius = CornerRadius::same(RADIUS + 2);
    visuals.menu_corner_radius = radius;

    style
}

/// The egui_dock look for the given theme, built on top of the current egui style.
pub fn dock_style(egui_style: &egui::Style, kind: ThemeKind) -> egui_dock::Style {
    let t = kind.tokens();
    let mut style = egui_dock::Style::from_egui(egui_style);

    style.main_surface_border_stroke = Stroke::NONE;

    style.tab_bar.bg_fill = t.surface;
    style.tab_bar.hline_color = t.border;
    style.tab_bar.height = 30.0;

    style.tab.tab_body.bg_fill = t.panel;
    style.tab.tab_body.stroke = Stroke::NONE;
    style.tab.tab_body.inner_margin = Margin::same(PANEL_PADDING);

    for tab in [&mut style.tab.active, &mut style.tab.focused] {
        tab.bg_fill = t.panel;
        tab.text_color = t.text;
        tab.outline_color = t.border;
    }
    style.tab.inactive.bg_fill = t.surface;
    style.tab.inactive.text_color = t.text_muted;
    style.tab.inactive.outline_color = Color32::TRANSPARENT;
    style.tab.hovered.bg_fill = t.control;
    style.tab.hovered.text_color = t.text;
    style.tab.hovered.outline_color = Color32::TRANSPARENT;

    style.separator.color_idle = t.border;
    style.separator.color_hovered = t.accent;
    style.separator.color_dragged = t.accent;

    style.overlay.selection_color = t.accent.gamma_multiply(0.35);

    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_twice_returns_to_the_start() {
        assert_eq!(ThemeKind::Dark.toggled(), ThemeKind::Light);
        assert_eq!(ThemeKind::Dark.toggled().toggled(), ThemeKind::Dark);
    }

    #[test]
    fn styles_follow_their_tokens() {
        for kind in [ThemeKind::Dark, ThemeKind::Light] {
            let style = egui_style(kind);
            assert_eq!(style.visuals.dark_mode, kind == ThemeKind::Dark);
            assert_eq!(style.visuals.panel_fill, kind.tokens().panel);
        }
    }
}
