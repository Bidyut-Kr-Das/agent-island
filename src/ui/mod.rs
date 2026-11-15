mod blob;
mod theme;
mod views;

use blob::BlobState;
use views::View;

use crate::server::Msg;
use crate::{event, hook::Reply, procs, state::State};
use eframe::egui::{self, Pos2, ViewportBuilder, ViewportCommand};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::HWND;

pub struct Core {
    pub state: State,
    pub replies: HashMap<u64, Sender<Reply>>,
    empty_since: Option<Instant>,
    last_prune: Option<Instant>,
}

const PRUNE_EVERY: Duration = Duration::from_secs(3);
const EXIT_AFTER_EMPTY: Duration = Duration::from_secs(3);

impl Core {
    pub fn new() -> Core {
        Core { state: State::default(), replies: HashMap::new(), empty_since: None, last_prune: None }
    }

    pub fn handle(&mut self, msg: Msg, now: u64) {
        match msg {
            Msg::Event { raw, chain, reply } => {
                let Some(ev) = event::parse(&raw) else { return };
                let id = self.state.apply(&ev, procs::find_pids(&chain), now);
                if let (Some(id), Some((reply, req))) = (id, reply) {
                    self.replies.insert(id, reply);
                    let _ = req.send(id);
                }
            }
            Msg::Timeout(id) => {
                self.replies.remove(&id);
                self.state.timeout(id);
            }
        }
    }

    pub fn answer(&mut self, req_id: u64, r: Reply) {
        if let Some(tx) = self.replies.remove(&req_id) {
            let _ = tx.send(r);
        }
        self.state.resolve(req_id);
    }

    /// True when the process should exit.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.last_prune.is_none_or(|t| now.duration_since(t) >= PRUNE_EVERY) {
            self.last_prune = Some(now);
            self.state.prune(procs::is_alive);
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

static CTX: OnceLock<egui::Context> = OnceLock::new();

/// Runs the island until every session is gone. Returns at once if another island owns the pipe.
pub fn run() {
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = || {
        if let Some(c) = CTX.get() {
            c.request_repaint();
        }
    };
    if crate::server::spawn(tx, wake).is_err() {
        return;
    }
    let viewport = ViewportBuilder::default()
        .with_title("Agent Islands")
        .with_inner_size([400.0, 520.0])
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
            win::sync_style(hwnd, true);
            win::no_frame(hwnd);
            win::place(hwnd);
            theme::setup_fonts(&cc.egui_ctx);
            let _ = CTX.set(cc.egui_ctx.clone());
            // Test hook: AGENT_ISLANDS_OPEN=<session id> opens the panel on that session once it exists.
            let open_req = std::env::var("AGENT_ISLANDS_OPEN").ok();
            let app = App {
                core: Core::new(), rx, hwnd, trimmed: false, selected: None, open_req, was_down: false, card_since: None,
                // Test hook, debug builds only: AGENT_ISLANDS_CLICK=allow|deny answers the first permission card 1 s after it shows.
                #[cfg(debug_assertions)]
                auto_click: parse_click(std::env::var("AGENT_ISLANDS_CLICK").ok().as_deref()),
            };
            Ok(Box::new(app))
        }),
    );
}

struct App {
    core: Core,
    rx: Receiver<Msg>,
    hwnd: HWND,
    trimmed: bool,
    selected: Option<String>,
    open_req: Option<String>,
    was_down: bool,
    /// The permission request on the card and when it appeared.
    card_since: Option<(u64, f64)>,
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
        if self.core.tick(Instant::now()) {
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
        let view = views::current_view(&self.core, self.selected.is_some());
        // The bar marks the session a card is about (island-left.html state 3), else the selection.
        let marked = match &view {
            View::Permission(id) => self.core.state.permissions.iter().find(|p| p.req_id == *id).map(|p| p.session_id.clone()),
            View::Waiting(id) => Some(id.clone()),
            _ => self.selected.clone(),
        };
        let pill = views::pill(ui, &self.core, marked.as_deref(), t);
        if pill.clicked() {
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
        let panel = match view {
            View::Permission(id) => {
                let (rect, mut reply) = views::permission_card(ui, &self.core, id, r, t);
                if self.card_since.is_none_or(|(c, _)| c != id) {
                    self.card_since = Some((id, t));
                }
                let since = self.card_since.map_or(t, |(_, t0)| t0);
                if !views::clickable(since, t) {
                    reply = None;
                }
                #[cfg(debug_assertions)]
                if t - since >= 1.0 && reply.is_none() {
                    reply = self.auto_click.take();
                }
                if let Some(reply) = reply {
                    self.core.answer(id, reply);
                }
                rect
            }
            View::Waiting(id) => {
                let (rect, go) = views::waiting_card(ui, &self.core, &id, r, t);
                let s = self.core.state.sessions.iter().find(|s| s.id == id);
                if go && let Some(s) = s && let Some(pid) = s.terminal_pid {
                    crate::focus::go_to_terminal(pid, s.title.as_deref());
                }
                rect
            }
            View::Panel | View::Pill => {
                let panel = views::panel(ui, &self.core, &mut self.selected, r, t);
                if panel.is_positive() {
                    panel
                } else {
                    let (rect, open) = views::finished_pill(ui, &self.core, r);
                    if let Some(id) = open {
                        self.open_finished(id);
                    }
                    rect
                }
            }
        };
        // egui sees no hover while passthrough is on, so ask Windows where the cursor is.
        let over = win::cursor(self.hwnd, ctx.pixels_per_point()).is_some_and(|p| views::inside(p, r, panel));
        win::sync_style(self.hwnd, !over);
        let down = win::left_down();
        if self.selected.is_some() && views::outside_click(self.was_down, down, over) {
            self.selected = None;
        }
        self.was_down = down;
        let moving = panel.is_positive() || self.core.state.sessions.iter().any(|s| matches!(s.status, crate::state::Status::Working | crate::state::Status::Waiting(_)));
        ctx.request_repaint_after(Duration::from_millis(if moving { 33 } else { 100 }));
    }
}

mod win {
    use super::{Pos2, views::MARGIN};
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

    /// Top-left of the pill at (8, 96) logical px inside the primary monitor's work area.
    pub fn place(hwnd: HWND) {
        let mut work = RECT::default();
        let (mut dpi, mut dpi_y) = (96u32, 96u32);
        unsafe {
            let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut work as *mut RECT as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
            let _ = GetDpiForMonitor(MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY), MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
            let s = dpi as f32 / 96.0;
            let (x, y) = (work.left + ((8.0 - MARGIN) * s).round() as i32, work.top + ((96.0 - MARGIN) * s).round() as i32);
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

    /// Never activate, no taskbar or Alt+Tab entry, always layered; only TRANSPARENT (click-through) toggles.
    /// winit rewrites GWL_EXSTYLE whenever its own flags change (e.g. on first show), so sync_style
    /// runs every frame and owns passthrough instead of ViewportCommand::MousePassthrough.
    pub fn ex_style(cur: isize, passthrough: bool) -> isize {
        let t = WS_EX_TRANSPARENT.0 as isize;
        let want = (cur | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED).0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
        if passthrough { want | t } else { want & !t }
    }

    pub fn sync_style(hwnd: HWND, passthrough: bool) {
        unsafe {
            let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let want = ex_style(cur, passthrough);
            if want == cur {
                return;
            }
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
            if cur & WS_EX_LAYERED.0 as isize == 0 {
                // Only when LAYERED is (re)applied: winit drops it whenever it rewrites the style (e.g. first show).
                // Without LWA_ALPHA a layered GL window composites see-through.
                let _ = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 255, LWA_ALPHA);
                let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::mpsc::channel;

    fn raw(name: &str, sid: &str) -> Value {
        json!({"hook_event_name": name, "session_id": sid, "cwd": r"D:\p\x", "transcript_path": "t.jsonl", "tool_name": "Bash", "tool_input": {"command": "ls"}})
    }

    fn event(core: &mut Core, name: &str, sid: &str, wants: bool) -> Option<(std::sync::mpsc::Receiver<Reply>, u64)> {
        let (rtx, rrx) = channel();
        let (qtx, qrx) = channel();
        let reply = wants.then_some((rtx, qtx));
        core.handle(Msg::Event { raw: raw(name, sid), chain: vec![], reply }, 1);
        wants.then(|| (rrx, qrx.try_recv().unwrap()))
    }

    #[test]
    fn answer_sends_reply_and_resolves() {
        let mut c = Core::new();
        let (rx, id) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        assert_eq!(c.state.permissions.len(), 1);
        c.answer(id, Reply::Allow);
        assert!(matches!(rx.try_recv(), Ok(Reply::Allow)));
        assert!(c.state.permissions.is_empty());
        assert!(c.replies.is_empty());
    }

    #[test]
    fn answer_twice_sends_once() {
        let mut c = Core::new();
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
    fn timeout_msg_marks_timed_out() {
        let mut c = Core::new();
        let (_rx, id) = event(&mut c, "PermissionRequest", "s", true).unwrap();
        c.handle(Msg::Timeout(id), 2);
        assert!(c.state.permissions[0].timed_out);
        assert!(c.replies.is_empty());
    }

    #[test]
    fn ex_style_keeps_layered_and_toggles_only_transparent() {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let has = |s: isize, f: WINDOW_EX_STYLE| s & f.0 as isize != 0;
        let start = (WS_EX_APPWINDOW | WS_EX_TOPMOST).0 as isize;
        for through in [true, false] {
            let s = win::ex_style(start, through);
            assert!(has(s, WS_EX_LAYERED), "layered stays on (through={through})");
            assert!(has(s, WS_EX_NOACTIVATE) && has(s, WS_EX_TOOLWINDOW) && has(s, WS_EX_TOPMOST));
            assert!(!has(s, WS_EX_APPWINDOW));
            assert_eq!(has(s, WS_EX_TRANSPARENT), through);
        }
    }

    #[test]
    fn tick_exits_after_3s_empty_not_before() {
        let mut c = Core::new();
        let t = Instant::now();
        assert!(!c.tick(t));
        assert!(!c.tick(t + Duration::from_millis(2900)));
        assert!(c.tick(t + Duration::from_secs(3)));
    }

    #[test]
    fn tick_does_not_exit_while_session_exists() {
        let mut c = Core::new();
        event(&mut c, "SessionStart", "s", false);
        let t = Instant::now();
        assert!(!c.tick(t));
        assert!(!c.tick(t + Duration::from_secs(10)));
    }
}
