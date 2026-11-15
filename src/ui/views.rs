use super::Core;
use super::blob::{BlobState, paint_blob};
use super::theme::{BG, LINE, SESSION};
use eframe::egui::{self, Color32, Id, Pos2, Rect, Sense, Shadow, Stroke, StrokeKind, pos2, vec2};

/// The pill's top-left inside the window; the margin keeps the shadow from being cut off.
pub const MARGIN: f32 = 8.0;

const BLOB: f32 = 22.0;
const GAP: f32 = 6.0;
const PAD_Y: f32 = 10.0;
const PAD_X: f32 = 5.0;
const PILL_W: f32 = BLOB + 2.0 * PAD_X + 2.0;

/// Outer pill height for `n` blobs (island-left.html .vpill: 1 px border, 10 px padding, 22 px blobs, 6 px gap).
pub fn pill_height(n: usize) -> f32 {
    if n == 0 { 0.0 } else { 2.0 + 2.0 * PAD_Y + n as f32 * BLOB + (n - 1) as f32 * GAP }
}

/// True when `p` is inside the vertical capsule `r` (corner radius = half its width).
pub fn hit(p: Pos2, r: Rect) -> bool {
    let rad = r.width() / 2.0;
    let y = p.y.clamp(r.top() + rad, (r.bottom() - rad).max(r.top() + rad));
    p.distance(Pos2::new(r.center().x, y)) <= rad
}

pub fn pill(ui: &mut egui::Ui, core: &Core, t: f64) -> egui::Response {
    let h = ui.ctx().animate_value_with_time(Id::unique("pill_h"), pill_height(core.state.sessions.len()), 0.25);
    let r = Rect::from_min_size(pos2(MARGIN, MARGIN), vec2(PILL_W, h));
    let resp = ui.allocate_rect(r, Sense::click());
    if r.height() < 1.0 {
        return resp;
    }
    let p = ui.painter();
    let rad = PILL_W / 2.0;
    p.add(Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(115) }.as_shape(r, rad));
    p.rect(r, rad, BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let clip = p.with_clip_rect(r.shrink(1.0));
    for (i, s) in core.state.sessions.iter().enumerate() {
        let bottom = r.top() + 1.0 + PAD_Y + (i + 1) as f32 * BLOB + i as f32 * GAP;
        let color = SESSION[s.color % SESSION.len()];
        paint_blob(&clip, pos2(r.center().x, bottom), BLOB, color, BlobState::of(s.status, s.finished), t);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    #[test]
    fn pill_height_matches_mockup_box() {
        assert_eq!(pill_height(0), 0.0);
        assert_eq!(pill_height(1), 44.0);
        assert_eq!(pill_height(4), 2.0 + 20.0 + 4.0 * 22.0 + 3.0 * 6.0);
    }

    #[test]
    fn hit_respects_rounded_ends() {
        let r = Rect::from_min_size(pos2(8.0, 8.0), eframe::egui::vec2(34.0, 100.0));
        assert!(hit(pos2(25.0, 9.0), r), "top center");
        assert!(hit(pos2(9.0, 58.0), r), "left edge, straight part");
        assert!(!hit(pos2(9.0, 9.0), r), "outside the rounded corner");
        assert!(!hit(pos2(25.0, 109.0), r), "below");
        assert!(!hit(pos2(43.0, 58.0), r), "right of it");
    }
}
