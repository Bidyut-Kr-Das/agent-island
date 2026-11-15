use super::Core;
use crate::settings::Side;
use crate::state::{Finish, Permission, Session, Todo, TodoStatus, WaitKind};
use std::collections::VecDeque;
use std::hash::Hash;
use super::blob::{BlobState, paint_blob};
use super::theme::{BG, FG, FG2, FG3, LINE, OK, SESSION, WARN};
use crate::hook::Reply;
use crate::state::Status;
use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{self, Color32, FontFamily, FontId, Galley, Id, Painter, Pos2, Rect, Sense, Shadow, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};
use std::sync::Arc;

/// The pill's top-left inside the window; the margin keeps the shadow from being cut off.
pub const MARGIN: f32 = 8.0;

const BLOB: f32 = 24.0;
const GAP: f32 = 6.0;
const PAD_Y: f32 = 10.0;
const PAD_X: f32 = 5.0;
const PILL_W: f32 = BLOB + 2.0 * PAD_X + 2.0;
const PANEL_W: f32 = 300.0;
const CARD_W: f32 = 280.0;
pub const WIN_W: f32 = 400.0;
pub const WIN_H: f32 = 520.0;

/// Left x of the pill inside the window.
pub fn pill_x(side: Side) -> f32 {
    match side {
        Side::Left => MARGIN,
        Side::Right => WIN_W - MARGIN - PILL_W,
    }
}

/// Left x of a box `w` wide beside the pill, 8 px away, on the side away from the screen edge.
pub fn beside(pill: Rect, w: f32, side: Side) -> f32 {
    match side {
        Side::Left => pill.right() + 8.0,
        Side::Right => pill.left() - 8.0 - w,
    }
}

/// Outer pill height for `n` blobs (island-left.html .vpill: 1 px border, 10 px padding, 24 px blobs, 6 px gap).
pub fn pill_height(n: usize) -> f32 {
    if n == 0 { 0.0 } else { 2.0 + 2.0 * PAD_Y + n as f32 * BLOB + (n - 1) as f32 * GAP }
}

/// True when `p` is inside the vertical capsule `r` (corner radius = half its width).
pub fn hit(p: Pos2, r: Rect) -> bool {
    let rad = r.width() / 2.0;
    let y = p.y.clamp(r.top() + rad, (r.bottom() - rad).max(r.top() + rad));
    p.distance(Pos2::new(r.center().x, y)) <= rad
}

pub fn pill(ui: &mut egui::Ui, core: &Core, selected: Option<&str>, side: Side, t: f64) -> egui::Response {
    let h = ui.ctx().animate_value_with_time(Id::unique("pill_h"), pill_height(core.state.sessions.len()), 0.25);
    let r = Rect::from_min_size(pos2(pill_x(side), MARGIN), vec2(PILL_W, h));
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
            // island-left.html .selbar: 2 x 12 px, 6 px outside the blob box (mirrored on the right), vertically centered on it.
            let bx = if side == Side::Left { r.center().x - BLOB / 2.0 - 6.0 } else { r.center().x + BLOB / 2.0 + 4.0 };
            let bar = Rect::from_min_size(pos2(bx, bottom - BLOB / 2.0 - 6.0), vec2(2.0, 12.0));
            p.rect_filled(bar, 1.0, FG);
        }
    }
    resp
}

/// The expanded panel for the selected session, right of `pill`. Closes (sets `selected` to None) when that
/// session is gone. Returns the panel's current on-screen rect (`Rect::NOTHING` when fully closed) and
/// whether "Go to terminal" was clicked.
pub fn panel(ui: &mut egui::Ui, core: &Core, selected: &mut Option<String>, pill: Rect, side: Side, t: f64) -> (Rect, bool) {
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
    let Some(s) = last.as_deref().and_then(find).filter(|_| k > 0.0) else { return (Rect::NOTHING, false) };

    // island-left.html .box: 1 px border, 14 / 16 px padding; todo rows 4 px padding, 14 px icon, 10 px gap.
    let inner = PANEL_W - 2.0 - 32.0;
    let text = |s: &str, size: f32, family: &str, color: Color32, width: f32| layout(ui.painter(), &[(s, fmt(size, family, color))], width, 2);
    let title = text(&s.project, 14.0, "geist", SESSION[s.color % SESSION.len()], inner - 30.0);
    let rows: Vec<_> = panel_rows(s)
        .into_iter()
        .map(|(icon, label)| {
            let g = match icon {
                RowIcon::Check | RowIcon::Dot => text(&label, 13.0, "geist", FG3, inner - 24.0),
                RowIcon::Spinner => text(&label, 13.0, "geist", FG, inner - 24.0),
                RowIcon::None => text(&label, 13.0, "geist", FG3, inner),
            };
            (icon, g)
        })
        .collect();
    let heights: Vec<f32> = rows.iter().map(|(_, g)| g.size().y.max(14.0) + 8.0).collect();
    let head = 2.0 + 28.0 + title.size().y + if rows.is_empty() { 0.0 } else { 10.0 };
    let first = first_row(&heights, WIN_H - MARGIN - pill.top() - head);
    let full = panel_rect(pill, head + heights[first..].iter().sum::<f32>(), side);

    let r = grow(full, k, side);
    let mut p = ui.painter().clone();
    p.multiply_opacity(k);
    p.add(Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(128) }.as_shape(r, 8.0));
    p.rect(r, 8.0, BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let p = p.with_clip_rect(r.shrink(1.0));
    let x = full.left() + 17.0;
    let mut y = full.top() + 15.0;
    p.galley(pos2(x, y), title.clone(), FG);
    y += title.size().y + 10.0;
    for ((icon, g), h) in rows.into_iter().zip(heights).skip(first) {
        let c = pos2(x + 7.0, y + h / 2.0);
        match icon {
            RowIcon::Check => check(&p, c),
            RowIcon::Spinner => spinner(&p, c, t),
            RowIcon::Dot => {
                p.circle_filled(c, 3.0, FG3);
            }
            RowIcon::None => {}
        }
        let tx = if icon == RowIcon::None { x } else { x + 24.0 };
        p.galley(pos2(tx, y + (h - g.size().y) / 2.0), g, FG);
        y += h;
    }
    // Go to terminal: only once fully open, and only when the session has a terminal.
    let mut go = false;
    if s.terminal_pid.is_some() {
        let b = terminal_button_rect(full);
        p.rect(b, 4.0, BTN_BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
        let (c, st) = (b.center(), Stroke::new(1.5, FG));
        p.add(Shape::line(vec![c + vec2(-4.0, -3.5), c + vec2(-0.5, 0.0), c + vec2(-4.0, 3.5)], st));
        p.line_segment([c + vec2(1.5, 4.0), c + vec2(5.0, 4.0)], st);
        if k >= 1.0 {
            go = ui.interact(b, Id::unique("go_terminal"), Sense::click()).on_hover_text("Go to terminal").clicked();
        }
    }
    (r, go)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowIcon {
    Check,
    Spinner,
    Dot,
    None,
}

/// Panel rows: the session's todos (completed, in progress); in progress only spins while working.
pub fn panel_rows(s: &Session) -> Vec<(RowIcon, String)> {
    let working = s.status == Status::Working;
    let mut rows: Vec<_> = visible_todos(&s.todos)
        .into_iter()
        .map(|td| {
            let icon = match (td.status, working) {
                (TodoStatus::Completed, _) => RowIcon::Check,
                (_, true) => RowIcon::Spinner,
                _ => RowIcon::Dot,
            };
            (icon, td.subject.clone())
        })
        .collect();
    if rows.is_empty() {
        match s.status {
            Status::Working => rows.push((RowIcon::Spinner, "Working".into())),
            Status::Idle => rows.push((RowIcon::None, "No tasks recorded".into())),
            Status::Waiting(_) => {}
        }
    }
    rows
}

/// 22 px "Go to terminal" button in the panel's top-right corner.
pub fn terminal_button_rect(panel: Rect) -> Rect {
    Rect::from_min_size(pos2(panel.right() - 15.0 - 22.0, panel.top() + 12.0), vec2(22.0, 22.0))
}

/// Text format for the mockup's type styles (mono has 1.4 line height).
fn fmt(size: f32, family: &str, color: Color32) -> TextFormat {
    let mut f = TextFormat::simple(FontId::new(size, FontFamily::Name(family.into())), color);
    if family == "geist-mono" {
        f.line_height = Some(size * 1.4);
    }
    f
}

/// Lays out `parts` within `width`, at most `rows` rows, cut with an ellipsis.
fn layout(p: &Painter, parts: &[(&str, TextFormat)], width: f32, rows: usize) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    for (s, f) in parts {
        job.append(s, 0.0, f.clone());
    }
    job.wrap = TextWrapping { max_width: width, max_rows: rows, break_anywhere: rows == 1, ..Default::default() };
    p.layout_job(job)
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

/// Cursor over the pill or the open panel, card or label (ponytail: their corners count as square).
pub fn inside(p: Pos2, pill: Rect, panel: Rect) -> bool {
    hit(p, pill) || panel.contains(p)
}

/// Window region in physical px: `[left, top, right, bottom, corner diameter]` for the pill and each
/// shown shape beside it as (rect, corner radius), skipping empty ones.
pub fn region(pill: Rect, others: &[(Rect, f32)], ppp: f32) -> Vec<[i32; 5]> {
    let px = |v: f32| (v * ppp).round() as i32;
    let pill = (pill.height() >= 1.0).then_some((pill, pill.width() / 2.0));
    let others = others.iter().copied().filter(|(r, _)| r.is_positive());
    pill.into_iter().chain(others).map(|(r, rad)| [px(r.left()), px(r.top()), px(r.right()), px(r.bottom()), px(2.0 * rad)]).collect()
}

/// Index of the blob whose BLOB-sized box contains `p.y`.
pub fn blob_at(p: Pos2, pill: Rect, n: usize) -> Option<usize> {
    let rel = p.y - pill.top() - 1.0 - PAD_Y;
    let i = (rel / (BLOB + GAP)).floor();
    (rel >= 0.0 && (i as usize) < n && rel - i * (BLOB + GAP) <= BLOB).then_some(i as usize)
}

/// Panel at full size: 8 px right of the pill, top-aligned, no taller than the window allows.
pub fn panel_rect(pill: Rect, h: f32, side: Side) -> Rect {
    Rect::from_min_size(pos2(beside(pill, PANEL_W, side), pill.top()), vec2(PANEL_W, h.min(WIN_H - MARGIN - pill.top())))
}

/// `full` scaled to 60..100% by `k`, anchored at its edge nearest the pill.
pub fn grow(full: Rect, k: f32, side: Side) -> Rect {
    let size = full.size() * (0.6 * (1.0 - k) + k);
    let x = if side == Side::Right { full.right() - size.x } else { full.left() };
    Rect::from_min_size(pos2(x, full.top()), size)
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

const BTN_BG: Color32 = Color32::from_rgb(0x1d, 0x1d, 0x21);
const CMD_BG: Color32 = Color32::from_rgb(0x17, 0x17, 0x1a);
const DENY: Color32 = Color32::from_rgb(0xff, 0x4d, 0x4d);
const ALLOW_FG: Color32 = Color32::from_rgb(0x06, 0x14, 0x0d);

/// A card button: label, fill, text color, 1 px border.
type Btn<'a> = (&'a str, Color32, Color32, bool);

/// Card beside session `s`'s blob (island-blob.html states 4 and 5, island-left.html state 3 placement):
/// waiting blob and project, `meta`, optional command box, buttons. Returns its rect and the clicked button.
#[allow(clippy::too_many_arguments)]
fn card(ui: &mut egui::Ui, core: &Core, s: &Session, key: impl Hash + std::fmt::Debug + Copy, meta: &str, corner: Option<String>, cmd: Option<&Permission>, btns: &[Btn], armed: bool, pill: Rect, side: Side, t: f64) -> (Rect, Option<usize>) {
    let p = ui.painter().clone();
    let inner = CARD_W - 2.0 - 32.0;
    let corner = corner.map(|c| layout(&p, &[(&c, fmt(11.0, "geist", FG3))], inner, 1));
    let cw = corner.as_ref().map_or(0.0, |g| g.size().x + 8.0);
    let title = layout(&p, &[(&s.project, fmt(14.0, "geist", SESSION[s.color % SESSION.len()]))], inner - 30.0 - cw, 1);
    let meta = layout(&p, &[(meta, fmt(12.0, "geist", FG2))], inner, 2);
    let cmd = cmd.map(|c| {
        let mut f = fmt(12.0, "geist-mono", FG);
        f.line_height = Some(12.0 * 1.3);
        let tool = TextFormat { color: WARN, ..f.clone() };
        (c, layout(&p, &[(&c.tool, tool), ("\u{a0} ", f.clone()), (&c.detail, f)], inner - 22.0, 1))
    });
    let cmd_h = cmd.as_ref().map_or(0.0, |(_, g)| 10.0 + 18.0 + g.size().y);
    let btn_h = 31.0;
    let h = 15.0 + 20.0 + 6.0 + meta.size().y + cmd_h + 12.0 + btn_h + 15.0;
    let i = core.state.sessions.iter().position(|x| x.id == s.id).unwrap_or(0);
    let r = card_rect(pill, i, h, side);

    p.add(Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(128) }.as_shape(r, 8.0));
    p.rect(r, 8.0, BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let x = r.left() + 17.0;
    let mut y = r.top() + 15.0;
    paint_blob(&p, pos2(x + 10.0, y + 20.0), 20.0, SESSION[s.color % SESSION.len()], BlobState::Waiting, t);
    p.galley(pos2(x + 30.0, y + 10.0 - title.size().y / 2.0), title, FG);
    if let Some(g) = corner {
        p.galley(pos2(x + inner - g.size().x, y + 10.0 - g.size().y / 2.0), g, FG3);
    }
    y += 26.0;
    p.galley(pos2(x, y), meta.clone(), FG2);
    y += meta.size().y;
    if let Some((c, g)) = cmd {
        y += 10.0;
        let b = Rect::from_min_size(pos2(x, y), vec2(inner, cmd_h - 10.0));
        p.rect(b, 4.0, CMD_BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
        p.galley(pos2(x + 11.0, y + 9.0), g, FG);
        ui.interact(b, Id::unique(("cmd", c.req_id)), Sense::hover()).on_hover_text(&c.full);
        y += cmd_h - 10.0;
    }
    y += 12.0;
    let n = btns.len() as f32;
    let w = (inner - 8.0 * (n - 1.0)) / n;
    let mut clicked = None;
    let mut p = p;
    if !armed {
        p.multiply_opacity(0.6);
    }
    for (k, &(label, fill, color, border)) in btns.iter().enumerate() {
        let b = Rect::from_min_size(pos2(x + k as f32 * (w + 8.0), y), vec2(w, btn_h));
        let stroke = if border { Stroke::new(1.0, LINE) } else { Stroke::NONE };
        p.rect(b, 4.0, fill, stroke, StrokeKind::Inside);
        let g = layout(&p, &[(label, fmt(12.5, "geist", color))], w, 1);
        p.galley(b.center() - g.size() / 2.0, g, color);
        if ui.interact(b, btn_id(key, k), Sense::click()).clicked() {
            clicked = clicked.or(Some(k));
        }
    }
    (r, clicked)
}

/// Id of button `k` on the card for `key` (the request id on permission cards), so a click pressed on
/// one request can never land on the next request that takes the card's place.
pub fn btn_id(key: impl Hash + std::fmt::Debug, k: usize) -> Id {
    Id::unique(("btn", key, k))
}

/// Pointer history for the permission card showing `req`, so a click the user was already making
/// where the card appeared can never answer it.
#[derive(Clone, Copy, Debug)]
pub struct Arming {
    pub req: u64,
    pub appeared: f64,
    /// The cursor has been off the card since it appeared (or since the card last slid under it).
    outside: bool,
    /// The cursor moved onto the card from outside after it appeared.
    pub entered: bool,
    /// Cursor position last frame.
    last: Option<Pos2>,
}

impl Arming {
    pub fn new(req: u64, appeared: f64, over: bool, cursor: Option<Pos2>) -> Arming {
        Arming { req, appeared, outside: !over, entered: false, last: cursor }
    }

    /// Feeds whether the cursor is over the card this frame, and where it is. Coming over the card
    /// without the cursor moving means the card moved under it: that needs a fresh leave and re-enter.
    pub fn update(&mut self, over: bool, cursor: Option<Pos2>) {
        let moved = cursor != self.last;
        self.last = cursor;
        if !over {
            self.outside = true;
        } else if self.outside && moved {
            self.entered = true;
        } else {
            self.outside = false;
        }
    }
}

/// Allow/Deny count a click only if the cursor entered the card after it appeared and the press began
/// at least 600 ms after it appeared.
pub fn armed(appeared: f64, press_started: f64, entered: bool) -> bool {
    entered && press_started >= appeared + 0.6
}

/// Start time of the latest press: this frame if a button went down in it (press and release may share a
/// frame), else egui's press start while held, else the previous value (egui clears it on release).
pub fn press_time(prev: f64, pressed_now: bool, start: Option<f64>, now: f64) -> f64 {
    if pressed_now { now } else { start.unwrap_or(prev) }
}

/// Permission card for `req_id` (spec 7.4). Returns its rect and the user's answer, if clicked this frame.
/// `armed` only dims the buttons; the caller decides whether a click counts.
pub fn permission_card(ui: &mut egui::Ui, core: &Core, req_id: u64, armed: bool, pill: Rect, side: Side, t: f64) -> (Rect, Option<Reply>) {
    let st = &core.state;
    let Some(perm) = st.permissions.iter().find(|p| p.req_id == req_id) else { return (Rect::NOTHING, None) };
    let Some(s) = st.sessions.iter().find(|s| s.id == perm.session_id) else { return (Rect::NOTHING, None) };
    let btns = [("Deny", BTN_BG, DENY, true), ("Allow", OK, ALLOW_FG, false)];
    let (r, k) = card(ui, core, s, req_id, &perm.verb, of_n(&st.permissions), Some(perm), &btns, armed, pill, side, t);
    (r, k.map(|k| if k == 0 { Reply::Deny } else { Reply::Allow }))
}

/// Waiting card for session `id` (spec 7.5). Returns its rect and whether "Go to terminal" was clicked.
pub fn waiting_card(ui: &mut egui::Ui, core: &Core, id: &str, pill: Rect, side: Side, t: f64) -> (Rect, bool) {
    let Some(s) = core.state.sessions.iter().find(|s| s.id == id) else { return (Rect::NOTHING, false) };
    let (r, k) = card(ui, core, s, id, waiting_text(s.status), None, None, &[("Go to terminal", BTN_BG, FG, true)], true, pill, side, t);
    (r, k.is_some())
}

/// Finished label beside the topmost finished blob (spec 7.6, island-left.html state 4).
/// Returns its rect and the session to open when clicked.
pub fn finished_pill(ui: &mut egui::Ui, core: &Core, pill: Rect, side: Side) -> (Rect, Option<String>) {
    let sessions = &core.state.sessions;
    let Some((i, bold, muted)) = finished_label(sessions) else { return (Rect::NOTHING, None) };
    let p = ui.painter().clone();
    let g = layout(&p, &[(&bold, fmt(12.5, "geist", SESSION[sessions[i].color % SESSION.len()])), (" ", fmt(12.5, "geist", FG2)), (muted, fmt(12.5, "geist", FG2))], f32::INFINITY, 1);
    let r = label_rect(pill, i, g.size() + vec2(26.0, 14.0), side);
    p.add(Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(115) }.as_shape(r, r.height() / 2.0));
    p.rect(r, r.height() / 2.0, BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
    p.galley(r.center() - g.size() / 2.0, g, FG);
    let clicked = ui.interact(r, Id::unique("finished"), Sense::click()).clicked();
    (r, clicked.then(|| sessions[i].id.clone()))
}

#[derive(Debug, PartialEq)]
pub enum View {
    Pill,
    Panel,
    Permission(u64),
    Waiting(String),
}

/// Oldest pending permission, then any waiting session, then the panel if expanded, else the bare pill.
pub fn current_view(core: &Core, expanded: bool) -> View {
    let st = &core.state;
    if let Some(p) = st.permissions.iter().find(|p| !p.timed_out) {
        View::Permission(p.req_id)
    } else if let Some(s) = st.sessions.iter().find(|s| matches!(s.status, Status::Waiting(_))) {
        View::Waiting(s.id.clone())
    } else if expanded {
        View::Panel
    } else {
        View::Pill
    }
}

pub fn waiting_text(s: Status) -> &'static str {
    if s == Status::Waiting(WaitKind::Question) { "Has a question for you" } else { "Waiting for you" }
}

/// "1 of N" when more than one permission is pending (the card always shows the oldest).
pub fn of_n(q: &VecDeque<Permission>) -> Option<String> {
    let n = q.iter().filter(|p| !p.timed_out).count();
    (n > 1).then(|| format!("1 of {n}"))
}

/// (index of the topmost finished blob, bold part (session color), muted part) for the finished label.
pub fn finished_label(sessions: &[Session]) -> Option<(usize, String, &'static str)> {
    let done: Vec<usize> = (0..sessions.len()).filter(|&i| BlobState::of(sessions[i].status, sessions[i].finished) == BlobState::Done).collect();
    let &i = done.first()?;
    Some(match (done.len(), sessions[i].finished) {
        (1, Some(Finish::Error)) => (i, sessions[i].project.clone(), "stopped with an error"),
        (1, _) => (i, sessions[i].project.clone(), "finished"),
        (n, _) => (i, n.to_string(), "finished"),
    })
}

fn blob_top(pill: Rect, i: usize) -> f32 {
    pill.top() + 1.0 + PAD_Y + i as f32 * (BLOB + GAP)
}

/// Card 8 px right of the pill, top-aligned with blob `i`, moved up if needed to fit the window.
pub fn card_rect(pill: Rect, i: usize, h: f32, side: Side) -> Rect {
    let top = blob_top(pill, i).min(WIN_H - MARGIN - h).max(MARGIN);
    Rect::from_min_size(pos2(beside(pill, CARD_W, side), top), vec2(CARD_W, h))
}

/// Label 8 px right of the pill, vertically centered on blob `i`.
pub fn label_rect(pill: Rect, i: usize, size: Vec2, side: Side) -> Rect {
    Rect::from_min_size(pos2(beside(pill, size.x, side), blob_top(pill, i) + BLOB / 2.0 - size.y / 2.0), size)
}

/// Settings card beside the pill, top-aligned with it: the title and a "Side" row of Left / Right buttons.
/// Returns its rect and the side clicked this frame.
pub fn settings_card(ui: &mut egui::Ui, pill: Rect, side: Side) -> (Rect, Option<Side>) {
    let p = ui.painter().clone();
    let h = 15.0 + 20.0 + 12.0 + 28.0 + 15.0;
    let r = Rect::from_min_size(pos2(beside(pill, CARD_W, side), pill.top().min(WIN_H - MARGIN - h).max(MARGIN)), vec2(CARD_W, h));
    p.add(Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(128) }.as_shape(r, 8.0));
    p.rect(r, 8.0, BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let (x, mut y) = (r.left() + 17.0, r.top() + 15.0);
    let title = layout(&p, &[("Settings", fmt(14.0, "geist", FG))], f32::INFINITY, 1);
    p.galley(pos2(x, y + 10.0 - title.size().y / 2.0), title, FG);
    y += 20.0 + 12.0;
    let label = layout(&p, &[("Side", fmt(12.5, "geist", FG2))], f32::INFINITY, 1);
    p.galley(pos2(x, y + 14.0 - label.size().y / 2.0), label, FG2);
    let mut clicked = None;
    for (k, (name, s)) in [("Left", Side::Left), ("Right", Side::Right)].into_iter().enumerate() {
        let b = Rect::from_min_size(pos2(r.right() - 17.0 - 136.0 + k as f32 * 72.0, y), vec2(64.0, 28.0));
        let fill = if s == side { Color32::from_white_alpha(26) } else { BTN_BG };
        p.rect(b, 4.0, fill, Stroke::new(1.0, LINE), StrokeKind::Inside);
        let g = layout(&p, &[(name, fmt(12.5, "geist", FG))], 64.0, 1);
        p.galley(b.center() - g.size() / 2.0, g, FG);
        if ui.interact(b, Id::unique(("side", name)), Sense::click()).clicked() {
            clicked = Some(s);
        }
    }
    (r, clicked)
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
            todos: vec![], finished: None, claude_pid: None, terminal_pid: None, changed_at,
        }
    }

    #[test]
    fn visible_todos_keeps_completed_and_in_progress_in_order() {
        let t = [todo("a", TodoStatus::Completed), todo("b", TodoStatus::Pending), todo("c", TodoStatus::InProgress), todo("d", TodoStatus::Completed)];
        let ids: Vec<&str> = visible_todos(&t).iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "c", "d"]);
    }

    fn sess_with(status: Status, todos: Vec<Todo>) -> Session {
        Session { status, todos, ..sess("a", 1) }
    }

    #[test]
    fn idle_session_rows_show_todos_not_idle() {
        let s = sess_with(Status::Idle, vec![todo("a", TodoStatus::Completed), todo("b", TodoStatus::Pending), todo("c", TodoStatus::Completed)]);
        let rows = panel_rows(&s);
        assert_eq!(rows, vec![(RowIcon::Check, "a".to_string()), (RowIcon::Check, "c".to_string())]);
        assert!(rows.iter().all(|(_, t)| t != "Idle"));
    }

    #[test]
    fn no_todos_idle_shows_no_tasks_recorded() {
        assert_eq!(panel_rows(&sess_with(Status::Idle, vec![])), vec![(RowIcon::None, "No tasks recorded".to_string())]);
        assert_eq!(panel_rows(&sess_with(Status::Working, vec![])), vec![(RowIcon::Spinner, "Working".to_string())]);
    }

    #[test]
    fn in_progress_row_static_when_not_working() {
        let t = vec![todo("a", TodoStatus::InProgress)];
        assert_eq!(panel_rows(&sess_with(Status::Working, t.clone()))[0].0, RowIcon::Spinner);
        assert_eq!(panel_rows(&sess_with(Status::Idle, t))[0].0, RowIcon::Dot);
    }

    #[test]
    fn terminal_button_inside_panel_hit_region_top_right() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, 100.0));
        let panel = panel_rect(pill, 120.0, Side::Left);
        let b = terminal_button_rect(panel);
        assert!(panel.contains_rect(b));
        assert!(b.center().x > panel.center().x && b.center().y < panel.center().y, "top-right");
        let reg = region(pill, &[(panel, 8.0)], 1.0)[1];
        assert!(b.left() >= reg[0] as f32 && b.right() <= reg[2] as f32 && b.top() >= reg[1] as f32 && b.bottom() <= reg[3] as f32);
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
        let r = panel_rect(pill, 200.0, Side::Left);
        assert_eq!(r, Rect::from_min_size(pos2(50.0, 8.0), vec2(300.0, 200.0)));
        assert_eq!(panel_rect(pill, 900.0, Side::Left).bottom(), 520.0 - MARGIN, "fits the window");
    }

    #[test]
    fn region_pill_only_is_one_capsule_in_pixels() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, 100.0));
        assert_eq!(region(pill, &[(Rect::NOTHING, 8.0)], 1.25), vec![[10, 10, 53, 135, 43]]);
        assert_eq!(region(Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, 0.0)), &[], 1.0), Vec::<[i32; 5]>::new(), "no sessions: empty");
    }

    #[test]
    fn region_pill_and_panel() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, 100.0));
        let panel = panel_rect(pill, 200.0, Side::Left);
        assert_eq!(region(pill, &[(panel, 8.0)], 1.0), vec![[8, 8, 42, 108, 34], [50, 8, 350, 208, 16]]);
    }

    #[test]
    fn region_pill_card_and_label() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, 100.0));
        let card = card_rect(pill, 0, 150.0, Side::Left);
        let label = label_rect(pill, 1, vec2(120.0, 30.0), Side::Left);
        let r = region(pill, &[(card, 8.0), (label, 15.0)], 2.0);
        assert_eq!(r.len(), 3);
        assert_eq!(r[1], [100, 38, 660, 338, 32]);
        assert_eq!(r[2], [100, 92, 340, 152, 60], "capsule label");
    }

    #[test]
    fn inside_covers_pill_and_panel_only() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, 100.0));
        let panel = panel_rect(pill, 200.0, Side::Left);
        assert!(inside(pos2(25.0, 50.0), pill, panel));
        assert!(inside(pos2(200.0, 150.0), pill, panel));
        assert!(!inside(pos2(46.0, 50.0), pill, panel), "the gap between");
        assert!(!inside(pos2(200.0, 300.0), pill, panel), "below the panel");
        assert!(!inside(pos2(200.0, 150.0), pill, Rect::NOTHING), "closed panel");
    }

    #[test]
    fn blob_at_maps_blob_boxes() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, pill_height(3)));
        // Blob i spans y = 8 + 1 + 10 + i * 30 .. + 24.
        assert_eq!(blob_at(pos2(25.0, 19.0), pill, 3), Some(0));
        assert_eq!(blob_at(pos2(25.0, 19.0 + 30.0 + 11.0), pill, 3), Some(1));
        assert_eq!(blob_at(pos2(25.0, 19.0 + 60.0 + 21.0), pill, 3), Some(2));
        assert_eq!(blob_at(pos2(25.0, 19.0 + 27.0), pill, 3), None, "gap");
        assert_eq!(blob_at(pos2(25.0, 12.0), pill, 3), None, "top padding");
        assert_eq!(blob_at(pos2(25.0, 19.0 + 90.0), pill, 3), None, "below the last blob");
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
    fn grow_stays_anchored_at_the_pill_edge() {
        let full = Rect::from_min_size(pos2(50.0, 8.0), vec2(300.0, 200.0));
        assert_eq!(grow(full, 0.0, Side::Right).right(), full.right());
        assert_eq!(grow(full, 0.0, Side::Left).left(), full.left());
        assert!((grow(full, 0.0, Side::Left).width() - 180.0).abs() < 1e-3);
        assert_eq!(grow(full, 1.0, Side::Right), full);
        assert_eq!(grow(full, 1.0, Side::Left), full);
    }

    #[test]
    fn beside_left_and_right() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, 100.0));
        assert_eq!(beside(pill, 300.0, Side::Left), 50.0);
        assert_eq!(beside(pill, 300.0, Side::Right) + 300.0, 0.0);
    }

    #[test]
    fn pill_x_right_hugs_window_edge() {
        assert_eq!(pill_x(Side::Left), MARGIN);
        assert_eq!(pill_x(Side::Right) + PILL_W + MARGIN, WIN_W);
    }

    #[test]
    fn panel_rect_right_side_left_of_pill() {
        let pill = Rect::from_min_size(pos2(358.0, 8.0), vec2(34.0, 100.0));
        assert_eq!(panel_rect(pill, 100.0, Side::Right).right(), pill.left() - 8.0);
    }

    #[test]
    fn pill_height_matches_mockup_box() {
        assert_eq!(pill_height(0), 0.0);
        assert_eq!(BLOB, 24.0);
        assert_eq!(pill_height(1), 2.0 + 2.0 * PAD_Y + 24.0);
        assert_eq!(pill_height(4), 2.0 + 2.0 * PAD_Y + 4.0 * BLOB + 3.0 * GAP);
    }

    fn perm(req_id: u64, sid: &str, timed_out: bool) -> crate::state::Permission {
        crate::state::Permission {
            req_id, session_id: sid.into(), tool: "Bash".into(), verb: "Wants to run a command".into(),
            detail: "ls".into(), full: "ls".into(), timed_out, gated: false,
        }
    }

    #[test]
    fn permission_beats_waiting_beats_panel() {
        let mut c = Core::new(std::sync::mpsc::channel().0);
        c.state.sessions = vec![sess("a", 1), sess("b", 2)];
        assert_eq!(current_view(&c, false), View::Pill);
        assert_eq!(current_view(&c, true), View::Panel);
        c.state.sessions[0].status = Status::Waiting(WaitKind::Question);
        assert_eq!(current_view(&c, true), View::Waiting("a".into()));
        c.state.sessions[1].status = Status::Waiting(WaitKind::Permission);
        c.state.permissions.push_back(perm(7, "b", false));
        c.state.permissions.push_back(perm(8, "b", false));
        assert_eq!(current_view(&c, true), View::Permission(7), "oldest first");
    }

    #[test]
    fn timed_out_permission_shows_waiting() {
        let mut c = Core::new(std::sync::mpsc::channel().0);
        c.state.sessions = vec![sess("a", 1)];
        c.state.sessions[0].status = Status::Waiting(WaitKind::Other);
        c.state.permissions.push_back(perm(3, "a", true));
        assert_eq!(current_view(&c, false), View::Waiting("a".into()));
        assert_eq!(waiting_text(c.state.sessions[0].status), "Waiting for you");
        assert_eq!(waiting_text(Status::Waiting(WaitKind::Question)), "Has a question for you");
    }

    #[test]
    fn of_n_counts_pending_only_when_several() {
        let mut q = std::collections::VecDeque::new();
        q.push_back(perm(1, "a", false));
        assert_eq!(of_n(&q), None);
        q.push_back(perm(2, "a", true));
        assert_eq!(of_n(&q), None, "timed out ones do not count");
        q.push_back(perm(3, "b", false));
        q.push_back(perm(4, "b", false));
        assert_eq!(of_n(&q).as_deref(), Some("1 of 3"));
    }

    #[test]
    fn finished_count_reflects_unseen() {
        let mut v = vec![sess("a", 1), sess("b", 2), sess("c", 3)];
        assert_eq!(finished_label(&v), None);
        v[1].finished = Some(Finish::Ok);
        assert_eq!(finished_label(&v), Some((1, "b".into(), "finished")));
        v[1].finished = Some(Finish::Error);
        assert_eq!(finished_label(&v), Some((1, "b".into(), "stopped with an error")));
        v[2].finished = Some(Finish::Ok);
        assert_eq!(finished_label(&v), Some((1, "2".into(), "finished")), "topmost blob, count");
        v[1].status = Status::Working;
        assert_eq!(finished_label(&v), Some((2, "c".into(), "finished")), "only idle finished blobs count");
        v[2].finished = None; // mark_seen
        assert_eq!(finished_label(&v), None);
    }

    #[test]
    fn card_sits_8px_right_top_aligned_with_blob_and_fits() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, pill_height(4)));
        // Blob 2 top: 8 + 1 + 10 + 2 * 30.
        assert_eq!(card_rect(pill, 2, 150.0, Side::Left), Rect::from_min_size(pos2(50.0, 79.0), vec2(280.0, 150.0)));
        assert_eq!(card_rect(pill, 2, 480.0, Side::Left).bottom(), 520.0 - MARGIN, "pushed up to fit the window");
    }

    #[test]
    fn label_centered_on_blob_8px_right() {
        let pill = Rect::from_min_size(pos2(8.0, 8.0), vec2(34.0, pill_height(2)));
        // Blob 1 center: 8 + 1 + 10 + 30 + 12 = 61.
        assert_eq!(label_rect(pill, 1, vec2(120.0, 28.0), Side::Left), Rect::from_min_size(pos2(50.0, 47.0), vec2(120.0, 28.0)));
    }

    #[test]
    fn button_ids_are_keyed_by_request() {
        assert_ne!(btn_id(1u64, 1), btn_id(2u64, 1), "same button, next request of the same session");
        assert_ne!(btn_id(1u64, 0), btn_id(1u64, 1));
        assert_eq!(btn_id(1u64, 1), btn_id(1u64, 1), "stable across frames");
    }

    #[test]
    fn press_before_600ms_ignored() {
        assert!(!armed(10.0, 10.0, true));
        assert!(!armed(10.0, 10.59, true));
    }

    /// Cursor positions for tests: a different point on each call means the cursor moved.
    fn at(x: f32) -> Option<Pos2> {
        Some(pos2(x, 100.0))
    }

    #[test]
    fn press_after_600ms_with_entry_counts() {
        let mut a = Arming::new(1, 10.0, false, at(0.0));
        a.update(true, at(60.0));
        assert!(a.entered);
        assert!(armed(a.appeared, 10.6, a.entered));
        assert!(!armed(a.appeared, 10.6, false), "no entry, no click");
    }

    #[test]
    fn cursor_already_over_card_needs_leave_and_reenter() {
        let mut a = Arming::new(1, 10.0, true, at(60.0));
        a.update(true, at(61.0));
        assert!(!a.entered, "still where the card appeared, even if it wiggles");
        assert!(!armed(a.appeared, 12.0, a.entered));
        a.update(false, at(0.0));
        assert!(!a.entered, "left, not yet back");
        a.update(true, at(60.0));
        assert!(a.entered);
        a.update(false, at(0.0));
        assert!(a.entered, "stays armed once entered");
        assert!(armed(a.appeared, 12.0, a.entered));
    }

    #[test]
    fn card_moving_under_still_cursor_does_not_arm() {
        let mut a = Arming::new(1, 10.0, false, at(60.0));
        a.update(false, at(60.0));
        a.update(true, at(60.0)); // the card slid up under the resting cursor
        assert!(!a.entered);
        a.update(true, at(62.0));
        assert!(!a.entered, "moving inside after the slide is not an entry");
        a.update(false, at(0.0));
        a.update(true, at(60.0));
        assert!(a.entered, "a real leave and re-enter arms");
        a.update(true, at(60.0));
        assert!(a.entered, "resting on the button after entering stays armed");
    }

    #[test]
    fn release_late_but_press_early_ignored() {
        let mut a = Arming::new(1, 10.0, false, at(0.0));
        a.update(true, at(60.0));
        // Pressed on the frame at 10.2, still held at 10.5, released on the frame at 11.0.
        let p = press_time(f64::NEG_INFINITY, true, Some(10.2), 10.2);
        let p = press_time(p, false, Some(10.2), 10.5);
        let p = press_time(p, false, None, 11.0);
        assert_eq!(p, 10.2);
        assert!(!armed(a.appeared, p, a.entered));
        // Press and release inside one frame: the press is this frame.
        assert_eq!(press_time(10.2, true, None, 11.0), 11.0);
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
