// Leaf – ein schlanker Browser in Rust.
// Natives Fenster (tao) + System-WebView (wry → WebView2).
// RAM/CPU sparen: ein gemeinsamer Browser-Prozess, versteckte Tabs werden
// eingefroren, nach 10 min ganz entladen, neue Tabs kosten gar keinen WebView.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adblock;
mod icon;
mod store;
#[cfg(windows)]
mod win;

use serde::Deserialize;
use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use store::{bare, Favorites, History, Session, Settings, WinState};
use tao::{
    dpi::{LogicalPosition as TPos, LogicalSize},
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Fullscreen, Icon, ResizeDirection, Theme, Window, WindowBuilder},
};
use wry::{
    dpi::{LogicalPosition, LogicalSize as WSize},
    http::Request,
    PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder,
};

const BAR_H: f64 = 84.0;
const FRAME: f64 = 8.0;
const SEARCH: &str = "https://www.google.com/search?q=";
const DISCARD_AFTER: Duration = Duration::from_secs(10 * 60);
const TICK: Duration = Duration::from_secs(30);
const PORT: u16 = 47291;
const UI_HTML: &str = include_str!("ui.html");

#[cfg(windows)]
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,\
msEdgeShoppingAssistant,msEdgeCollections,Translate,OptimizationHints,MediaRouter,BackForwardCache \
--disable-background-networking --disable-component-update --disable-domain-reliability \
--disable-sync --no-pings --process-per-site --renderer-process-limit=4";

/// Tastenkürzel in Webseiten → nur eine feste Liste an Befehlen erlaubt.
const SHORTCUTS_JS: &str = r#"(function(){if(window.top!==window)return;
addEventListener('keydown',function(e){var c=e.ctrlKey||e.metaKey,k=e.key.toLowerCase(),m=null;
if(c&&e.shiftKey&&k==='t')m='reopen_tab';else if(c&&(k==='t'||k==='n'))m='new_tab';else if(c&&k==='w')m='close_tab';
else if((c&&k==='l')||e.key==='F6')m='focus_url';else if(c&&e.key==='Tab')m=e.shiftKey?'prev_tab':'next_tab';
else if(e.key==='F11')m='fullscreen';else if(c&&e.shiftKey&&k==='j')m='downloads';else if(c&&k==='d')m='toggle_fav';
else if(c&&!e.shiftKey&&!e.altKey&&k>='1'&&k<='9')m='tab_'+k;
if(m){e.preventDefault();e.stopPropagation();window.ipc.postMessage(m);}},true);})();"#;

pub enum UserEvent {
    Ui(String),
    Key(String),
    Title(u32, String),
    Load(u32, bool, String),
    NewTab(String),
    Download,
    Blocked(u32),
    FullScreen(u32, bool),
    Nav(u32),
    NavError(u32, String),
    Favicon(u32, String),
    Open(String),
    CookiesSaved,
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Cmd {
    Navigate { url: String },
    Back,
    Forward,
    Reload,
    Stop,
    NewTab,
    CloseTab { id: Option<u32> },
    Switch { id: u32 },
    MoveTab { id: u32, to: usize },
    TabIndex { i: usize },
    NextTab,
    PrevTab,
    Mute { id: u32 },
    FocusUrl,
    Ready,
    Drag,
    Max,
    Min,
    Close,
    Resize { dir: String },
    Overlay { on: bool },
    Suggest { q: String },
    Downloads,
    ToggleAdblock,
    ToggleFav,
    RemoveFav { url: String },
    ClearHistory,
    ClearData,
    ReopenTab,
    Fullscreen,
}

struct Tab {
    id: u32,
    title: String,
    /// leer = Startseite (kein WebView, kostet ~0 RAM)
    url: String,
    favicon: String,
    loading: bool,
    view: Option<WebView>,
    last_active: Instant,
    blocked: u32,
    /// zeigt gerade Leafs eigene Fehlerseite
    error: bool,
}

/// Eigene Fehlerseite im Leaf-Look (statt der Edge-Seite von WebView2).
fn error_html(url: &str, msg: &str) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let host = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = host.split(['/', '?', '#']).next().unwrap_or(host);
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Nicht erreichbar</title><style>
html,body{{height:100%;margin:0}}
body{{display:flex;align-items:center;justify-content:center;background:radial-gradient(60% 50% at 50% 30%,#14251a,#0d110e 70%);
color:#eef2ef;font:15px/1.5 "Segoe UI Variable Text","Segoe UI",system-ui,sans-serif}}
.c{{max-width:460px;padding:24px;text-align:center}}
canvas{{width:44px;height:44px;margin-bottom:18px;image-rendering:pixelated}}
h1{{font-size:22px;font-weight:600;letter-spacing:-.02em;margin:0 0 6px}}
.h{{color:#30d158;font-weight:600;word-break:break-all}}
p{{color:rgba(238,242,239,.62);margin:14px 0 26px}}
button{{font:inherit;font-size:13.5px;color:#0d110e;background:#30d158;border:0;border-radius:18px;padding:8px 20px;cursor:pointer}}
button:hover{{background:#4ade80}}
</style></head><body><div class="c">
<canvas id="l" width="16" height="16"></canvas>
<script>(()=>{{const R=["................","..........#####.","........##LLLL#.","......##LLLLGG#.",".....#LLLLGGGG#.","....#LLLGGGGVG#.","....#LLGGGGVGG#.","...#LLGGGGVGG#..","...#LGGGGVGGG#..","...#LGGGVGGG#...","...#GGGVGGG#....","...#GGVGG##.....","....#VG##.......","...#V##.........","..#V#...........",".##............."],
C={{'#':'#14532d','L':'#86efac','G':'#22c55e','V':'#15803d'}},x=document.getElementById('l').getContext('2d');
R.forEach((r,y)=>[...r].forEach((c,i)=>{{if(C[c]){{x.fillStyle=C[c];x.fillRect(i,y,1,1)}}}}));}})()</script>
<h1>Seite nicht erreichbar</h1><div class="h">{}</div><p>{}</p>
<button onclick="window.ipc.postMessage('reload')">Erneut versuchen</button></div></body></html>"#,
        esc(host),
        esc(msg)
    )
}

impl Tab {
    fn new(id: u32, url: &str) -> Self {
        Tab {
            id,
            title: if url.is_empty() {
                "Neuer Tab".into()
            } else {
                bare(url).into()
            },
            url: url.into(),
            favicon: String::new(),
            loading: false,
            view: None,
            last_active: Instant::now(),
            blocked: 0,
            error: false,
        }
    }
}

struct Browser {
    window: Window,
    ctx: WebContext,
    proxy: EventLoopProxy<UserEvent>,
    toolbar: WebView,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u32,
    dir: PathBuf,
    history: History,
    favs: Favorites,
    settings: Settings,
    saved_session: Session,
    overlay: bool,
    fullscreen: bool,
    downloads: bool,
    closed: Vec<String>,
    /// Änderungszeit des Hintergrundbilds (0 = keins) – dient auch als Cache-Buster
    wall: u64,
    /// Beenden läuft: Fenster ist weg, Session-Cookies werden noch gesichert
    quitting: Option<Instant>,
    ticks: u32,
}

fn wall_stamp() -> u64 {
    #[cfg(windows)]
    if let Some(p) = win::wallpaper() {
        return std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs().max(1))
            .unwrap_or(1);
    }
    0
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(x, y).into(),
        size: WSize::new(w.max(0.0), h.max(0.0)).into(),
    }
}

#[cfg(windows)]
fn platform<'a>(b: WebViewBuilder<'a>) -> WebViewBuilder<'a> {
    use wry::WebViewBuilderExtWindows;
    b.with_additional_browser_args(BROWSER_ARGS)
}
#[cfg(not(windows))]
fn platform<'a>(b: WebViewBuilder<'a>) -> WebViewBuilder<'a> {
    b
}

fn make_tab_view(
    window: &Window,
    ctx: &mut WebContext,
    proxy: &EventLoopProxy<UserEvent>,
    id: u32,
    url: &str,
    bounds: Rect,
) -> wry::Result<WebView> {
    let (p1, p2, p3, p4, p5) = (
        proxy.clone(),
        proxy.clone(),
        proxy.clone(),
        proxy.clone(),
        proxy.clone(),
    );
    let mut b = WebViewBuilder::with_web_context(ctx)
        .with_bounds(bounds)
        .with_url(url)
        .with_background_color((255, 255, 255, 255))
        .with_initialization_script(SHORTCUTS_JS);
    if adblock::enabled() {
        b = b.with_initialization_script(adblock::COSMETIC_JS);
    }
    let b = b
        .with_ipc_handler(move |req: Request<String>| {
            let _ = p1.send_event(UserEvent::Key(req.into_body()));
        })
        .with_document_title_changed_handler(move |t| {
            let _ = p2.send_event(UserEvent::Title(id, t));
        })
        .with_on_page_load_handler(move |ev, url| {
            let _ = p3.send_event(UserEvent::Load(
                id,
                matches!(ev, PageLoadEvent::Started),
                url,
            ));
        })
        .with_new_window_req_handler(move |url| {
            let _ = p4.send_event(UserEvent::NewTab(url));
            false
        })
        .with_download_started_handler(move |_, _| {
            let _ = p5.send_event(UserEvent::Download);
            true // WebView2 zeigt sein natives Download-Panel mit Fortschritt
        });
    let view = platform(b).build_as_child(window)?;
    #[cfg(windows)]
    win::attach(&view, proxy.clone(), id);
    Ok(view)
}

fn hide(view: &WebView) {
    let _ = view.set_visible(false);
    #[cfg(windows)]
    win::suspend(view);
}

impl Browser {
    fn size(&self) -> (f64, f64) {
        let s = self
            .window
            .inner_size()
            .to_logical::<f64>(self.window.scale_factor());
        (s.width, s.height)
    }

    fn frame(&self) -> f64 {
        if self.fullscreen || self.window.is_maximized() {
            0.0
        } else {
            FRAME
        }
    }

    fn content_rect(&self) -> Rect {
        let (w, h) = self.size();
        if self.fullscreen {
            return rect(0.0, 0.0, w, h);
        }
        let f = self.frame();
        rect(f, BAR_H, w - 2.0 * f, h - BAR_H - f)
    }

    /// Fensterrand ans Theme anpassen; prüfen, ob sich das Hintergrundbild geändert hat.
    fn restyle(&mut self) {
        #[cfg(windows)]
        {
            use tao::platform::windows::WindowExtWindows;
            win::style_window(self.window.hwnd(), self.window.theme() == Theme::Dark);
        }
        let w = wall_stamp();
        if w != self.wall {
            self.wall = w;
            self.send_wall();
        }
    }

    /// Glas-Effekt: Die Oberfläche zeigt den unscharfen Desktop-Hintergrund genau an
    /// der Stelle, an der das Fenster steht → wirkt wie durchsichtig, kostet kaum etwas.
    fn send_wall(&self) {
        let sf = self.window.scale_factor();
        let (Some(m), Ok(p)) = (self.window.current_monitor(), self.window.inner_position()) else {
            return;
        };
        let mp = m.position().to_logical::<f64>(sf);
        let ms = m.size().to_logical::<f64>(sf);
        let p = p.to_logical::<f64>(sf);
        let _ = self.toolbar.evaluate_script(&format!(
            "ui.wall({},{:.0},{:.0},{:.0},{:.0})",
            self.wall,
            p.x - mp.x,
            p.y - mp.y,
            ms.width,
            ms.height
        ));
    }

    fn cur(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    fn view(&self) -> Option<&WebView> {
        self.cur().and_then(|t| t.view.as_ref())
    }

    fn is_home(&self) -> bool {
        self.cur().is_none_or(|t| t.view.is_none())
    }

    /// Die Oberfläche füllt immer das ganze Fenster; der Seiten-WebView liegt
    /// darüber, weil wry neue Views oben einsortiert.
    fn layout(&self) {
        let (w, h) = self.size();
        if self.fullscreen {
            let _ = self.toolbar.set_visible(false);
            if let Some(v) = self.view() {
                let _ = v.set_bounds(rect(0.0, 0.0, w, h));
            }
            return;
        }
        let _ = self.toolbar.set_visible(true);
        let _ = self.toolbar.set_bounds(rect(0.0, 0.0, w, h));
        if let Some(v) = self.view() {
            if self.overlay {
                let _ = v.set_visible(false);
            } else {
                let _ = v.set_bounds(self.content_rect());
                let _ = v.set_visible(true);
            }
        }
    }

    fn create_view(&mut self, idx: usize) {
        let bounds = self.content_rect();
        let (id, url) = (self.tabs[idx].id, self.tabs[idx].url.clone());
        match make_tab_view(&self.window, &mut self.ctx, &self.proxy, id, &url, bounds) {
            Ok(v) => {
                self.tabs[idx].view = Some(v);
                self.tabs[idx].loading = true;
            }
            Err(e) => eprintln!("WebView-Fehler: {e}"),
        }
    }

    fn new_tab(&mut self, url: &str) {
        let id = self.next_id;
        self.next_id += 1;
        // neben dem aktuellen Tab einfügen (wie Chrome/Safari)
        let at = if self.active < self.tabs.len() {
            self.active + 1
        } else {
            self.tabs.len()
        };
        self.tabs.insert(at, Tab::new(id, url));
        if self.active < self.tabs.len() && at <= self.active {
            self.active += 1;
        }
        self.activate(at);
        if url.is_empty() {
            self.focus_url();
        }
    }

    fn activate(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if idx != self.active {
            if let Some(v) = self.tabs.get(self.active).and_then(|t| t.view.as_ref()) {
                hide(v);
            }
        }
        self.active = idx;
        self.overlay = false;
        if self.tabs[idx].view.is_none() && !self.tabs[idx].url.is_empty() {
            self.create_view(idx);
        }
        self.tabs[idx].last_active = Instant::now();
        if let Some(v) = &self.tabs[idx].view {
            #[cfg(windows)]
            win::resume(v);
            let _ = v.focus();
        }
        self.layout();
        self.sync();
    }

    fn close_tab(&mut self, id: Option<u32>) -> bool {
        let idx = match id {
            Some(id) => match self.tabs.iter().position(|t| t.id == id) {
                Some(i) => i,
                None => return true,
            },
            None => self.active,
        };
        if self.tabs.len() == 1 {
            // letzter Tab → Fenster schließen, Tab bleibt aber in der Sitzung
            return false;
        }
        let t = self.tabs.remove(idx); // Drop gibt den WebView sofort frei
        if !t.url.is_empty() {
            self.closed.push(t.url);
            if self.closed.len() > 25 {
                self.closed.remove(0);
            }
        }
        if self.tabs.is_empty() {
            return false;
        }
        if idx < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        let a = self.active.min(self.tabs.len() - 1);
        self.active = usize::MAX;
        self.activate(a);
        true
    }

    fn navigate(&mut self, input: &str) {
        let url = fix_url(input);
        if url.is_empty() {
            return;
        }
        self.overlay = false;
        let idx = self.active;
        let Some(tab) = self.tabs.get_mut(idx) else {
            return;
        };
        tab.url = url.clone();
        match &tab.view {
            Some(v) => {
                let _ = v.load_url(&url);
                let _ = v.focus();
            }
            None => {
                self.create_view(idx);
                if let Some(v) = &self.tabs[idx].view {
                    let _ = v.focus();
                }
            }
        }
        self.layout();
        self.sync();
    }

    fn focus_url(&self) {
        let _ = self.toolbar.focus();
        let _ = self.toolbar.evaluate_script("ui.focusUrl()");
    }

    fn set_fullscreen(&mut self, on: bool) {
        self.fullscreen = on;
        self.window
            .set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        self.layout();
    }

    fn sync(&self) {
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .map(|t| {
                #[cfg(windows)]
                let (_, _, audio, muted) = t.view.as_ref().map(win::status).unwrap_or_default();
                #[cfg(not(windows))]
                let (audio, muted) = (false, false);
                json!({"id": t.id, "title": t.title, "url": t.url, "loading": t.loading,
                       "icon": t.favicon, "audio": audio, "muted": muted})
            })
            .collect();
        #[cfg(windows)]
        let (can_back, can_fwd, _, _) = self.view().map(win::status).unwrap_or_default();
        #[cfg(not(windows))]
        let (can_back, can_fwd) = (false, false);
        let home = self.is_home();
        let cur_url = self.cur().map(|t| t.url.as_str()).unwrap_or("");
        let (top, favs): (Vec<_>, Vec<_>) = if home {
            (
                self.history
                    .top(8)
                    .iter()
                    .filter(|v| !self.favs.has(&v.url))
                    .map(|v| json!({"url": v.url, "title": v.title}))
                    .collect(),
                self.favs
                    .items
                    .iter()
                    .map(|v| json!({"url": v.url, "title": v.title}))
                    .collect(),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let state = json!({
            "tabs": tabs,
            "active": self.cur().map(|t| t.id).unwrap_or(0),
            "home": home,
            "top": top,
            "favs": favs,
            "fav": self.favs.has(cur_url),
            "back": can_back,
            "fwd": can_fwd,
            "adblock": adblock::enabled(),
            "blocked": self.cur().map(|t| t.blocked).unwrap_or(0),
            "downloads": self.downloads,
            "max": self.window.is_maximized(),
        });
        let _ = self.toolbar.evaluate_script(&format!("ui.update({state})"));
        if let Some(t) = self.cur() {
            self.window.set_title(&format!("{} – Leaf", t.title));
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let active = self.active;
        for (i, t) in self.tabs.iter_mut().enumerate() {
            let Some(v) = &t.view else { continue };
            #[cfg(windows)]
            let audio = win::status(v).2;
            #[cfg(not(windows))]
            let audio = {
                let _ = v;
                false
            };
            if i != active
                && !t.loading
                && !audio
                && now.duration_since(t.last_active) > DISCARD_AFTER
            {
                t.view = None;
            }
        }
        self.history.flush();
        self.persist();
        // alle 5 min Session-Cookies sichern (falls Windows hart herunterfährt)
        self.ticks += 1;
        if self.ticks.is_multiple_of(10) {
            self.save_cookies();
        }
    }

    fn save_cookies(&self) {
        #[cfg(windows)]
        win::save_session_cookies(
            &self.toolbar,
            self.dir.join("session-cookies.bin"),
            self.proxy.clone(),
        );
    }

    fn save_window(&mut self) {
        if self.fullscreen {
            return;
        }
        let max = self.window.is_maximized();
        let sf = self.window.scale_factor();
        let mut ws = self.settings.window.clone().unwrap_or_default();
        ws.max = max;
        if !max && !self.window.is_minimized() {
            if let Ok(p) = self.window.outer_position() {
                let p = p.to_logical::<f64>(sf);
                let (w, h) = self.size();
                ws.x = p.x;
                ws.y = p.y;
                ws.w = w;
                ws.h = h;
            }
        }
        self.settings.window = Some(ws);
        store::save_settings(&self.dir, &self.settings);
    }

    /// Tab-Sitzung speichern – wird nach jedem Ereignis aufgerufen, schreibt aber
    /// nur, wenn sich wirklich etwas geändert hat (überlebt so auch Abstürze).
    fn persist(&mut self) {
        let open: Vec<&Tab> = self.tabs.iter().filter(|t| !t.url.is_empty()).collect();
        let active_id = self.cur().map(|t| t.id);
        let s = Session {
            tabs: open.iter().map(|t| t.url.clone()).collect(),
            titles: open.iter().map(|t| t.title.clone()).collect(),
            active: open
                .iter()
                .position(|t| Some(t.id) == active_id)
                .unwrap_or(0),
        };
        if s != self.saved_session {
            store::save_session(&self.dir, &s);
            self.saved_session = s;
        }
    }

    /// Beenden: Fenster sofort weg, alles speichern, dann Session-Cookies sichern.
    /// true = sofort beenden.
    fn begin_quit(&mut self) -> bool {
        if self.quitting.is_some() {
            return false;
        }
        self.save_window();
        self.persist();
        self.history.flush();
        self.window.set_visible(false);
        self.quitting = Some(Instant::now() + Duration::from_secs(3));
        QUITTING.store(true, Ordering::Relaxed);
        self.save_cookies();
        !cfg!(windows)
    }

    fn run_cmd(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Navigate { url } => self.navigate(&url),
            #[cfg(windows)]
            Cmd::Back => self.with_view(win::back),
            #[cfg(windows)]
            Cmd::Forward => self.with_view(win::forward),
            #[cfg(windows)]
            Cmd::Reload => match self.cur() {
                // auf der Fehlerseite: die eigentliche Adresse erneut laden
                Some(t) if t.error => {
                    if let Some(v) = &t.view {
                        let _ = v.load_url(&t.url);
                    }
                }
                _ => self.with_view(win::reload),
            },
            #[cfg(windows)]
            Cmd::Stop => self.with_view(win::stop),
            #[cfg(not(windows))]
            Cmd::Back | Cmd::Forward | Cmd::Reload | Cmd::Stop => {}
            Cmd::NewTab => self.new_tab(""),
            Cmd::CloseTab { id } => return self.close_tab(id),
            Cmd::Switch { id } => {
                if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                    self.activate(i);
                }
            }
            Cmd::MoveTab { id, to } => {
                if let Some(from) = self.tabs.iter().position(|t| t.id == id) {
                    let active_id = self.cur().map(|t| t.id);
                    let t = self.tabs.remove(from);
                    self.tabs.insert(to.min(self.tabs.len()), t);
                    if let Some(a) = active_id {
                        self.active = self.tabs.iter().position(|t| t.id == a).unwrap_or(0);
                    }
                    self.sync();
                }
            }
            Cmd::TabIndex { i } => {
                // Strg+9 = letzter Tab, wie in Chrome/Safari
                let idx = if i >= 9 {
                    self.tabs.len() - 1
                } else {
                    i.saturating_sub(1)
                };
                if idx < self.tabs.len() {
                    self.activate(idx);
                }
            }
            Cmd::NextTab => self.activate((self.active + 1) % self.tabs.len()),
            Cmd::PrevTab => self.activate((self.active + self.tabs.len() - 1) % self.tabs.len()),
            Cmd::Mute { id } => {
                #[cfg(windows)]
                if let Some(v) = self
                    .tabs
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.view.as_ref())
                {
                    win::toggle_mute(v);
                }
                let _ = id;
                self.sync();
            }
            Cmd::FocusUrl => self.focus_url(),
            Cmd::Ready => {
                self.sync();
                self.send_wall();
                if self.is_home() {
                    self.focus_url();
                }
            }
            Cmd::Drag => {
                let _ = self.window.drag_window();
            }
            Cmd::Max => self.window.set_maximized(!self.window.is_maximized()),
            Cmd::Min => self.window.set_minimized(true),
            Cmd::Close => return false,
            Cmd::Resize { dir } => {
                if self.frame() > 0.0 {
                    use ResizeDirection::*;
                    let d = match dir.as_str() {
                        "n" => North,
                        "s" => South,
                        "w" => West,
                        "e" => East,
                        "nw" => NorthWest,
                        "ne" => NorthEast,
                        "sw" => SouthWest,
                        "se" => SouthEast,
                        _ => return true,
                    };
                    let _ = self.window.drag_resize_window(d);
                }
            }
            Cmd::Overlay { on } => {
                if self.overlay != on {
                    self.overlay = on;
                    self.layout();
                }
            }
            Cmd::Suggest { q } => {
                let ql = q.trim().to_lowercase();
                let mut list: Vec<_> = self
                    .favs
                    .items
                    .iter()
                    .filter(|v| {
                        bare(&v.url).to_lowercase().contains(&ql)
                            || v.title.to_lowercase().contains(&ql)
                    })
                    .take(3)
                    .map(|v| json!({"url": v.url, "title": v.title, "fav": true}))
                    .collect();
                for v in self.history.search(&q, 8) {
                    if list.len() >= 7 {
                        break;
                    }
                    if !self.favs.has(&v.url) {
                        list.push(json!({"url": v.url, "title": v.title}));
                    }
                }
                let _ = self
                    .toolbar
                    .evaluate_script(&format!("ui.suggest({})", json!(list)));
            }
            Cmd::Downloads => {
                #[cfg(windows)]
                self.with_view(win::open_downloads);
            }
            Cmd::ToggleAdblock => {
                self.settings.adblock = !adblock::enabled();
                adblock::set_enabled(self.settings.adblock);
                store::save_settings(&self.dir, &self.settings);
                #[cfg(windows)]
                self.with_view(win::reload);
                self.sync();
            }
            Cmd::ToggleFav => {
                if let Some(t) = self.cur() {
                    if !t.url.is_empty() {
                        let (u, ti) = (t.url.clone(), t.title.clone());
                        self.favs.toggle(&u, &ti);
                        self.sync();
                    }
                }
            }
            Cmd::RemoveFav { url } => {
                self.favs.remove(&url);
                self.sync();
            }
            Cmd::ClearHistory => {
                self.history.clear();
                self.history.flush();
                self.sync();
            }
            Cmd::ClearData => {
                self.history.clear();
                self.history.flush();
                self.closed.clear();
                let _ = std::fs::remove_file(self.dir.join("session-cookies.bin"));
                #[cfg(windows)]
                win::clear_browsing_data(&self.toolbar);
                self.sync();
            }
            Cmd::ReopenTab => {
                if let Some(u) = self.closed.pop() {
                    self.new_tab(&u);
                }
            }
            Cmd::Fullscreen => self.set_fullscreen(!self.fullscreen),
        }
        true
    }

    #[cfg(windows)]
    fn with_view(&self, f: fn(&WebView)) {
        if let Some(v) = self.view() {
            f(v);
        }
    }

    /// false = Fenster schließen
    fn handle(&mut self, ev: UserEvent) -> bool {
        match ev {
            UserEvent::Ui(msg) => match serde_json::from_str::<Cmd>(&msg) {
                Ok(cmd) => return self.run_cmd(cmd),
                Err(e) => eprintln!("UI-Befehl ungültig: {e}"),
            },
            UserEvent::Key(k) => {
                let cmd = match k.as_str() {
                    "new_tab" => Cmd::NewTab,
                    "close_tab" => Cmd::CloseTab { id: None },
                    "next_tab" => Cmd::NextTab,
                    "prev_tab" => Cmd::PrevTab,
                    "focus_url" => Cmd::FocusUrl,
                    "reopen_tab" => Cmd::ReopenTab,
                    "fullscreen" => Cmd::Fullscreen,
                    "downloads" => Cmd::Downloads,
                    "toggle_fav" => Cmd::ToggleFav,
                    "reload" => Cmd::Reload,
                    k if k.starts_with("tab_") => match k[4..].parse() {
                        Ok(i) => Cmd::TabIndex { i },
                        Err(_) => return true,
                    },
                    _ => return true,
                };
                return self.run_cmd(cmd);
            }
            UserEvent::Title(id, title) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    if !title.trim().is_empty() {
                        if !t.error {
                            self.history.set_title(&t.url, &title);
                            self.favs.set_title(&t.url, &title);
                        }
                        t.title = title;
                    }
                }
                self.sync();
            }
            UserEvent::Load(id, started, url) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    t.loading = started;
                    if started {
                        t.blocked = 0;
                        if url != "about:blank" {
                            t.error = false;
                        }
                    } else if !t.error {
                        self.history.visit(&url);
                        self.history.set_title(&url, &t.title);
                    }
                    if url != "about:blank" {
                        t.url = url;
                    }
                }
                self.sync();
            }
            UserEvent::Blocked(id) => {
                let mut redraw = false;
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    t.blocked += 1;
                    redraw = t.blocked % 8 == 1;
                }
                if redraw && self.cur().is_some_and(|t| t.id == id) {
                    self.sync();
                }
            }
            UserEvent::FullScreen(id, on) => {
                if self.cur().is_some_and(|t| t.id == id) {
                    self.set_fullscreen(on);
                }
            }
            UserEvent::Nav(_) => self.sync(),
            UserEvent::NavError(id, msg) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    t.error = true;
                    t.loading = false;
                    t.favicon.clear();
                    if let Some(v) = &t.view {
                        let _ = v.load_html(&error_html(&t.url, &msg));
                    }
                }
                self.sync();
            }
            UserEvent::Favicon(id, uri) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    t.favicon = if uri.starts_with("http") || uri.starts_with("data:") {
                        uri
                    } else {
                        String::new()
                    };
                }
                self.sync();
            }
            UserEvent::Download => {
                self.downloads = true;
                self.sync();
            }
            UserEvent::CookiesSaved => {}
            UserEvent::NewTab(url) => self.new_tab(&url),
            UserEvent::Open(url) => {
                let u = url.trim();
                let empty_open = self.cur().is_some_and(|t| t.url.is_empty());
                match (u.is_empty(), empty_open) {
                    (true, true) => self.focus_url(),  // leerer Tab ist schon da
                    (false, true) => self.navigate(u), // leeren Tab wiederverwenden
                    (true, false) => self.new_tab(""),
                    (false, false) => self.new_tab(&fix_url(u)),
                }
                self.window.set_minimized(false);
                self.window.set_focus();
            }
        }
        true
    }
}

fn fix_url(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return String::new();
    }
    if s.contains("://")
        || s.starts_with("about:")
        || s.starts_with("file:")
        || s.starts_with("mailto:")
    {
        return s.into();
    }
    if s.len() > 2
        && s.as_bytes()[1] == b':'
        && (s.as_bytes()[2] == b'\\' || s.as_bytes()[2] == b'/')
    {
        return format!("file:///{}", s.replace('\\', "/"));
    }
    let local = s.starts_with("localhost") || s.starts_with("127.0.0.1");
    if !s.contains(' ') && (s.contains('.') || local) {
        return if local {
            format!("http://{s}")
        } else {
            format!("https://{s}")
        };
    }
    let q: String = s
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("{SEARCH}{q}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(fix_url("github.com"), "https://github.com");
        assert_eq!(fix_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(
            fix_url("rust lernen"),
            "https://www.google.com/search?q=rust+lernen"
        );
        assert_eq!(fix_url("ä?"), "https://www.google.com/search?q=%C3%A4%3F");
        assert_eq!(fix_url(r"C:\a\b.html"), "file:///C:/a/b.html");
    }

    #[test]
    fn blocker() {
        adblock::init(std::path::Path::new("."), true);
        assert!(adblock::is_blocked(
            "https://securepubads.g.doubleclick.net/tag/js/gpt.js"
        ));
        assert!(adblock::is_blocked(
            "https://www.google-analytics.com/analytics.js"
        ));
        assert!(adblock::is_blocked("https://user:pw@ads.yahoo.com:443/x"));
        assert!(!adblock::is_blocked(
            "https://www.google.com/search?q=doubleclick.net"
        ));
        assert!(!adblock::is_blocked("https://www.youtube.com/watch?v=1"));
        assert!(!adblock::is_blocked("https://notdoubleclick.net/"));
        adblock::set_enabled(false);
        assert!(!adblock::is_blocked("https://doubleclick.net/"));
    }
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let d = base.join("Leaf");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Wird gerade beendet → keine Links mehr von anderen Instanzen annehmen.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Läuft Leaf schon? Dann URL an die laufende Instanz übergeben.
/// Nur wenn sie mit "1" bestätigt, gilt die Übergabe als erledigt.
fn hand_off(arg: &str) -> bool {
    let addr = ([127, 0, 0, 1], PORT).into();
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(150)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    if s.write_all(arg.as_bytes()).is_err() || s.shutdown(std::net::Shutdown::Write).is_err() {
        return false;
    }
    let mut ack = [0u8; 1];
    matches!(s.read(&mut ack), Ok(1) if ack[0] == b'1')
}

fn listen(proxy: EventLoopProxy<UserEvent>) {
    let Ok(l) = TcpListener::bind(("127.0.0.1", PORT)) else {
        return;
    };
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut buf = String::new();
            let _ = (&mut s).take(8192).read_to_string(&mut buf);
            let ok =
                !QUITTING.load(Ordering::Relaxed) && proxy.send_event(UserEvent::Open(buf)).is_ok();
            let _ = s.write_all(if ok { b"1" } else { b"0" });
        }
    });
}

fn main() -> wry::Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_default();
    if hand_off(&arg) {
        return Ok(());
    }

    let dir = data_dir();
    let settings = store::load_settings(&dir);
    adblock::init(&dir, settings.adblock);

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    listen(proxy.clone());

    let ws: WinState = settings.window.clone().unwrap_or_default();
    let mut wb = WindowBuilder::new()
        .with_title("Leaf")
        .with_decorations(false)
        .with_visible(false)
        .with_window_icon(Icon::from_rgba(icon::rgba(32), 32, 32).ok())
        .with_inner_size(LogicalSize::new(ws.w, ws.h))
        .with_min_inner_size(LogicalSize::new(520.0, 360.0))
        .with_maximized(ws.max);
    if ws.w > 0.0 && (ws.x != 0.0 || ws.y != 0.0) {
        wb = wb.with_position(TPos::new(ws.x, ws.y));
    }
    #[cfg(windows)]
    let wb = {
        use tao::platform::windows::WindowBuilderExtWindows;
        wb.with_undecorated_shadow(true)
    };
    let window = wb
        .build(&event_loop)
        .expect("Fenster konnte nicht erstellt werden");

    let mut ctx = WebContext::new(Some(dir.clone()));
    let p = proxy.clone();
    let s = window.inner_size().to_logical::<f64>(window.scale_factor());
    let toolbar = platform(
        WebViewBuilder::with_web_context(&mut ctx)
            .with_bounds(rect(0.0, 0.0, s.width, s.height))
            .with_html(UI_HTML)
            .with_background_color((19, 22, 20, 255))
            // liefert das Desktop-Hintergrundbild an die Oberfläche: http://leaf.localhost/wall
            .with_custom_protocol("leaf".into(), |_, _req| {
                #[cfg(windows)]
                let bytes = win::wallpaper()
                    .and_then(|p| std::fs::read(p).ok())
                    .unwrap_or_default();
                #[cfg(not(windows))]
                let bytes: Vec<u8> = Vec::new();
                wry::http::Response::builder()
                    .header("Content-Type", "image/jpeg")
                    .header("Cache-Control", "max-age=31536000")
                    .body(std::borrow::Cow::Owned(bytes))
                    .unwrap()
            })
            .with_ipc_handler(move |req: Request<String>| {
                let _ = p.send_event(UserEvent::Ui(req.into_body()));
            }),
    )
    .build_as_child(&window)?;

    let session = store::load_session(&dir);
    let mut b = Browser {
        window,
        ctx,
        proxy,
        toolbar,
        tabs: Vec::new(),
        active: 0,
        next_id: 1,
        history: History::open(&dir),
        favs: Favorites::open(&dir),
        dir,
        settings,
        saved_session: Session::default(),
        overlay: false,
        fullscreen: false,
        downloads: false,
        closed: Vec::new(),
        wall: wall_stamp(),
        quitting: None,
        ticks: 0,
    };
    // Session-Cookies vom letzten Mal zurückspielen, bevor ein Tab lädt
    #[cfg(windows)]
    win::restore_session_cookies(&b.toolbar, &b.dir.join("session-cookies.bin"));
    b.restyle();

    // Sitzung wiederherstellen – Tabs werden erst beim Anklicken geladen.
    for (i, url) in session.tabs.iter().enumerate() {
        let id = b.next_id;
        b.next_id += 1;
        let mut t = Tab::new(id, url);
        if let Some(title) = session.titles.get(i).filter(|s| !s.is_empty()) {
            t.title = title.clone();
        }
        b.tabs.push(t);
    }
    b.saved_session = session.clone();
    b.active = usize::MAX;
    if b.tabs.is_empty() {
        b.active = 0;
        b.new_tab("");
    } else {
        b.activate(session.active.min(b.tabs.len() - 1));
    }
    if !arg.is_empty() {
        b.new_tab(&fix_url(&arg));
    }
    b.window.set_visible(true);
    b.layout(); // Größe erst nach dem Anzeigen endgültig
    b.restyle();

    let mut next_tick = Instant::now() + TICK;
    event_loop.run(move |event, _, cf| {
        *cf = ControlFlow::WaitUntil(b.quitting.map_or(next_tick, |q| q.min(next_tick)));
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if let Some(q) = b.quitting {
                    // Sicherheitsnetz: Cookie-Sicherung hängt → trotzdem beenden
                    if Instant::now() >= q {
                        *cf = ControlFlow::Exit;
                    }
                    return;
                }
                b.tick();
                next_tick = Instant::now() + TICK;
                *cf = ControlFlow::WaitUntil(next_tick);
            }
            Event::UserEvent(UserEvent::CookiesSaved) => {
                if b.quitting.is_some() {
                    *cf = ControlFlow::Exit;
                }
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    if b.begin_quit() {
                        *cf = ControlFlow::Exit;
                    }
                }
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    b.layout();
                    b.sync();
                    b.send_wall();
                }
                WindowEvent::Moved(_) => b.send_wall(),
                // Theme oder Hintergrundbild könnten sich geändert haben
                WindowEvent::ThemeChanged(_) | WindowEvent::Focused(true) => b.restyle(),
                _ => {}
            },
            Event::UserEvent(e) => {
                if b.quitting.is_some() {
                    return;
                }
                if b.handle(e) {
                    b.persist();
                } else if b.begin_quit() {
                    *cf = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    });
}
