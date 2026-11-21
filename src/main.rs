// Leaf – ein schlanker Browser in Rust.
// Natives Fenster (tao) + System-WebView (wry → WebView2).
// RAM/CPU sparen: ein gemeinsamer Browser-Prozess, versteckte Tabs werden nach
// 5 min eingefroren, nach 30 min entladen (nie, solange sie Ton abspielen),
// neue Tabs kosten gar keinen WebView.
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
const SUSPEND_AFTER: Duration = Duration::from_secs(5 * 60);
const DISCARD_AFTER: Duration = Duration::from_secs(30 * 60);
const TICK: Duration = Duration::from_secs(30);
const PORT: u16 = 47291;
const UI_HTML: &str = include_str!("ui.html");

/// Hintergrunddienste aus, die nur bremsen. Bewusst kein Prozess-Limit mehr:
/// das sparte etwas RAM, ließ bei einem Absturz aber gleich mehrere Tabs sterben.
#[cfg(windows)]
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,\
msEdgeShoppingAssistant,msEdgeCollections,Translate,OptimizationHints,MediaRouter \
--disable-background-networking --disable-component-update --disable-domain-reliability \
--disable-sync --no-pings";

/// Farbe der Seite für den Tab: theme-color, sonst die kräftigste Farbe des Favicons.
const COLOR_JS: &str = r#"(function(){if(window.top!==window)return;
var sent='';function send(c){if(c!==sent){sent=c;window.ipc.postMessage('color:'+c);}}
function hex(c){var x=document.createElement('canvas').getContext('2d');x.fillStyle='#000';x.fillStyle=c;var v=x.fillStyle;return /^#[0-9a-f]{6}$/i.test(v)?v:'';}
function vivid(h){var r=parseInt(h.substr(1,2),16),g=parseInt(h.substr(3,2),16),b=parseInt(h.substr(5,2),16),mx=Math.max(r,g,b),mn=Math.min(r,g,b);return mx-mn>40&&mx>60&&mn<225;}
function pick(img){try{var c=document.createElement('canvas');c.width=c.height=16;var x=c.getContext('2d');x.drawImage(img,0,0,16,16);
 var d=x.getImageData(0,0,16,16).data,best=null,bs=0;for(var k=0;k<d.length;k+=4){if(d[k+3]<200)continue;var r=d[k],g=d[k+1],b=d[k+2],s=Math.max(r,g,b)-Math.min(r,g,b);if(s>bs){bs=s;best=[r,g,b];}}
 if(best&&bs>60)send('#'+best.map(function(v){return ('0'+v.toString(16)).slice(-2);}).join(''));}catch(e){}}
function fromIcon(){var l=document.querySelector('link[rel~="icon"]'),u;try{u=new URL(l?l.href:'/favicon.ico',location.href);}catch(e){return;}
 if(u.origin===location.origin){var i=new Image();i.onload=function(){pick(i);};i.src=u.href;return;}
 /* Favicon auf fremdem Server (CDN): per CORS laden, dann ist es lesbar */
 fetch(u.href,{mode:'cors',credentials:'omit'}).then(function(r){return r.ok?r.blob():null;}).then(function(b){if(b&&window.createImageBitmap)return createImageBitmap(b);}).then(function(bm){if(bm)pick(bm);}).catch(function(){});}
function run(){var m=document.querySelector('meta[name="theme-color"]'),c=m?hex(m.content):'';if(c&&vivid(c))send(c);else fromIcon();}
if(document.readyState==='complete')setTimeout(run,300);else addEventListener('load',function(){setTimeout(run,300);});})();"#;

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
    /// Nachricht einer Webseite (Tab, Text) – nur feste Befehle erlaubt
    Key(u32, String),
    Title(u32, String),
    Load(u32, bool, String),
    NewTab(String),
    Blocked(u32),
    FullScreen(u32, bool),
    Nav(u32),
    NavError(u32, String),
    ShowError(u32, String),
    /// Tab hat eine externe App gestartet (steam:// …)
    External(u32),
    /// nur Testmodus: simulierter Klick (Tab, "x,y")
    DebugClick(u32, String),
    Favicon(u32, String),
    Open(String),
    CookiesSaved,
    DlStart {
        dl: u32,
        tab: u32,
        name: String,
        total: i64,
    },
    DlProgress {
        dl: u32,
        got: i64,
        total: i64,
    },
    DlDone {
        dl: u32,
        ok: bool,
    },
    Zoom(u32, f64),
    /// Prozess abgestürzt: (Tab, war es der Browser-Prozess?)
    Crashed(u32, bool),
}

struct Download {
    dl: u32,
    name: String,
    got: i64,
    total: i64,
    done: Option<bool>,
    at: Instant,
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
    Zoom { z: f64 },
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
    /// Markenfarbe der Seite (#rrggbb) oder leer
    color: String,
    /// seit wann im Hintergrund
    hidden_at: Option<Instant>,
    suspended: bool,
    /// letzter Download/App-Start aus diesem Tab (dessen Navigation „scheitert“ technisch)
    dl_at: Option<Instant>,
    /// Start der aktuellen Navigation (für die Ladezeit im Debug-Log)
    load_start: Option<Instant>,
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
            color: String::new(),
            hidden_at: None,
            suspended: false,
            dl_at: None,
            load_start: None,
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
    downloads: Vec<Download>,
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
    let (p1, p2, p3) = (proxy.clone(), proxy.clone(), proxy.clone());
    let mut b = WebViewBuilder::with_web_context(ctx)
        .with_bounds(bounds)
        .with_url(url)
        .with_background_color((255, 255, 255, 255))
        .with_initialization_script(SHORTCUTS_JS)
        .with_initialization_script(COLOR_JS);
    if adblock::enabled() {
        b = b.with_initialization_script(adblock::PAGE_JS);
    }
    let b = b
        .with_ipc_handler(move |req: Request<String>| {
            let _ = p1.send_event(UserEvent::Key(id, req.into_body()));
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
        });
    // Neue Fenster, Downloads, externe Apps: eigene Handler in win::attach
    let view = platform(b).build_as_child(window)?;
    #[cfg(windows)]
    win::attach(&view, proxy.clone(), id);
    Ok(view)
}

/// Nur ausblenden – Videos/Musik laufen weiter. Eingefroren wird erst später im tick.
fn hide(t: &mut Tab) {
    if let Some(v) = &t.view {
        let _ = v.set_visible(false);
        t.hidden_at = Some(Instant::now());
    }
}

/// Log für Fehlersuche: nur wenn die Umgebungsvariable LEAF_DEBUG gesetzt ist.
pub fn debug_log(msg: &str) {
    if std::env::var_os("LEAF_DEBUG").is_some() {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(data_dir().join("debug.log"))
        {
            let _ = writeln!(f, "{msg}");
        }
    }
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
            if let Some(t) = self.tabs.get_mut(self.active) {
                hide(t);
            }
        }
        self.active = idx;
        self.overlay = false;
        if self.tabs[idx].view.is_none() && !self.tabs[idx].url.is_empty() {
            self.create_view(idx);
        }
        let t = &mut self.tabs[idx];
        t.last_active = Instant::now();
        t.hidden_at = None;
        if let Some(v) = &t.view {
            #[cfg(windows)]
            if t.suspended {
                win::resume(v);
            }
            t.suspended = false;
            let _ = v.focus();
        }
        self.layout();
        self.sync();
    }

    fn close_tab(&mut self, id: Option<u32>) -> bool {
        debug_log(&format!("close_tab {id:?} (tabs: {})", self.tabs.len()));
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
                       "icon": t.favicon, "audio": audio, "muted": muted, "color": t.color})
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
            "dl": self.download_state(),
            "zoom": self.current_zoom(),
            "max": self.window.is_maximized(),
        });
        let _ = self.toolbar.evaluate_script(&format!("ui.update({state})"));
        if let Some(t) = self.cur() {
            self.window.set_title(&format!("{} – Leaf", t.title));
        }
    }

    /// Hintergrund-Tabs: nach 5 min einfrieren, nach 30 min entladen –
    /// aber nie, solange sie Ton abspielen (YouTube/Musik im Hintergrund).
    fn tick(&mut self) {
        let now = Instant::now();
        for t in self.tabs.iter_mut() {
            let (Some(v), Some(since)) = (&t.view, t.hidden_at) else {
                continue;
            };
            #[cfg(windows)]
            let audio = win::status(v).2;
            #[cfg(not(windows))]
            let audio = {
                let _ = v;
                false
            };
            if audio || t.loading {
                continue;
            }
            let idle = now.duration_since(since);
            if idle > DISCARD_AFTER {
                t.view = None;
                t.suspended = false;
            } else if idle > SUSPEND_AFTER && !t.suspended {
                #[cfg(windows)]
                win::suspend(v);
                t.suspended = true;
            }
        }
        self.downloads
            .retain(|d| d.done.is_none() || d.at.elapsed() < Duration::from_secs(600));
        self.history.flush();
        self.persist();
        // alle 5 min Session-Cookies sichern (falls Windows hart herunterfährt)
        self.ticks += 1;
        if self.ticks.is_multiple_of(10) {
            self.save_cookies();
        }
    }

    /// Für die Leiste: null = noch nie etwas geladen, sonst aktiver bzw. letzter Download
    fn download_state(&self) -> serde_json::Value {
        let Some(last) = self.downloads.last() else {
            return serde_json::Value::Null;
        };
        let running: Vec<&Download> = self.downloads.iter().filter(|d| d.done.is_none()).collect();
        let shown = running.first().copied().unwrap_or(last);
        let pct = if shown.total > 0 {
            (shown.got * 100 / shown.total).clamp(0, 100)
        } else {
            -1
        };
        json!({
            "running": running.len(),
            "name": shown.name,
            "pct": pct,
            "done": shown.done,
            "fresh": shown.at.elapsed() < Duration::from_secs(4),
        })
    }

    fn current_zoom(&self) -> f64 {
        #[cfg(windows)]
        if let Some(v) = self.view() {
            return (win::zoom(v) * 100.0).round() / 100.0;
        }
        1.0
    }

    /// Gespeicherten Zoom einer Seite (pro Domain, wie in Chrome) anwenden.
    fn apply_zoom(&self, idx: usize) {
        let Some(t) = self.tabs.get(idx) else { return };
        let Some(v) = &t.view else { return };
        let z = self
            .settings
            .zoom
            .get(host_of(&t.url))
            .copied()
            .unwrap_or(1.0);
        #[cfg(windows)]
        if (win::zoom(v) - z).abs() > 0.001 {
            let _ = v.zoom(z);
        }
        #[cfg(not(windows))]
        let _ = (v, z);
    }

    /// Farbe einer Seite übernehmen – nur gültige #rrggbb-Werte.
    fn set_color(&mut self, id: u32, c: &str) {
        let valid =
            c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit());
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            let c = if valid {
                c.to_ascii_lowercase()
            } else {
                String::new()
            };
            if t.color != c {
                t.color = c;
                self.sync();
            }
        }
    }

    /// Alles speichern und Leaf neu starten (nach einem Absturz der Web-Engine).
    fn restart(&mut self) {
        debug_log("restart");
        self.save_window();
        self.persist();
        self.history.flush();
        QUITTING.store(true, Ordering::Relaxed);
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).arg(RESTART_ARG).spawn();
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
        debug_log("begin_quit");
        #[cfg(windows)]
        win::close_popups(&self.toolbar);
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
            // (ohne Tabs würde % durch 0 teilen → Absturz)
            Cmd::NextTab if !self.tabs.is_empty() => {
                self.activate(self.active.wrapping_add(1) % self.tabs.len())
            }
            Cmd::PrevTab if !self.tabs.is_empty() => {
                let n = self.tabs.len();
                self.activate((self.active.min(n - 1) + n - 1) % n)
            }
            Cmd::NextTab | Cmd::PrevTab => {}
            Cmd::Zoom { z } => {
                let z = z.clamp(0.25, 5.0);
                if let Some(t) = self.cur() {
                    let host = host_of(&t.url).to_string();
                    if let Some(v) = &t.view {
                        let _ = v.zoom(z);
                    }
                    if (z - 1.0).abs() < 0.001 {
                        self.settings.zoom.remove(&host);
                    } else if !host.is_empty() {
                        self.settings.zoom.insert(host, z);
                    }
                    store::save_settings(&self.dir, &self.settings);
                }
            }
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
            Cmd::Close => {
                debug_log("cmd close");
                return false;
            }
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
            UserEvent::Key(id, k) => {
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
                    k if k.starts_with("color:") => {
                        self.set_color(id, &k[6..]);
                        return true;
                    }
                    k if k.starts_with("pruned:") => {
                        debug_log(&format!("tab {id} werbung entfernt: {}", &k[7..]));
                        return true;
                    }
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
                        t.load_start = Some(Instant::now());
                    } else if let Some(s) = t.load_start.take() {
                        debug_log(&format!(
                            "tab {id} geladen in {} ms: {url}",
                            s.elapsed().as_millis()
                        ));
                    }
                    if started {
                        t.blocked = 0;
                        if url != "about:blank" {
                            t.error = false;
                        }
                    } else if !t.error {
                        self.history.visit(&url);
                        self.history.set_title(&url, &t.title);
                    }
                    if !url.is_empty() && url != "about:blank" {
                        if host_of(&t.url) != host_of(&url) {
                            t.color.clear(); // neue Seite → neue Farbe
                        }
                        t.url = url;
                    }
                }
                if !started {
                    if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                        self.apply_zoom(i);
                    }
                    // Testmodus: einmal einen echten Klick an LEAF_CLICK="x,y" auslösen
                    if std::env::var_os("LEAF_DEBUG").is_some() {
                        if let Ok(xy) = std::env::var("LEAF_CLICK") {
                            static DONE: AtomicBool = AtomicBool::new(false);
                            if !DONE.swap(true, Ordering::Relaxed) {
                                let p = self.proxy.clone();
                                std::thread::spawn(move || {
                                    std::thread::sleep(Duration::from_millis(1500));
                                    let _ = p.send_event(UserEvent::DebugClick(id, xy));
                                });
                            }
                        }
                    }
                }
                self.sync();
            }
            UserEvent::DebugClick(id, xy) => {
                #[cfg(windows)]
                if let (Some(v), Some((x, y))) = (
                    self.tabs
                        .iter()
                        .find(|t| t.id == id)
                        .and_then(|t| t.view.as_ref()),
                    xy.split_once(','),
                ) {
                    debug_log(&format!("tab {id} klick bei {x},{y}"));
                    win::debug_click(
                        v,
                        x.trim().parse().unwrap_or(0),
                        y.trim().parse().unwrap_or(0),
                    );
                }
                let _ = (id, xy);
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
            UserEvent::Nav(id) => {
                #[cfg(windows)]
                if let Some(v) = self
                    .tabs
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.view.as_ref())
                {
                    debug_log(&format!("tab {id} audio={}", win::status(v).2));
                }
                let _ = id;
                self.sync();
            }
            UserEvent::NavError(id, msg) => {
                // Ein Download-Link „scheitert“ als Navigation, und das meldet die Engine
                // noch vor dem Download-Start. Darum kurz warten, ob ein Download folgt.
                let p = self.proxy.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(700));
                    let _ = p.send_event(UserEvent::ShowError(id, msg));
                });
            }
            UserEvent::ShowError(id, msg) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    // Download gestartet oder schon die nächste Seite unterwegs → keine Fehlerseite
                    if t.loading
                        || t.dl_at
                            .is_some_and(|at| at.elapsed() < Duration::from_secs(10))
                    {
                        return true;
                    }
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
            UserEvent::DlStart {
                dl,
                tab,
                name,
                total,
            } => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) {
                    t.dl_at = Some(Instant::now());
                    t.loading = false;
                }
                self.downloads.push(Download {
                    dl,
                    name,
                    got: 0,
                    total,
                    done: None,
                    at: Instant::now(),
                });
                self.sync();
            }
            UserEvent::DlProgress { dl, got, total } => {
                if let Some(d) = self.downloads.iter_mut().find(|d| d.dl == dl) {
                    d.got = got;
                    d.total = total;
                }
                self.sync();
            }
            UserEvent::DlDone { dl, ok } => {
                if let Some(d) = self.downloads.iter_mut().find(|d| d.dl == dl) {
                    d.done = Some(ok);
                    if ok {
                        d.got = d.total.max(d.got);
                    }
                    d.at = Instant::now();
                }
                self.sync();
            }
            UserEvent::Zoom(id, z) => {
                // Strg+Mausrad: merken wie über den Regler. Während des Ladens nicht –
                // da meldet die Engine ihren Startwert 100 %, der sonst die Einstellung löscht.
                if self.cur().is_some_and(|t| t.id == id && !t.loading) {
                    if let Some(host) = self.cur().map(|t| host_of(&t.url).to_string()) {
                        if (z - 1.0).abs() < 0.001 {
                            self.settings.zoom.remove(&host);
                        } else if !host.is_empty() {
                            self.settings.zoom.insert(host, z);
                        }
                        store::save_settings(&self.dir, &self.settings);
                    }
                    self.sync();
                }
            }
            UserEvent::Crashed(id, browser) => {
                debug_log(&format!("crashed tab {id} browser={browser}"));
                if browser {
                    // Ganze Engine weg (alle Tabs + Leiste) → sauber neu starten;
                    // die Sitzung ist gespeichert, Tabs kommen sofort zurück.
                    self.restart();
                    return false;
                }
                let active = self.cur().map(|t| t.id) == Some(id);
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    if active {
                        #[cfg(windows)]
                        if let Some(v) = &t.view {
                            win::reload(v);
                        }
                    } else {
                        t.view = None; // lädt beim nächsten Anklicken neu
                    }
                }
                self.sync();
            }
            UserEvent::CookiesSaved => {}
            UserEvent::External(id) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    t.dl_at = Some(Instant::now());
                }
            }
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
                if std::env::var_os("LEAF_DEBUG").is_none() {
                    self.window.set_focus();
                }
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
        // Pfad-Regeln: nur Werbe-Pfade, nicht die ganze Seite
        assert!(adblock::is_blocked(
            "https://www.youtube.com/pagead/viewthroughconversion/1"
        ));
        assert!(adblock::is_blocked(
            "https://www.youtube.com/api/stats/ads?ver=2"
        ));
        assert!(!adblock::is_blocked(
            "https://www.youtube.com/api/stats/watchtime"
        ));
        assert!(!adblock::is_blocked("https://notyoutube.com/pagead/x"));
        // SoundCloud-Audiowerbung läuft über AdsWizz
        assert!(adblock::is_blocked(
            "https://synchrobox.adswizz.com/register2.php"
        ));
        assert!(!adblock::is_blocked("https://soundcloud.com/discover"));
        assert!(!adblock::is_blocked("https://i1.sndcdn.com/a.jpg"));
        assert!(adblock::filters().len() > 3000);
        // geblockte Werbe-Skripte bekommen Attrappen statt eines Fehlers
        assert!(
            adblock::script_stub("https://securepubads.g.doubleclick.net/tag/js/gpt.js")
                .contains("googletag")
        );
        assert!(
            adblock::script_stub("https://www.googletagmanager.com/gtag/js?id=G-1")
                .contains("eventCallback")
        );
        assert_eq!(adblock::script_stub("https://ads.example/x.js"), "");
        adblock::set_enabled(false);
        assert!(!adblock::is_blocked("https://doubleclick.net/"));
    }

    #[test]
    fn hosts() {
        assert_eq!(host_of("https://www.youtube.com/watch?v=1"), "youtube.com");
        assert_eq!(host_of("http://localhost:3000/x"), "localhost");
        assert_eq!(host_of(""), "");
    }
}

const RESTART_ARG: &str = "--restart";

/// Port für die Link-Übergabe; ein Testprofil (LEAF_PROFILE) nutzt einen eigenen,
/// damit es nie mit dem normal laufenden Leaf spricht.
fn port() -> u16 {
    match std::env::var("LEAF_PROFILE") {
        Ok(p) if !p.is_empty() => PORT + 1,
        _ => PORT,
    }
}

/// Host einer URL ohne "www." (für Zoom pro Seite, Tab-Farbe)
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or("");
    let host = rest.split(['/', '?', '#', ':']).next().unwrap_or("");
    host.strip_prefix("www.").unwrap_or(host)
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    // LEAF_PROFILE=<Name> nutzt ein getrenntes Profil (zum Testen, ohne echte Daten)
    let d = match std::env::var("LEAF_PROFILE") {
        Ok(p) if !p.is_empty() => base.join(format!("Leaf-{p}")),
        _ => base.join("Leaf"),
    };
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Wird gerade beendet → keine Links mehr von anderen Instanzen annehmen.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Läuft Leaf schon? Dann URL an die laufende Instanz übergeben.
/// Nur wenn sie mit "1" bestätigt, gilt die Übergabe als erledigt.
fn hand_off(arg: &str) -> bool {
    let addr = ([127, 0, 0, 1], port()).into();
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
    let Ok(l) = TcpListener::bind(("127.0.0.1", port())) else {
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
    let mut arg = std::env::args().nth(1).unwrap_or_default();
    if arg == RESTART_ARG {
        // Neustart nach Absturz: der alte Prozess gibt gerade noch Port & Profil frei
        std::thread::sleep(Duration::from_millis(1500));
        arg.clear();
    } else if hand_off(&arg) {
        return Ok(());
    }

    let dir = data_dir();
    // Falls doch einmal etwas schiefgeht: Ursache festhalten statt still zu verschwinden
    let log = dir.join("crash.log");
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(f, "[{t}] {info}");
        }
    }));
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
        downloads: Vec::new(),
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
                    debug_log("close requested");
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
