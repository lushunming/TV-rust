//! 深色 TV 风格主题 + 中文字体加载

use egui::{Color32, FontData, FontDefinitions, Style, Visuals};

/// 错误/危险提示色
pub const DANGER: Color32 = Color32::from_rgb(240, 120, 120);

/// 深蓝背景（对应原应用 darkBlue 0E1621）
pub const BG: Color32 = Color32::from_rgb(14, 22, 33);
/// 浅一点的面板背景
pub const BG_PANEL: Color32 = Color32::from_rgb(20, 31, 46);
/// 强调色（对应原应用 lightBlue 5EB5F7）
pub const ACCENT: Color32 = Color32::from_rgb(94, 181, 247);
pub const TEXT: Color32 = Color32::from_rgb(225, 233, 242);
pub const TEXT_DIM: Color32 = Color32::from_rgb(140, 155, 172);
pub const CARD: Color32 = Color32::from_rgb(28, 41, 60);

fn font_candidates() -> Vec<&'static str> {
    #[cfg(target_os = "windows")]
    {
        vec![
            "C:/Windows/Fonts/msyh.ttc",
            "C:/Windows/Fonts/msyhl.ttc",
            "C:/Windows/Fonts/simhei.ttf",
            "C:/Windows/Fonts/simsun.ttc",
        ]
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
        ]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        vec![
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        ]
    }
}

fn load_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for path in font_candidates() {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert("cjk".into(), FontData::from_owned(bytes).into());
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push("cjk".into());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}

pub fn setup(ctx: &egui::Context) {
    load_fonts(ctx);

    let mut style = Style::default();
    style.visuals = Visuals::dark();
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = BG_PANEL;
    style.visuals.extreme_bg_color = CARD;
    style.visuals.faint_bg_color = BG_PANEL;
    style.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.35);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    style.visuals.widgets.noninteractive.bg_fill = BG_PANEL;
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, CARD);
    style.visuals.widgets.inactive.bg_fill = CARD;
    style.visuals.widgets.active.bg_fill = CARD;
    style.visuals.widgets.open.bg_fill = CARD;
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT);
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);
    style.visuals.widgets.active.fg_stroke = egui::Stroke::new(2.0, ACCENT);
    style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.5, ACCENT);
    style.visuals.widgets.hovered.bg_fill = CARD;
    style.visuals.window_corner_radius = egui::CornerRadius::same(6);
    let r = egui::CornerRadius::same(4);
    style.visuals.widgets.noninteractive.corner_radius = r;
    style.visuals.widgets.inactive.corner_radius = r;
    style.visuals.widgets.hovered.corner_radius = r;
    style.visuals.widgets.active.corner_radius = r;
    style.visuals.widgets.open.corner_radius = r;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    // 整体字号调大（内置默认 12.5/10 对中文偏小）
    style.text_styles = [
        (egui::TextStyle::Heading, egui::FontId::proportional(22.0)),
        (egui::TextStyle::Body, egui::FontId::proportional(16.0)),
        (egui::TextStyle::Button, egui::FontId::proportional(15.0)),
        (egui::TextStyle::Small, egui::FontId::proportional(13.0)),
        (egui::TextStyle::Monospace, egui::FontId::monospace(14.0)),
    ]
    .into();
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}
