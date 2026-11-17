mod blob;
mod theme;
mod views;

use blob::BlobState;
use views::View;

use crate::server::Msg;
use crate::settings::{self, Side};
use crate::{event, hook::Reply, procs, state::State};
use eframe::egui::{self, Pos2, ViewportBuilder, ViewportCommand};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::HWND;

pub struct Core {
    pub state: State,
    pub replies: HashMap<u64, Sender<Reply>>,
    pub phone: crate::phone::Phone,
    empty_since: Option<Instant>,
    last_prune: Option<Instant>,
    tx: Sender<Msg>,
}

const PRUNE_EVERY: Duration = Duration::from_secs(3);
const EXIT_AFTER_EMPTY: Duration = Duration::from_secs(3);

impl Core {
    pub fn new(tx: Sender<Msg>) -> Core {
        Core { state: State::default(), replies: HashMap::new(), phone: crate::phone::Phone::new(tx.clone()), empty_since: None, last_prune: None, tx }
    }

    pub fn handle(&mut self, msg: Msg, now: u64) {
        match msg {
            Msg::Event { raw, chain, reply, token } => {
                if let Some(t) = token {
                    let empty = t.is_empty();
                    self.phone.set_token(t);
                    if empty || self.phone.chat.load(Ordering::Relaxed) == 0 {
                        self.set_away(false);
                    }
                }
                let Some(ev) = event::parse(&raw) else { return };
                if ev.name == "SessionEnd" {
                    self.phone.forget(&ev.session_id);
                }
                let known = self.state.sessions.iter().any(|s| s.id == ev.session_id && !s.transcript.is_empty());
                if !known && !ev.transcript_path.is_empty() && ev.name != "SessionEnd" {
                    // Rebuild todos on the worker thread; transcripts can be tens of MB.
                    // ponytail: the UI sees the result on its next frame, up to 3 s when idle; no wake here.
                    // ponytail: a replay landing after live todo events overwrites them with the transcript's
                    // state, which already contains them, unless the transcript write lags the hook.
                    let (tx, path, session_id) = (self.tx.clone(), ev.transcript_path.clone(), ev.session_id.clone());
                    std::thread::spawn(move || {
                        // A live transcript can end inside a multi-byte character.
                        if let Ok(bytes) = std::fs::read(path) {
                            let text = String::from_utf8_lossy(&bytes);
                            let _ = tx.send(Msg::Replay { session_id, todos: event::replay(&text) });
                        }
                    });
                }
                // apply drops the session on SessionEnd, so take it first.
                let ended = (ev.name == "SessionEnd").then(|| self.state.sessions.iter().find(|s| s.id == ev.session_id).cloned()).flatten();
                let id = self.state.apply(&ev, procs::find_pids(&chain), now);
                let s = ended.or_else(|| self.state.sessions.iter().find(|s| s.id == ev.session_id).cloned());
                if let Some(s) = s {
                    let card = id.and_then(|id| self.state.permissions.iter().find(|p| p.req_id == id));
                    self.phone.on_event(&raw, &s, card);
                }
                // Dropping the Sender of a card the turn ended releases its waiting hook.
                let perms = &self.state.permissions;
                self.replies.retain(|id, _| perms.iter().any(|p| p.req_id == *id));
                // A held question (ask with a reply) has no card yet (Task 4): `id` is None, the senders drop,
                // and the server writes skip so the terminal dialog opens.
                if let (Some(id), Some((reply, req))) = (id, reply) {
                    self.replies.insert(id, reply);
                    let _ = req.send(id);
                }
            }
            Msg::Listen { session_id, cwd, chain, pipe } => {
                let pids = procs::find_pids(&chain);
                if let Some(s) = self.state.sessions.iter_mut().find(|s| s.id == session_id) {
                    s.claude_pid = pids.claude.or(s.claude_pid);
                    s.terminal_pid = pids.terminal.or(s.terminal_pid);
                } else {
                    let ev = event::HookEvent { name: "SessionStart".into(), session_id: session_id.clone(), cwd, ..Default::default() };
                    self.state.apply(&ev, pids, now);
                }
                // Test hook, debug builds only: AGENT_ISLANDS_ECHO=<text> is sent to the first listener 2 s after it registers.
                #[cfg(debug_assertions)]
                if let Ok(text) = std::env::var("AGENT_ISLANDS_ECHO")
                    && !ECHOED.swap(true, Ordering::Relaxed)
                    && let Ok(mut f) = pipe.try_clone()
                {
                    std::thread::spawn(move || {
                        use std::io::Write;
                        std::thread::sleep(Duration::from_secs(2));
                        let _ = writeln!(f, "{}", serde_json::to_string(&text).unwrap_or_default());
                    });
                }
                self.phone.listen(session_id, pipe);
            }
            Msg::Timeout(id) => {
                self.phone.timed_out.insert(id);
                self.replies.remove(&id);
                self.state.timeout(id);
            }
            Msg::Replay { session_id, todos } => self.state.set_todos(&session_id, todos),
            Msg::Sent { message_id, session_id } => {
                // An ended session's last message ("Ended") is not routable: a reply would show its uuid.
                if self.state.sessions.iter().any(|s| s.id == session_id) {
                    self.phone.sent.insert(message_id, session_id);
                }
            }
            Msg::Phone(m) => {
                if let crate::phone::Action::Answer(id, r, callback_id) = self.phone.incoming(m, &self.state.sessions, Instant::now()) {
                    let text = if self.replies.contains_key(&id) {
                        self.answer(id, r);
                        ""
                    } else if self.phone.timed_out.contains(&id) {
                        "Too late, answered in the terminal"
                    } else {
                        "Already answered"
                    };
                    self.phone.ack(callback_id, text);
                }
            }
        }
    }

    /// Away can be on only while paired. Turning it off releases everything waiting.
    pub fn set_away(&mut self, on: bool) {
        if on && self.phone.chat.load(Ordering::Relaxed) == 0 {
            return;
        }
        let was = self.phone.away.swap(on, Ordering::Relaxed);
        if let Some(p) = &self.phone.settings {
            let _ = settings::set(p, "away", on.into());
        }
        if was && !on {
            // Cards get a fresh 30 s island wait.
            for tx in self.replies.values() {
                let _ = tx.send(Reply::Rearm);
            }
            // Task 4: also remove every waiting question sender here, so the terminal dialog opens.
        }
    }

    pub fn disconnect(&mut self) {
        self.phone.disconnect();
        self.set_away(false);
    }

    pub fn answer(&mut self, req_id: u64, r: Reply) {
        if let Some(tx) = self.replies.remove(&req_id) {
            let _ = tx.send(r);
        }
        self.state.resolve(req_id);
    }

    /// True when the process should exit. `now_ms` is wall-clock ms, as passed to `handle`.
    pub fn tick(&mut self, now: Instant, now_ms: u64) -> bool {
        if self.last_prune.is_none_or(|t| now.duration_since(t) >= PRUNE_EVERY) {
            self.last_prune = Some(now);
            self.state.prune(procs::is_alive, now_ms);
        }
        if !self.state.is_empty() {
            self.empty_since = None;
            return false;
        }
        now.duration_since(*self.empty_since.get_or_insert(now)) >= EXIT_AFTER_EMPTY
    }
}

/// AGENT_ISLANDS_CLICK value to the reply it simulates; only the exact values `allow` and `deny`.
#[cfg(debug_assertions)]
fn parse_click(v: Option<&str>) -> Option<Reply> {
    match v? {
        "allow" => Some(Reply::Allow),
        "deny" => Some(Reply::Deny),
        _ => None,
    }
}

#[cfg(debug_assertions)]
static ECHOED: AtomicBool = AtomicBool::new(false);

static CTX: OnceLock<egui::Context> = OnceLock::new();

/// Repaints the UI from any thread; no-op before the window exists.
pub(crate) fn wake() {
    if let Some(c) = CTX.get() {
        c.request_repaint();
    }
}

/// Runs the island until every session is gone. Returns at once if another island owns the pipe.
pub fn run() {
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = || {
        if let Some(c) = CTX.get() {
            c.request_repaint();
        }
    };
    let away = Arc::new(AtomicBool::new(false));
    if crate::server::spawn(tx.clone(), wake, away.clone()).is_err() {
        return;
    }
    let viewport = ViewportBuilder::default()
        .with_title("Agent Islands")
        .with_inner_size([views::WIN_W, views::WIN_H])
        .with_resizable(false)
        .with_transparent(true)
        .with_decorations(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_active(false);
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, ..Default::default() };
    let _ = eframe::run_native(
        "agent-islands",
        options,
        Box::new(move |cc| {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let RawWindowHandle::Win32(h) = cc.window_handle()?.as_raw() else { return Err("not win32".into()) };
            let hwnd = HWND(h.hwnd.get() as *mut _);
            win::sync_style(hwnd);
            win::no_frame(hwnd);
            let side = settings::path().map(|p| settings::load_from(&p)).unwrap_or_default();
            win::place(hwnd, side);
            let mut core = Core::new(tx.clone());
            core.phone.chat.store(settings::path().and_then(|p| settings::load_chat(&p)).unwrap_or(0), Ordering::Relaxed);
            core.phone.away = away.clone();
            // Away survives a restart only while still paired.
            away.store(core.phone.chat.load(Ordering::Relaxed) != 0 && settings::path().is_some_and(|p| settings::load_away(&p)), Ordering::Relaxed);
            core.phone.settings = settings::path();
            theme::setup_fonts(&cc.egui_ctx);
            let _ = CTX.set(cc.egui_ctx.clone());
            // Test hook, debug builds only: AGENT_ISLANDS_OPEN=<session id> opens the panel on that session once it exists.
            #[cfg(debug_assertions)]
            let open_req = std::env::var("AGENT_ISLANDS_OPEN").ok();
            #[cfg(not(debug_assertions))]
            let open_req = None;
            let app = App {
                core, rx, hwnd, region: Vec::new(), trimmed: false, selected: None, open_req, was_down: false, side, settings_open: false, arming: None, press_at: f64::NEG_INFINITY,
                // Test hook, debug builds only: AGENT_ISLANDS_CLICK=allow|deny answers the first permission card 1 s after it shows.
                #[cfg(debug_assertions)]
                auto_click: parse_click(std::env::var("AGENT_ISLANDS_CLICK").ok().as_deref()),
            };
            Ok(Box::new(app))
        }),
    );
}

/// Off the UI thread: UI Automation can be slow, and the island must not freeze.
fn go_to_terminal(s: &crate::state::Session) {
    if let Some(pid) = s.terminal_pid {
        let transcript = std::path::PathBuf::from(&s.transcript);
        std::thread::spawn(move || crate::focus::go_to_terminal(pid, crate::focus::ai_title(&transcript).as_deref()));
    }
}

struct App {
    core: Core,
    rx: Receiver<Msg>,
    hwnd: HWND,
    /// Window region last set (see views::region).
    region: Vec<[i32; 5]>,
    trimmed: bool,
    selected: Option<String>,
    open_req: Option<String>,
    was_down: bool,
    side: Side,
    settings_open: bool,
    /// Pointer history for the permission card on screen.
    arming: Option<views::Arming>,
    /// When the latest mouse press started (egui time).
    press_at: f64,
    #[cfg(debug_assertions)]
    auto_click: Option<Reply>,
}

impl App {
    /// Opens the panel on a finished session and marks it seen.
    fn open_finished(&mut self, id: String) {
        self.core.state.mark_seen(&id);
        self.selected = Some(id);
    }
}

impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        while let Ok(msg) = self.rx.try_recv() {
            self.core.handle(msg, now_ms);
        }
        let ctx = ui.ctx().clone();
        if self.core.tick(Instant::now(), now_ms) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        let t = ui.input(|i| i.time);
        if !self.trimmed && t > 2.0 {
            // The GL driver touches ~70 MB while starting up and never again; hand it back once.
            self.trimmed = true;
            win::trim();
        }
        let sessions = &self.core.state.sessions;
        if let Some(id) = self.open_req.take_if(|id| sessions.iter().any(|s| &s.id == id)) {
            self.selected = Some(id);
        }
        let phone = self.core.phone.status(Instant::now());
        let view = views::current_view(&self.core, self.selected.is_some());
        // The bar marks the session a card is about (island-left.html state 3), else the selection.
        let marked = match &view {
            View::Permission(id) => self.core.state.permissions.iter().find(|p| p.req_id == *id).map(|p| p.session_id.clone()),
            View::Waiting(id) => Some(id.clone()),
            _ => self.selected.clone(),
        };
        let pill = views::pill(ui, &self.core, marked.as_deref(), self.side, self.core.phone.away.load(Ordering::Relaxed), t);
        if pill.secondary_clicked() {
            self.settings_open = !self.settings_open;
        }
        if pill.clicked() {
            self.settings_open = false;
            let sessions = &self.core.state.sessions;
            let clicked = pill.interact_pointer_pos().and_then(|p| views::blob_at(p, pill.rect, sessions.len())).map(|i| &sessions[i]);
            if let Some(s) = clicked.filter(|s| BlobState::of(s.status, s.finished) == BlobState::Done) {
                let id = s.id.clone();
                self.open_finished(id);
            } else {
                self.selected = views::toggle(self.selected.as_deref(), clicked.map(|s| s.id.as_str()), views::default_selection(sessions));
            }
        }
        let r = pill.rect;
        let cursor = win::cursor(self.hwnd, ctx.pixels_per_point());
        let (pressed, start) = ui.input(|i| (i.pointer.any_pressed(), i.pointer.press_start_time()));
        self.press_at = views::press_time(self.press_at, pressed, start, t);
        let (panel, radius) = match view {
            View::Permission(id) => {
                let shown = self.arming.filter(|a| a.req == id).is_some_and(|a| views::armed(a.appeared, t, a.entered));
                let (rect, mut reply) = views::permission_card(ui, &self.core, id, shown, r, self.side, t);
                let over = cursor.is_some_and(|p| rect.contains(p));
                let a = self.arming.get_or_insert(views::Arming::new(id, t, over, cursor));
                if a.req != id {
                    *a = views::Arming::new(id, t, over, cursor);
                }
                a.update(over, cursor);
                let a = *a;
                if !views::armed(a.appeared, self.press_at, a.entered) {
                    reply = None;
                }
                // The test hook bypasses the pointer rules; it does not exist in release builds.
                #[cfg(debug_assertions)]
                if t - a.appeared >= 1.0 && reply.is_none() {
                    reply = self.auto_click.take();
                }
                if let Some(reply) = reply {
                    self.core.answer(id, reply);
                }
                (rect, 8.0)
            }
            View::Waiting(id) => {
                let (rect, go) = views::waiting_card(ui, &self.core, &id, r, self.side, t);
                let s = self.core.state.sessions.iter().find(|s| s.id == id);
                if go && let Some(s) = s {
                    go_to_terminal(s);
                }
                (rect, 8.0)
            }
            View::Panel | View::Pill if self.settings_open => {
                let (rect, clicked, away, disconnect) = views::settings_card(ui, r, self.side, self.core.phone.away.load(Ordering::Relaxed), &phone);
                if let Some(v) = away {
                    self.core.set_away(v);
                }
                if disconnect {
                    self.core.disconnect();
                }
                if let Some(side) = clicked {
                    if let Some(p) = settings::path() {
                        let _ = settings::save_to(&p, side);
                    }
                    self.side = side;
                    win::place(self.hwnd, side);
                }
                (rect, 10.0)
            }
            View::Panel | View::Pill => {
                let (panel, go) = views::panel(ui, &self.core, &phone, &mut self.selected, r, self.side, t);
                if go && let Some(s) = self.selected.as_deref().and_then(|id| self.core.state.sessions.iter().find(|s| s.id == id)) {
                    go_to_terminal(s);
                }
                if panel.is_positive() {
                    (panel, 8.0)
                } else {
                    let (rect, open) = views::finished_pill(ui, &self.core, r, self.side);
                    if let Some(id) = open {
                        self.open_finished(id);
                    }
                    (rect, rect.height() / 2.0)
                }
            }
        };
        // The window region is the drawn shapes: Windows sends clicks anywhere else to the window below.
        // Hover tooltips (the full command on a permission card) float outside the card.
        let mut shapes = vec![(panel, radius)];
        ctx.memory(|m| {
            let tips = m.areas().visible_layer_ids().into_iter().filter(|l| l.order == egui::Order::Tooltip);
            shapes.extend(tips.filter_map(|l| m.area_rect(l.id).map(|r| (r, 4.0))));
        });
        let region = views::region(r, &shapes, ctx.pixels_per_point());
        if region != self.region {
            win::set_region(self.hwnd, &region);
            self.region = region;
        }
        win::sync_style(self.hwnd);
        let over = cursor.is_some_and(|p| views::inside(p, r, panel));
        let down = win::left_down();
        if (self.selected.is_some() || self.settings_open) && views::outside_click(self.was_down, down, over) {
            self.selected = None;
            self.settings_open = false;
        }
        self.was_down = down;
        let moving = panel.is_positive() || self.core.state.sessions.iter().any(|s| matches!(s.status, crate::state::Status::Working | crate::state::Status::Waiting(_)));
        // Nothing moving: input and pipe messages wake the UI; one frame per prune interval keeps
        // pruning and the exit-when-empty timer going.
        ctx.request_repaint_after(if moving { Duration::from_millis(33) } else { PRUNE_EVERY });
    }
}

mod win {
    use super::{Pos2, Side, views::{MARGIN, WIN_W}};
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::*;

    /// No Windows 11 border or rounded corners around the transparent window.
    pub fn no_frame(hwnd: HWND) {
        use windows::Win32::Graphics::Dwm::*;
        let (color, corner) = (DWMWA_COLOR_NONE, DWMWCP_DONOTROUND);
        unsafe {
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &color as *const _ as *const _, 4);
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, 4);
        }
    }

    /// Window position so the pill sits 8 logical px from the work area's `side` edge, 96 px down.
    pub fn place(hwnd: HWND, side: Side) {
        let mut work = RECT::default();
        let (mut dpi, mut dpi_y) = (96u32, 96u32);
        unsafe {
            let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut work as *mut RECT as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
            let _ = GetDpiForMonitor(MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY), MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
            let k = dpi as f32 / 96.0;
            let x = match side {
                Side::Left => work.left as f32 + (8.0 - MARGIN) * k,
                Side::Right => work.right as f32 - 8.0 * k - (WIN_W - MARGIN) * k,
            };
            let (x, y) = (x.round() as i32, work.top + ((96.0 - MARGIN) * k).round() as i32);
            let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    /// Left button down now, or pressed since the last call (so short clicks between frames count).
    pub fn left_down() -> bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
        unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8001 != 0 }
    }

    pub fn trim() {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        let _ = unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX) };
    }

    /// Cursor in window-local logical px.
    pub fn cursor(hwnd: HWND, ppp: f32) -> Option<Pos2> {
        let (mut p, mut r) = (POINT::default(), RECT::default());
        unsafe { GetCursorPos(&mut p).ok()?; GetWindowRect(hwnd, &mut r).ok()? };
        Some(Pos2::new((p.x - r.left) as f32 / ppp, (p.y - r.top) as f32 / ppp))
    }

    /// Never activate, no taskbar or Alt+Tab entry. Never click-through: the window region decides where
    /// clicks land. Not layered: winit's DWM transparency already blends per pixel.
    /// winit rewrites GWL_EXSTYLE whenever its own flags change (e.g. on first show), so sync_style
    /// runs every frame.
    pub fn ex_style(cur: isize) -> isize {
        (cur | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW).0 as isize) & !((WS_EX_APPWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT).0 as isize)
    }

    /// A plain popup. winit keeps WS_CAPTION and friends on undecorated windows, so DWM drew a drop
    /// shadow around the whole window rect, a caption edge and a close button ("x") over the screen.
    pub fn style(cur: isize) -> isize {
        let frame = WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
        (cur | WS_POPUP.0 as isize) & !(frame.0 as isize)
    }

    pub fn sync_style(hwnd: HWND) {
        unsafe {
            let mut changed = false;
            for (index, want) in [(GWL_EXSTYLE, ex_style as fn(isize) -> isize), (GWL_STYLE, style)] {
                let cur = GetWindowLongPtrW(hwnd, index);
                if want(cur) != cur {
                    SetWindowLongPtrW(hwnd, index, want(cur));
                    changed = true;
                }
            }
            if changed {
                let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
    }

    /// Clips the window to the union of `shapes` (views::region); outside it the window neither draws nor
    /// takes clicks.
    pub fn set_region(hwnd: HWND, shapes: &[[i32; 5]]) {
        use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, RGN_OR, SetWindowRgn};
        unsafe {
            let rgn = CreateRectRgn(0, 0, 0, 0);
            for &[l, t, r, b, d] in shapes {
                // The right and bottom edges of a round-rect region are exclusive.
                let part = CreateRoundRectRgn(l, t, r + 1, b + 1, d, d);
                CombineRgn(Some(rgn), Some(rgn), Some(part), RGN_OR);
                let _ = DeleteObject(part.into());
            }
            // The system owns the region from here on.
            SetWindowRgn(hwnd, Some(rgn), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc::channel;

    fn pipe_pair(tag: &str) -> (std::fs::File, std::fs::File) {
        let name = format!(r"\\.\pipe\ai-test-core-{tag}-{}", std::process::id());
        let mut server = crate::pipe::Server::create_first_named(&name).unwrap();
        let t = std::thread::spawn(move || server.accept().unwrap());
        let (cli, _) = crate::pipe::connect_to(&name).unwrap();
        (t.join().unwrap(), cli)
    }

    #[test]
    fn listen_for_unknown_session_creates_it() {
        let mut c = Core::new(channel().0);
        let (srv, _cli) = pipe_pair("lu");
        c.handle(Msg::Listen { session_id: "s".into(), cwd: r"D:\p\x".into(), chain: vec![], pipe: srv }, 1);
        assert_eq!(c.state.sessions.len(), 1);
        assert_eq!(c.state.sessions[0].project, "X");
    }

    #[test]
    fn deliver_encodes_multiline_as_one_json_line() {
        let mut c = Core::new(channel().0);
        let (srv, cli) = pipe_pair("ml");
        c.handle(Msg::Listen { session_id: "s".into(), cwd: "x".into(), chain: vec![], pipe: srv }, 1);
        let text = "line 1\nsay \"hi\" \\ 🟠";
        assert!(c.phone.deliver("s", text));
        let mut line = String::new();
        BufReader::new(&cli).read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<String>(line.trim_end()).unwrap(), text);
    }

    #[test]
    fn deliver_to_closed_listener_fails_and_forgets() {
        let mut c = Core::new(channel().0);
        let (srv, cli) = pipe_pair("cl");
        c.handle(Msg::Listen { session_id: "s".into(), cwd: "x".into(), chain: vec![], pipe: srv }, 1);
        drop(cli);
        assert!(!c.phone.deliver("s", "a"));
        assert!(!c.phone.deliver("s", "a"));
    }

    #[test]
    fn replay_runs_once_after_listener_created_session() {
        let (tx, rx) = channel();
        let mut c = Core::new(tx);
        let (srv, _cli) = pipe_pair("rp");
        c.handle(Msg::Listen { session_id: "s".into(), cwd: "x".into(), chain: vec![], pipe: srv }, 1);
        let mut r = raw("SessionStart", "s");
        r["transcript_path"] = json!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/transcript.jsonl"));
        c.handle(Msg::Event { raw: r.clone(), chain: vec![], reply: None, token: None }, 2);
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)), Ok(Msg::Replay { .. })));
        c.handle(Msg::Event { raw: r, chain: vec![], reply: None, token: None }, 3);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "second replay spawned");
    }

    #[test]
    fn session_end_forgets_listener() {
        let mut c = Core::new(channel().0);
        let (srv, _cli) = pipe_pair("se");
        c.handle(Msg::Listen { session_id: "s".into(), cwd: "x".into(), chain: vec![], pipe: srv }, 1);
        c.handle(Msg::Event { raw: raw("SessionEnd", "s"), chain: vec![], reply: None, token: None }, 2);
        assert!(!c.phone.deliver("s", "a"));
    }

    fn raw(name: &str, sid: &str) -> Value {
        json!({"hook_event_name": name, "session_id": sid, "cwd": r"D:\p\x", "transcript_path": "t.jsonl", "tool_name": "Bash", "tool_input": {"command": "ls"}})
    }

    #[test]
    fn first_event_replays_transcript_once() {
        let (tx, rx) = channel();
        let mut c = Core::new(tx);
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/transcript.jsonl");
        let mut r = raw("SessionStart", "s");
        r["transcript_path"] = json!(path);
        c.handle(Msg::Event { raw: r.clone(), chain: vec![], reply: None, token: None }, 1);
        c.handle(Msg::Event { raw: r, chain: vec![], reply: None, token: None }, 2);
        let Ok(Msg::Replay { session_id, todos }) = rx.recv_timeout(Duration::from_secs(2)) else { panic!("no replay") };
        assert_eq!((session_id.as_str(), todos.len()), ("s", 2));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "second replay spawned");
        c.handle(Msg::Replay { session_id: "s".into(), todos }, 3);
        assert_eq!(c.state.sessions[0].todos.len(), 2);
    }

    #[test]
    fn replay_survives_trailing_invalid_utf8() {
        let (tx, rx) = channel();
        let mut c = Core::new(tx);
        let mut bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/transcript.jsonl")).unwrap();
        bytes.extend_from_slice(b"\xE2\x82");
        let path = std::env::temp_dir().join(format!("ai-utf8-{}.jsonl", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let mut r = raw("SessionStart", "s");
        r["transcript_path"] = json!(path.to_str().unwrap());
        c.handle(Msg::Event { raw: r, chain: vec![], reply: None, token: None }, 1);
        let got = rx.recv_timeout(Duration::from_secs(2));
        let _ = std::fs::remove_file(&path);
        let Ok(Msg::Replay { todos, .. }) = got else { panic!("no replay") };
        assert_eq!(todos.len(), 2);
    }

    #[test]
    fn session_end_for_unknown_session_spawns_no_replay() {
        let (tx, rx) = channel();
        let mut c = Core::new(tx);
        let mut r = raw("SessionEnd", "s");
        r["transcript_path"] = json!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/transcript.jsonl"));
        c.handle(Msg::Event { raw: r, chain: vec![], reply: None, token: None }, 1);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "replay spawned for SessionEnd");
    }

    fn event(core: &mut Core, name: &str, sid: &str, wants: bool) -> Option<(std::sync::mpsc::Receiver<Reply>, u64)> {
        let (rtx, rrx) = channel();
        let (qtx, qrx) = channel();
        let reply = wants.then_some((rtx, qtx));
        core.handle(Msg::Event { raw: raw(name, sid), chain: vec![], reply, token: None }, 1);
        wants.then(|| (rrx, qrx.try_recv().unwrap()))
    }

    #[test]
    fn answer_sends_reply_and_resolves() {
        let mut c = Core::new(channel().0);
        let (rx, id) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        assert_eq!(c.state.permissions.len(), 1);
        c.answer(id, Reply::Allow);
        assert!(matches!(rx.try_recv(), Ok(Reply::Allow)));
        assert!(c.state.permissions.is_empty());
        assert!(c.replies.is_empty());
    }

    #[test]
    fn answer_twice_sends_once() {
        let mut c = Core::new(channel().0);
        let (rx, id) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        c.answer(id, Reply::Deny);
        c.answer(id, Reply::Allow);
        assert!(matches!(rx.try_recv(), Ok(Reply::Deny)));
        assert!(rx.try_recv().is_err(), "second answer must not send");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn click_hook_accepts_only_exact_values() {
        assert!(matches!(parse_click(Some("allow")), Some(Reply::Allow)));
        assert!(matches!(parse_click(Some("deny")), Some(Reply::Deny)));
        for v in ["1", "yes", "Allow", "DENY", " allow", "allow ", ""] {
            assert!(parse_click(Some(v)).is_none(), "{v:?} must not auto-click");
        }
        assert!(parse_click(None).is_none());
    }

    #[test]
    fn stop_releases_waiting_hook() {
        let mut c = Core::new(channel().0);
        let (rx, _) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        let (other, _) = event(&mut c, "PermissionRequest", "t", true).unwrap();
        event(&mut c, "Stop", "s", false);
        assert!(matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
        assert!(matches!(other.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
        assert_eq!(c.replies.len(), 1);
    }

    #[test]
    fn timeout_msg_marks_timed_out() {
        let mut c = Core::new(channel().0);
        let (_rx, id) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        c.handle(Msg::Timeout(id), 2);
        assert!(c.state.permissions[0].timed_out);
        assert!(c.replies.is_empty());
    }

    fn paired_core() -> Core {
        let c = Core::new(channel().0);
        c.phone.chat.store(7, Ordering::Relaxed);
        c
    }

    #[test]
    fn set_away_on_needs_pairing() {
        let mut c = Core::new(channel().0);
        c.set_away(true);
        assert!(!c.phone.away.load(Ordering::Relaxed));
        c.phone.chat.store(7, Ordering::Relaxed);
        c.set_away(true);
        assert!(c.phone.away.load(Ordering::Relaxed));
    }

    #[test]
    fn away_off_rearms_cards() {
        let mut c = paired_core();
        let (rx, _) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        c.set_away(true);
        assert!(rx.try_recv().is_err());
        c.set_away(false);
        assert!(matches!(rx.try_recv(), Ok(Reply::Rearm)));
        assert!(!c.state.permissions.is_empty(), "the card stays");
    }

    #[test]
    fn disconnect_turns_away_off_and_releases() {
        let mut c = paired_core();
        let (rx, _) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        c.set_away(true);
        c.disconnect();
        assert!(!c.phone.away.load(Ordering::Relaxed));
        assert_eq!(c.phone.chat.load(Ordering::Relaxed), 0);
        assert!(matches!(rx.try_recv(), Ok(Reply::Rearm)));
    }

    #[test]
    fn token_cleared_turns_away_off() {
        let mut c = paired_core();
        let (rx, _) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        c.set_away(true);
        c.handle(Msg::Event { raw: raw("Stop", "t"), chain: vec![], reply: None, token: Some(String::new()) }, 2);
        assert!(!c.phone.away.load(Ordering::Relaxed));
        assert!(matches!(rx.try_recv(), Ok(Reply::Rearm)));
    }

    #[test]
    fn stale_button_taps() {
        use crate::telegram::{Incoming, Out};
        let (otx, orx) = channel();
        let mut c = Core::new(channel().0);
        c.phone = crate::phone::Phone::with_out(otx);
        c.phone.chat.store(7, Ordering::Relaxed);
        let (rx, id) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        let (_rx2, late) = event(&mut c, "PermissionRequest", "t", true).unwrap();
        c.handle(Msg::Timeout(late), 2);
        while orx.try_recv().is_ok() {}
        let tap = |c: &mut Core, data: String| {
            c.handle(Msg::Phone(Incoming::Button { chat: 7, callback_id: "c".into(), data, date: i64::MAX }), 3);
            let Ok(Out::Answer { callback_id, text }) = orx.try_recv() else { panic!("no answer") };
            assert_eq!(callback_id, "c");
            assert!(orx.try_recv().is_err(), "exactly one answer");
            text
        };
        assert_eq!(tap(&mut c, format!("a:{id}")), "");
        assert!(matches!(rx.try_recv(), Ok(Reply::Allow)));
        assert_eq!(tap(&mut c, format!("a:{id}")), "Already answered");
        assert_eq!(tap(&mut c, format!("a:{late}")), "Too late, answered in the terminal");
        assert_eq!(tap(&mut c, "a:999".into()), "Already answered");
    }

    #[test]
    fn stale_dated_tap_never_answers_a_live_card() {
        use crate::telegram::{Incoming, Out};
        let (otx, orx) = channel();
        let mut c = Core::new(channel().0);
        c.phone = crate::phone::Phone::with_out(otx);
        c.phone.chat.store(7, Ordering::Relaxed);
        let (rx, id) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        while orx.try_recv().is_ok() {}
        // A card of an earlier island run: its date is before this run started, whatever its req id.
        c.handle(Msg::Phone(Incoming::Button { chat: 7, callback_id: "c".into(), data: format!("a:{id}"), date: 1 }), 3);
        assert!(rx.try_recv().is_err(), "live card must stay pending");
        assert!(c.replies.contains_key(&id));
        assert!(matches!(orx.try_recv(), Ok(Out::Answer { callback_id, text }) if callback_id == "c" && text == "Too late, answered in the terminal"));
    }

    #[test]
    fn sent_for_ended_session_is_not_routable() {
        let mut c = Core::new(channel().0);
        c.handle(Msg::Sent { message_id: 5, session_id: "gone".into() }, 1);
        assert!(c.phone.sent.is_empty());
        event(&mut c, "PreToolUse", "s", false);
        c.handle(Msg::Sent { message_id: 6, session_id: "s".into() }, 1);
        assert_eq!(c.phone.sent.get(&6).map(String::as_str), Some("s"));
    }

    #[test]
    fn ex_style_never_activates_and_is_not_layered_or_click_through() {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let has = |s: isize, f: WINDOW_EX_STYLE| s & f.0 as isize != 0;
        let s = win::ex_style((WS_EX_APPWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT).0 as isize);
        assert!(has(s, WS_EX_NOACTIVATE) && has(s, WS_EX_TOOLWINDOW) && has(s, WS_EX_TOPMOST));
        assert!(!has(s, WS_EX_APPWINDOW) && !has(s, WS_EX_LAYERED) && !has(s, WS_EX_TRANSPARENT));
    }

    #[test]
    fn style_has_no_frame_or_caption() {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let s = win::style((WS_VISIBLE | WS_CLIPSIBLINGS | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX).0 as isize);
        assert_eq!(s, (WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS).0 as isize);
    }

    #[test]
    fn tick_exits_after_3s_empty_not_before() {
        let mut c = Core::new(channel().0);
        let t = Instant::now();
        assert!(!c.tick(t, 0));
        assert!(!c.tick(t + Duration::from_millis(2900), 0));
        assert!(c.tick(t + Duration::from_secs(3), 0));
    }

    #[test]
    fn tick_does_not_exit_while_session_exists() {
        let mut c = Core::new(channel().0);
        event(&mut c, "SessionStart", "s", false);
        let t = Instant::now();
        assert!(!c.tick(t, 0));
        assert!(!c.tick(t + Duration::from_secs(10), 0));
    }
}
