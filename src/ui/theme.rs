use eframe::egui::{Color32, Context, FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0x1c, 0x1d, 0x22);
pub const LINE: Color32 = Color32::from_rgba_premultiplied(36, 36, 36, 36); // rgba(255,255,255,.14)
pub const FG: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf4);
pub const FG2: Color32 = Color32::from_rgb(0xb4, 0xb4, 0xbc);
pub const FG3: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x94);
pub const OK: Color32 = Color32::from_rgb(0x2b, 0xd4, 0x5f);
pub const WARN: Color32 = Color32::from_rgb(0xff, 0xb8, 0x00);
/// King blob: gold body, paler crown, ruby jewel.
pub const KING: Color32 = Color32::from_rgb(0xf5, 0xb5, 0x2e);
pub const CROWN: Color32 = Color32::from_rgb(0xff, 0xe0, 0x7a);
pub const RUBY: Color32 = Color32::from_rgb(0xff, 0x3d, 0x6b);
pub const SESSION:[Color32; 6] = [
    Color32::from_rgb(0xff, 0x6b, 0x35),
    Color32::from_rgb(0x1e, 0x9b, 0xff),
    Color32::from_rgb(0xb8, 0x5c, 0xff),
    Color32::from_rgb(0xff, 0x3d, 0x8b),
    Color32::from_rgb(0x14, 0xc8, 0xc0),
    Color32::from_rgb(0x5b, 0x6c, 0xff),
];

pub fn setup_fonts(ctx: &Context) {
    let mut f = FontDefinitions::empty();
    for (name, bytes) in [
        ("geist", &include_bytes!("../../assets/fonts/Geist-Regular.ttf")[..]),
        ("geist-mono", &include_bytes!("../../assets/fonts/GeistMono-Regular.ttf")[..]),
    ] {
        f.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
        f.families.insert(FontFamily::Name(name.into()), vec![name.into()]);
    }
    f.families.insert(FontFamily::Proportional, vec!["geist".into()]);
    f.families.insert(FontFamily::Monospace, vec!["geist-mono".into()]);
    ctx.set_fonts(f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bg_and_line_values() {
        assert_eq!(BG, Color32::from_rgb(0x1c, 0x1d, 0x22));
        assert_eq!(LINE, Color32::from_white_alpha(36));
    }
}
