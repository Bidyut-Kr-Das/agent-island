use super::Core;
use crate::state::{Session, Todo, TodoStatus};
use super::blob::{BlobState, paint_blob};
use super::theme::{BG, FG, FG2, FG3, LINE, OK, SESSION};
use crate::state::Status;
use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{self, Color32, FontFamily, FontId, Galley, Id, Painter, Pos2, Rect, Sense, Shadow, Shape, Stroke, StrokeKind, pos2, vec2};
use std::sync::Arc;

/// The pill's top-left inside the window; the margin keeps the shadow from being cut off.
pub const MARGIN: f32 = 8.0;

const BLOB: f32 = 22.0;
const GAP: f32 = 6.0;
const PAD_Y: f32 = 10.0;
const PAD_X: f32 = 5.0;
const PILL_W: f32 = BLOB + 2.0 * PAD_X + 2.0;
const PANEL_W: f32 = 300.0;
const WIN_H: f32 = 520.0;

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

pub fn pill(ui: &mut egui::Ui, core: &Core, selected: Option<&str>, t: f64) -> egui::Response {
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
        if selected == Some(s.id.as_str()) {
            // island-left.html .selbar: 2 x 12 px, 6 px left of the blob box, vertically centered on it.
            let bar = Rect::from_min_size(pos2(r.center().x - BLOB / 2.0 - 6.0, bottom - BLOB / 2.0 - 6.0), vec2(2.0, 12.0));
            p.rect_filled(bar, 1.0, FG);
        }
    }
    resp
}

/// The expanded panel for the selected session, right of `pill`. Closes (sets `selected` to None) when that
/// session is gone. Returns the panel's current on-screen rect, `Rect::NOTHING` when fully closed.
pub fn panel(ui: &mut egui::Ui, core: &Core, selected: &mut Option<String>, pill: Rect, t: f64) -> Rect {
    let find = |id: &str| core.state.sessions.iter().find(|s| s.id == id);
    if selected.as_deref().is_some_and(|id| find(id).is_none()) {
        *selected = None;
    }
    // (open last frame, time it last opened or closed, session to keep showing while closing)
    let key = Id::unique("panel");
    let (was, since, last) = ui.data(|d| d.get_temp::<(bool, f64, Option<String>)>(key)).unwrap_or((false, f64::MIN, None));
    let open = selected.is_some();
    let since = if open != was { t } else { since };
    let last = selected.clone().or(last);
    ui.data_mut(|d| d.insert_temp(key, (open, since, last.clone())));
    let k = spring(((t - since) / 0.25) as f32);
    let k = if open { k } else { 1.0 - k };
    let Some(s) = last.as_deref().and_then(find).filter(|_| k > 0.0) else { return Rect::NOTHING };

    // island-left.html .box: 1 px border, 14 / 16 px padding; todo rows 4 px padding, 14 px icon, 10 px gap.
    let inner = PANEL_W - 2.0 - 32.0;
    let text = |s: &str, size: f32, family: &str, color: Color32, width: f32| -> Arc<Galley> {
        let mut f = TextFormat::simple(FontId::new(size, FontFamily::Name(family.into())), color);
        if family == "geist-semibold" {
            f.extra_letter_spacing = -0.01 * size;
        }
        if family == "geist-mono" {
            f.line_height = Some(size * 1.4);
        }
        let mut job = LayoutJob::single_section(s.into(), f);
        job.wrap = TextWrapping { max_width: width, max_rows: 2, ..Default::default() };
        ui.painter().layout_job(job)
    };
    let title = text(&s.project, 14.0, "geist-semibold", FG, inner);
    let path = text(&s.cwd, 11.0, "geist-mono", FG3, inner);
    // icon: Some(true) check, Some(false) spinner, None none.
    let row = |icon: Option<bool>, s: &str| {
        let g = match icon {
            Some(true) => text(s, 13.0, "geist", FG2, inner - 24.0),
            Some(false) => text(s, 13.0, "geist-medium", FG, inner - 24.0),
            None => text(s, 13.0, "geist", FG3, inner),
        };
        (icon, g)
    };
    let mut rows: Vec<_> = visible_todos(&s.todos).into_iter().map(|td| row(Some(td.status == TodoStatus::Completed), &td.subject)).collect();
    if rows.is_empty() {
        match s.status {
            Status::Working => rows.push(row(Some(false), "Working")),
            Status::Idle => rows.push(row(None, "Idle")),
            Status::Waiting(_) => {}
        }
    }
    let heights: Vec<f32> = rows.iter().map(|(_, g)| g.size().y.max(14.0) + 8.0).collect();
    let head = 2.0 + 28.0 + title.size().y + 2.0 + path.size().y + if rows.is_empty() { 0.0 } else { 10.0 };
    let first = first_row(&heights, WIN_H - MARGIN - pill.top() - head);
    let full = panel_rect(pill, head + heights[first..].iter().sum::<f32>());

    let r = Rect::from_min_size(full.min, full.size() * (0.6 + 0.4 * k));
    let mut p = ui.painter().clone();
    p.multiply_opacity(k);
    p.add(Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(128) }.as_shape(r, 8.0));
    p.rect(r, 8.0, BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let p = p.with_clip_rect(r.shrink(1.0));
    let x = full.left() + 17.0;
    let mut y = full.top() + 15.0;
    p.galley(pos2(x, y), title.clone(), FG);
    y += title.size().y + 2.0;
    p.galley(pos2(x, y), path.clone(), FG3);
    y += path.size().y + 10.0;
    for ((icon, g), h) in rows.into_iter().zip(heights).skip(first) {
        let c = pos2(x + 7.0, y + h / 2.0);
        match icon {
            Some(true) => check(&p, c),
            Some(false) => spinner(&p, c, t),
            None => {}
        }
        let tx = if icon.is_some() { x + 24.0 } else { x };
        p.galley(pos2(tx, y + (h - g.size().y) / 2.0), g, FG);
        y += h;
    }
    r
}

/// 14 px green circle with a dark check (island-left.html .chk).
fn check(p: &Painter, c: Pos2) {
    p.circle_filled(c, 7.0, OK);
    let pts = vec![c + vec2(-3.0, 0.2), c + vec2(-1.0, 2.2), c + vec2(3.0, -2.0)];
    p.add(Shape::line(pts, Stroke::new(1.6, BG)));
}

/// 14 px ring, 2 px wide, top quarter bright, one turn per 0.8 s (island-left.html .spin).
fn spinner(p: &Painter, c: Pos2, t: f64) {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    p.circle_stroke(c, 6.0, Stroke::new(2.0, Color32::from_rgb(0x34, 0x34, 0x3a)));
    let a0 = (t / 0.8).fract() as f32 * TAU - 0.75 * PI;
    let pts = (0..=12).map(|i| c + 6.0 * vec2((a0 + i as f32 / 12.0 * FRAC_PI_2).cos(), (a0 + i as f32 / 12.0 * FRAC_PI_2).sin()));
    p.add(Shape::line(pts.collect(), Stroke::new(2.0, FG)));
}

pub fn visible_todos(todos: &[Todo]) -> Vec<&Todo> {
    todos.iter().filter(|t| t.status != TodoStatus::Pending).collect()
}

/// The session that changed most recently.
pub fn default_selection(sessions: &[Session]) -> Option<String> {
    sessions.iter().max_by_key(|s| s.changed_at).map(|s| s.id.clone())
}

/// New selection after a click on the pill: on blob `clicked`, or on its padding (`None`).
pub fn toggle(selected: Option<&str>, clicked: Option<&str>, default: Option<String>) -> Option<String> {
    match (selected, clicked) {
        (Some(s), Some(c)) if s == c => None,
        (_, Some(c)) => Some(c.to_string()),
        (Some(_), None) => None,
        (None, None) => default,
    }
}

/// A new left-button press with the cursor outside the island's shapes.
pub fn outside_click(was_down: bool, down: bool, inside: bool) -> bool {
    down && !was_down && !inside
}

/// Cursor over the pill or the open panel (ponytail: panel corners count as square).
pub fn inside(p: Pos2, pill: Rect, panel: Rect) -> bool {
    hit(p, pill) || panel.contains(p)
}

/// Index of the blob whose 22 px box contains `p.y`.
pub fn blob_at(p: Pos2, pill: Rect, n: usize) -> Option<usize> {
    let rel = p.y - pill.top() - 1.0 - PAD_Y;
    let i = (rel / (BLOB + GAP)).floor();
    (rel >= 0.0 && (i as usize) < n && rel - i * (BLOB + GAP) <= BLOB).then_some(i as usize)
}

/// Panel at full size: 8 px right of the pill, top-aligned, no taller than the window allows.
pub fn panel_rect(pill: Rect, h: f32) -> Rect {
    Rect::from_min_size(pos2(pill.right() + 8.0, pill.top()), vec2(PANEL_W, h.min(WIN_H - MARGIN - pill.top())))
}

/// Index of the first row to draw so the last rows fit into `avail`.
pub fn first_row(heights: &[f32], avail: f32) -> usize {
    let mut sum = 0.0;
    for (i, h) in heights.iter().enumerate().rev() {
        sum += h;
        if sum > avail {
            return i + 1;
        }
    }
    0
}

/// Critically damped spring from 0 to 1 over normalized time `x` (settled at 1).
pub fn spring(x: f32) -> f32 {
    let f = |x: f32| 1.0 - (1.0 + 8.0 * x) * (-8.0 * x).exp();
    if x >= 1.0 { 1.0 } else { f(x.max(0.0)) / f(1.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;


    fn todo(id: &str, st: TodoStatus) -> Todo {
        Todo { id: id.into(), subject: id.into(), status: st }
    }

    fn sess(id: &str, changed_at: u64) -> Session {
        Session {
            id: id.into(), project: id.into(), cwd: String::new(), transcript: String::new(), color: 0, status: Status::Idle,
            todos: vec![], finished: None, title: None, claude_pid: None, terminal_pid: None, changed_at,
        }
    }

    #[test]
    fn visible_todos_keeps_completed_and_in_progress_in_order() {
        let t = [todo("a", TodoStatus::Completed), todo("b", TodoStatus::Pending), todo("c", TodoStatus::InProgress), todo("d", TodoStatus::Completed)];
        let ids: Vec<&str> = visible_todos(&t).iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "c", "d"]);
    }

    #[test]
    fn default_selection_is_latest_changed() {
        assert_eq!(default_selection(&[]), None);
        assert_eq!(default_selection(&[sess("a", 5), sess("b", 9), sess("c", 7)]).as_deref(), Some("b"));
    }

    #[test]
    fn toggle_selects_switches_and_closes() {
        let d = || Some("d".to_string());
        assert_eq!(toggle(None, Some("a"), d()).as_deref(), Some("a"), "open on clicked blob");
        assert_eq!(toggle(Some("a"), Some("b"), d()).as_deref(), Some("b"), "switch");
        assert_eq!(toggle(Some("a"), Some("a"), d()), None, "same blob closes");
        assert_eq!(toggle(None, None, d()).as_deref(), Some("d"), "pill padding opens default");
        assert_eq!(toggle(Some("a"), None, d()), None, "pill padding closes");
    }

    #[test]
    fn outside_click_only_on_press_edge_outside() {
        assert!(outside_click(false, true, false));
        assert!(!outside_click(true, true, false), "held, not a new press");
        assert!(!outside_click(false, true, true), "press inside");
        assert!(!outside_click(false, false, false));
    }

    #[test]
    fn panel_sits_8px_right_of_pill_top_aligned_and_capped() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, 100.0));
        let r = panel_rect(pill, 200.0);
        assert_eq!(r, Rect::from_min_size(pos2(50.0, 8.0), vec2(300.0, 200.0)));
        assert_eq!(panel_rect(pill, 900.0).bottom(), 520.0 - MARGIN, "fits the window");
    }

    #[test]
    fn inside_covers_pill_and_panel_only() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, 100.0));
        let panel = panel_rect(pill, 200.0);
        assert!(inside(pos2(25.0, 50.0), pill, panel));
        assert!(inside(pos2(200.0, 150.0), pill, panel));
        assert!(!inside(pos2(46.0, 50.0), pill, panel), "the gap between");
        assert!(!inside(pos2(200.0, 300.0), pill, panel), "below the panel");
        assert!(!inside(pos2(200.0, 150.0), pill, Rect::NOTHING), "closed panel");
    }

    #[test]
    fn blob_at_maps_blob_boxes() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, pill_height(3)));
        // Blob i spans y = 8 + 1 + 10 + i * 28 .. + 22.
        assert_eq!(blob_at(pos2(25.0, 19.0), pill, 3), Some(0));
        assert_eq!(blob_at(pos2(25.0, 19.0 + 28.0 + 11.0), pill, 3), Some(1));
        assert_eq!(blob_at(pos2(25.0, 19.0 + 56.0 + 21.0), pill, 3), Some(2));
        assert_eq!(blob_at(pos2(25.0, 19.0 + 24.0), pill, 3), None, "gap");
        assert_eq!(blob_at(pos2(25.0, 12.0), pill, 3), None, "top padding");
        assert_eq!(blob_at(pos2(25.0, 19.0 + 84.0), pill, 3), None, "below the last blob");
    }

    #[test]
    fn first_row_keeps_the_last_rows_that_fit() {
        assert_eq!(first_row(&[20.0, 20.0, 20.0], 100.0), 0);
        assert_eq!(first_row(&[20.0, 20.0, 20.0], 40.0), 1);
        assert_eq!(first_row(&[20.0, 36.0, 20.0], 50.0), 2);
        assert_eq!(first_row(&[], 10.0), 0);
    }

    #[test]
    fn spring_settles_monotonically() {
        assert_eq!(spring(0.0), 0.0);
        assert_eq!(spring(1.0), 1.0);
        assert_eq!(spring(2.0), 1.0);
        let v: Vec<f32> = (0..=10).map(|i| spring(i as f32 / 10.0)).collect();
        assert!(v.windows(2).all(|w| w[1] >= w[0]), "no overshoot: {v:?}");
        assert!(v[5] > 0.8, "critically damped, mostly there by half time: {}", v[5]);
    }

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
