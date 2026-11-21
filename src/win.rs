//! Windows-spezifisch über die WebView2-COM-API: Werbeblocker-Hook, Navigation,
//! Popups, Downloads, externe Apps, Zoom, Ton, Favicons, Tab-Pausierung,
//! Absturz-Erkennung, Session-Cookies – dazu Fenster-Stil und Hintergrundbild.
use crate::UserEvent;
use std::sync::atomic::Ordering;
use tao::event_loop::EventLoopProxy;
use webview2_com::{
    take_pwstr, BytesReceivedChangedEventHandler, ClearBrowsingDataCompletedHandler,
    ContainsFullScreenElementChangedEventHandler, DownloadStartingEventHandler,
    FaviconChangedEventHandler, GetCookiesCompletedHandler, HistoryChangedEventHandler,
    IsDocumentPlayingAudioChangedEventHandler, LaunchingExternalUriSchemeEventHandler,
    Microsoft::Web::WebView2::Win32::*, NavigationCompletedEventHandler,
    NewWindowRequestedEventHandler, ProcessFailedEventHandler, StateChangedEventHandler,
    TrySuspendCompletedHandler, WebResourceRequestedEventHandler, ZoomFactorChangedEventHandler,
};
use windows::{
    core::{w, Interface, PWSTR},
    Win32::{
        Foundation::{BOOL, HWND},
        Graphics::Dwm::*,
        System::WinRT::EventRegistrationToken,
    },
};
use wry::{WebView, WebViewExtWindows};

fn core(view: &WebView) -> Option<ICoreWebView2> {
    unsafe { view.controller().CoreWebView2().ok() }
}

/// Aktuelles Desktop-Hintergrundbild (für den Glas-Effekt der Oberfläche).
/// Funktioniert unabhängig von der Windows-Einstellung „Transparenzeffekte“.
pub fn wallpaper() -> Option<std::path::PathBuf> {
    let t = std::path::PathBuf::from(std::env::var_os("APPDATA")?)
        .join("Microsoft\\Windows\\Themes\\TranscodedWallpaper");
    if t.is_file() {
        return Some(t);
    }
    use windows::Win32::{Foundation::ERROR_SUCCESS, System::Registry::*};
    unsafe {
        let mut buf = [0u16; 520];
        let mut size = (buf.len() * 2) as u32;
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Control Panel\\Desktop"),
            w!("WallPaper"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as _),
            Some(&mut size),
        );
        if r != ERROR_SUCCESS {
            return None;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        let p = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]));
        p.is_file().then_some(p)
    }
}

/// Runde Win11-Ecken + dezenter, zum Theme passender Fensterrand.
pub fn style_window(hwnd: isize, dark: bool) {
    let hwnd = HWND(hwnd as _);
    unsafe {
        let set = |attr: DWMWINDOWATTRIBUTE, val: i32| {
            let _ = DwmSetWindowAttribute(hwnd, attr, &val as *const i32 as _, 4);
        };
        set(DWMWA_USE_IMMERSIVE_DARK_MODE, dark as i32);
        set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND.0);
        set(
            DWMWA_BORDER_COLOR,
            if dark {
                0x00403A38u32 as i32
            } else {
                0x00D0D0D0
            },
        );
    }
}

/// Werbeblocker, Vollbild, Ton, Favicon, Verlauf-Events + Komfort-Einstellungen.
pub fn attach(view: &WebView, proxy: EventLoopProxy<UserEvent>, id: u32) {
    let Some(wv) = core(view) else { return };
    unsafe {
        let mut token = EventRegistrationToken::default();

        if let Ok(st) = wv.Settings() {
            // keine Edge-Fehlerseite – Leaf zeigt eine eigene
            let _ = st.SetIsBuiltInErrorPageEnabled(false);
            if let Ok(s4) = st.cast::<ICoreWebView2Settings4>() {
                let _ = s4.SetIsPasswordAutosaveEnabled(true);
                let _ = s4.SetIsGeneralAutofillEnabled(true);
            }
        }
        // Testmodus: Seiten stumm, damit beim Testen nichts aus den Lautsprechern kommt
        if std::env::var_os("LEAF_DEBUG").is_some() {
            if let Ok(w8) = wv.cast::<ICoreWebView2_8>() {
                let _ = w8.SetIsMuted(true);
            }
        }

        if let Ok(env) = wv.cast::<ICoreWebView2_2>().and_then(|w| w.Environment()) {
            let p = proxy.clone();
            // Nur Anfragen an bekannte Werbe-Hosts werden gemeldet; alles andere lädt
            // ohne Umweg über Leaf (früher lief jede Anfrage durch den UI-Thread).
            let started = std::time::Instant::now();
            let mut any = false;
            for f in crate::adblock::filters() {
                any |= wv
                    .AddWebResourceRequestedFilter(
                        &windows::core::HSTRING::from(f),
                        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    )
                    .is_ok();
            }
            crate::debug_log(&format!("adblock filters: {:?}", started.elapsed()));
            if any {
                let _ = wv.add_WebResourceRequested(
                    &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else { return Ok(()) };
                        let mut uri = PWSTR::null();
                        args.Request()?.Uri(&mut uri)?;
                        if crate::adblock::is_blocked(&take_pwstr(uri)) {
                            let resp =
                                env.CreateWebResourceResponse(None, 403, w!("Blocked"), w!(""))?;
                            args.SetResponse(&resp)?;
                            let _ = p.send_event(UserEvent::Blocked(id));
                        }
                        Ok(())
                    })),
                    &mut token,
                );
            }
        }

        let p = proxy.clone();
        let _ = wv.add_ContainsFullScreenElementChanged(
            &ContainsFullScreenElementChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(s) = sender {
                    let mut fs = BOOL::default();
                    s.ContainsFullScreenElement(&mut fs)?;
                    let _ = p.send_event(UserEvent::FullScreen(id, fs.as_bool()));
                }
                Ok(())
            })),
            &mut token,
        );

        let p = proxy.clone();
        let _ = wv.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut ok = BOOL::default();
                args.IsSuccess(&mut ok)?;
                if ok.as_bool() {
                    return Ok(());
                }
                // Hat der Server geantwortet (auch 403/404/500), zeigt die Seite ihren
                // eigenen Inhalt – keine Leaf-Fehlerseite darüberlegen.
                if let Ok(a2) = args.cast::<ICoreWebView2NavigationCompletedEventArgs2>() {
                    let mut code = 0i32;
                    if a2.HttpStatusCode(&mut code).is_ok() && code > 0 {
                        return Ok(());
                    }
                }
                let mut st = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                args.WebErrorStatus(&mut st)?;
                // Nur echte Netzwerkfehler – abgebrochene Navigationen, Downloads,
                // Weiterleitungen usw. (Status UNKNOWN) bleiben unangetastet.
                let msg = match st {
                    COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED => "Diese Adresse gibt es nicht – vielleicht ein Tippfehler?",
                    COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED => "Keine Internetverbindung.",
                    COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT => "Die Seite antwortet nicht (Zeitüberschreitung).",
                    COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT
                    | COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE
                    | COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET
                    | COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED => "Der Server ist gerade nicht erreichbar.",
                    s if (1..=5).contains(&s.0) => "Das Sicherheitszertifikat dieser Seite ist ungültig – Leaf hat sie zu deinem Schutz blockiert.",
                    _ => return Ok(()),
                };
                let _ = p.send_event(UserEvent::NavError(id, msg.into()));
                Ok(())
            })),
            &mut token,
        );

        let p = proxy.clone();
        let _ = wv.add_HistoryChanged(
            &HistoryChangedEventHandler::create(Box::new(move |_, _| {
                let _ = p.send_event(UserEvent::Nav(id));
                Ok(())
            })),
            &mut token,
        );

        if let Ok(w8) = wv.cast::<ICoreWebView2_8>() {
            let p = proxy.clone();
            let _ = w8.add_IsDocumentPlayingAudioChanged(
                &IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |_, _| {
                    let _ = p.send_event(UserEvent::Nav(id));
                    Ok(())
                })),
                &mut token,
            );
        }

        if let Ok(w15) = wv.cast::<ICoreWebView2_15>() {
            let p = proxy.clone();
            let _ = w15.add_FaviconChanged(
                &FaviconChangedEventHandler::create(Box::new(move |sender, _| {
                    if let Some(w15) = sender.and_then(|s| s.cast::<ICoreWebView2_15>().ok()) {
                        let mut uri = PWSTR::null();
                        w15.FaviconUri(&mut uri)?;
                        let _ = p.send_event(UserEvent::Favicon(id, take_pwstr(uri)));
                    }
                    Ok(())
                })),
                &mut token,
            );
        }

        if let Ok(w9) = wv.cast::<ICoreWebView2_9>() {
            let _ = w9.SetDefaultDownloadDialogCornerAlignment(
                COREWEBVIEW2_DEFAULT_DOWNLOAD_DIALOG_CORNER_ALIGNMENT_TOP_RIGHT,
            );
        }

        // Neue Fenster: echte Popups (Login mit Google/Steam/Discord …) bekommen ein
        // eigenes Fenster, damit window.opener funktioniert. Normale Links mit
        // target=_blank öffnen einen Tab. Ungefragte Popups (Werbung) werden geblockt.
        let p = proxy.clone();
        let _ = wv.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut user = BOOL::default();
                args.IsUserInitiated(&mut user)?;
                if !user.as_bool() && crate::adblock::enabled() {
                    return args.SetHandled(true);
                }
                let mut sized = BOOL::default();
                if let Ok(f) = args.WindowFeatures() {
                    let _ = f.HasSize(&mut sized);
                }
                if sized.as_bool() {
                    return Ok(()); // WebView2 öffnet ein Popup-Fenster mit Opener
                }
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                args.SetHandled(true)?;
                let _ = p.send_event(UserEvent::NewTab(take_pwstr(uri)));
                Ok(())
            })),
            &mut token,
        );

        // Downloads: Fortschritt an die Leiste melden und das Download-Panel öffnen.
        if let Ok(w4) = wv.cast::<ICoreWebView2_4>() {
            let p = proxy.clone();
            let _ = w4.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(move |sender, args| {
                    let Some(args) = args else { return Ok(()) };
                    let op = args.DownloadOperation()?;
                    let mut path = PWSTR::null();
                    args.ResultFilePath(&mut path)?;
                    let path = take_pwstr(path);
                    let name = std::path::Path::new(&path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Download".into());
                    let mut total = 0i64;
                    let _ = op.TotalBytesToReceive(&mut total);
                    let dl = NEXT_DOWNLOAD.fetch_add(1, Ordering::Relaxed);
                    let _ = p.send_event(UserEvent::DlStart {
                        dl,
                        tab: id,
                        name,
                        total,
                    });
                    let mut tok = EventRegistrationToken::default();
                    let p2 = p.clone();
                    let mut last = (std::time::Instant::now(), -1i64);
                    let _ = op.add_BytesReceivedChanged(
                        &BytesReceivedChangedEventHandler::create(Box::new(move |op, _| {
                            if let Some(op) = op {
                                let (mut got, mut total) = (0i64, 0i64);
                                op.BytesReceived(&mut got)?;
                                let _ = op.TotalBytesToReceive(&mut total);
                                let pct = if total > 0 { got * 100 / total } else { -1 };
                                if pct != last.1 && last.0.elapsed().as_millis() > 250 {
                                    last = (std::time::Instant::now(), pct);
                                    let _ = p2.send_event(UserEvent::DlProgress { dl, got, total });
                                }
                            }
                            Ok(())
                        })),
                        &mut tok,
                    );
                    let p3 = p.clone();
                    let _ = op.add_StateChanged(
                        &StateChangedEventHandler::create(Box::new(move |op, _| {
                            if let Some(op) = op {
                                let mut st = COREWEBVIEW2_DOWNLOAD_STATE::default();
                                op.State(&mut st)?;
                                if st != COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS {
                                    let ok = st == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED;
                                    let _ = p3.send_event(UserEvent::DlDone { dl, ok });
                                }
                            }
                            Ok(())
                        })),
                        &mut tok,
                    );
                    if let Some(w9) = sender.and_then(|s| s.cast::<ICoreWebView2_9>().ok()) {
                        let _ = w9.OpenDefaultDownloadDialog();
                    }
                    Ok(())
                })),
                &mut token,
            );
        }

        // Externe Apps: steam://, discord:// usw. direkt öffnen (z. B. CS2-Server über
        // xplay.gg beitreten, Spiele starten). Andere Schemata fragen wie gewohnt nach.
        if let Ok(w18) = wv.cast::<ICoreWebView2_18>() {
            let _ = w18.add_LaunchingExternalUriScheme(
                &LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    let uri = take_pwstr(uri);
                    if trusted_app_scheme(&uri) {
                        args.SetCancel(true)?;
                        open_external(&uri);
                    }
                    Ok(())
                })),
                &mut token,
            );
        }

        // Zoom (Strg+Mausrad) an die Leiste melden
        let p = proxy.clone();
        let _ = view.controller().add_ZoomFactorChanged(
            &ZoomFactorChangedEventHandler::create(Box::new(move |c, _| {
                if let Some(c) = c {
                    let mut z = 1.0f64;
                    c.ZoomFactor(&mut z)?;
                    let _ = p.send_event(UserEvent::Zoom(id, z));
                }
                Ok(())
            })),
            &mut token,
        );

        // Abgestürzte Seiten-/Browser-Prozesse erkennen statt hängen zu bleiben
        let p = proxy.clone();
        let _ = wv.add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                args.ProcessFailedKind(&mut kind)?;
                let browser = kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED;
                if browser
                    || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
                    || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
                {
                    let _ = p.send_event(UserEvent::Crashed(id, browser));
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

static NEXT_DOWNLOAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Schemata, die ohne Rückfrage an die zugehörige App gehen dürfen.
fn trusted_app_scheme(uri: &str) -> bool {
    const OK: &[&str] = &[
        "steam",
        "discord",
        "spotify",
        "com.epicgames.launcher",
        "battlenet",
        "uplay",
        "origin",
        "origin2",
        "eadesktop",
        "roblox-player",
        "roblox",
        "zoommtg",
        "msteams",
        "slack",
        "tg",
        "whatsapp",
        "mailto",
        "tel",
        "riotclient",
        "goggalaxy",
        "obsidian",
    ];
    uri.split_once(':')
        .is_some_and(|(s, _)| OK.contains(&s.to_ascii_lowercase().as_str()))
}

fn open_external(uri: &str) {
    use windows::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &windows::core::HSTRING::from(uri),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_schemes() {
        assert!(super::trusted_app_scheme("steam://connect/1.2.3.4:27015"));
        assert!(super::trusted_app_scheme("steam://run/730"));
        assert!(super::trusted_app_scheme("Discord://-/channels/1"));
        assert!(!super::trusted_app_scheme("ms-msdt:/id PCWDiagnostic"));
        assert!(!super::trusted_app_scheme("search-ms:query=x"));
        assert!(!super::trusted_app_scheme("file:///C:/x.exe"));
    }
}

/// Zoom der Seite (1.0 = 100 %)
pub fn zoom(view: &WebView) -> f64 {
    let mut z = 1.0;
    unsafe {
        let _ = view.controller().ZoomFactor(&mut z);
    }
    z
}

pub fn back(view: &WebView) {
    if let Some(w) = core(view) {
        unsafe {
            let _ = w.GoBack();
        }
    }
}
pub fn forward(view: &WebView) {
    if let Some(w) = core(view) {
        unsafe {
            let _ = w.GoForward();
        }
    }
}
pub fn reload(view: &WebView) {
    if let Some(w) = core(view) {
        unsafe {
            let _ = w.Reload();
        }
    }
}
pub fn stop(view: &WebView) {
    if let Some(w) = core(view) {
        unsafe {
            let _ = w.Stop();
        }
    }
}

/// (kann zurück, kann vor, spielt Ton, stumm)
pub fn status(view: &WebView) -> (bool, bool, bool, bool) {
    let Some(w) = core(view) else {
        return (false, false, false, false);
    };
    unsafe {
        let (mut b, mut f, mut a, mut m) = (
            BOOL::default(),
            BOOL::default(),
            BOOL::default(),
            BOOL::default(),
        );
        let _ = w.CanGoBack(&mut b);
        let _ = w.CanGoForward(&mut f);
        if let Ok(w8) = w.cast::<ICoreWebView2_8>() {
            let _ = w8.IsDocumentPlayingAudio(&mut a);
            let _ = w8.IsMuted(&mut m);
        }
        (b.as_bool(), f.as_bool(), a.as_bool(), m.as_bool())
    }
}

pub fn toggle_mute(view: &WebView) {
    let Some(w) = core(view) else { return };
    unsafe {
        if let Ok(w8) = w.cast::<ICoreWebView2_8>() {
            let mut m = BOOL::default();
            let _ = w8.IsMuted(&mut m);
            let _ = w8.SetIsMuted(!m.as_bool());
        }
    }
}

/// Cookies, Cache, Passwörter, Formulardaten – alles der Leaf-Profil-Daten.
pub fn clear_browsing_data(view: &WebView) {
    let Some(w) = core(view) else { return };
    unsafe {
        if let Ok(p2) = w
            .cast::<ICoreWebView2_13>()
            .and_then(|w| w.Profile())
            .and_then(|p| p.cast::<ICoreWebView2Profile2>())
        {
            let _ = p2.ClearBrowsingDataAll(&ClearBrowsingDataCompletedHandler::create(Box::new(
                |_| Ok(()),
            )));
        }
    }
}

/// Versteckten Tab einfrieren: JS-Timer stoppen, RAM-Ziel senken.
pub fn suspend(view: &WebView) {
    let Some(wv) = core(view) else { return };
    unsafe {
        if let Ok(w19) = wv.cast::<ICoreWebView2_19>() {
            let _ = w19.SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW);
        }
        if let Ok(w3) = wv.cast::<ICoreWebView2_3>() {
            let _ = w3.TrySuspend(&TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(()))));
        }
    }
}

pub fn resume(view: &WebView) {
    let Some(wv) = core(view) else { return };
    unsafe {
        if let Ok(w19) = wv.cast::<ICoreWebView2_19>() {
            let _ = w19.SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL);
        }
        if let Ok(w3) = wv.cast::<ICoreWebView2_3>() {
            let _ = w3.Resume();
        }
    }
}

// ── Angemeldet bleiben ──────────────────────────────────────────────────────
// Dauerhafte Cookies speichert WebView2 selbst. Session-Cookies (Logins ohne
// Ablaufdatum) würden beim Beenden verloren gehen – Chrome behält sie bei
// „Tabs wiederherstellen“. Leaf macht es genauso: beim Beenden (und alle 5 min)
// sichern, beim Start zurückspielen. Verschlüsselt mit Windows-DPAPI, d. h. nur
// dein Windows-Konto auf diesem PC kann die Datei lesen.

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedCookie {
    n: String,
    v: String,
    d: String,
    p: String,
    secure: bool,
    http: bool,
    same: i32,
}

fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    use windows::Win32::{
        Foundation::{LocalFree, HLOCAL},
        Security::Cryptography::*,
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        let r = if protect {
            CryptProtectData(
                &input,
                w!("Leaf"),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        } else {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        r.ok()?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(out.pbData as _));
        Some(v)
    }
}

fn cookie_manager(view: &WebView) -> Option<ICoreWebView2CookieManager> {
    let wv = core(view)?;
    unsafe {
        wv.cast::<ICoreWebView2_2>()
            .and_then(|w| w.CookieManager())
            .ok()
    }
}

/// Session-Cookies sichern; meldet sich danach mit `UserEvent::CookiesSaved`.
pub fn save_session_cookies(
    view: &WebView,
    path: std::path::PathBuf,
    proxy: EventLoopProxy<UserEvent>,
) {
    let Some(mgr) = cookie_manager(view) else {
        let _ = proxy.send_event(UserEvent::CookiesSaved);
        return;
    };
    let p2 = proxy.clone();
    let handler = GetCookiesCompletedHandler::create(Box::new(move |_, list| unsafe {
        let mut out = Vec::new();
        if let Some(list) = list {
            let mut n = 0u32;
            list.Count(&mut n)?;
            for i in 0..n {
                let c = list.GetValueAtIndex(i)?;
                let mut session = BOOL::default();
                c.IsSession(&mut session)?;
                if !session.as_bool() {
                    continue;
                }
                let s = |f: &dyn Fn(*mut PWSTR) -> windows::core::Result<()>| {
                    let mut p = PWSTR::null();
                    f(&mut p).map(|_| take_pwstr(p))
                };
                let (mut secure, mut http) = (BOOL::default(), BOOL::default());
                let mut same = COREWEBVIEW2_COOKIE_SAME_SITE_KIND::default();
                c.IsSecure(&mut secure)?;
                c.IsHttpOnly(&mut http)?;
                c.SameSite(&mut same)?;
                out.push(SavedCookie {
                    n: s(&|p| c.Name(p))?,
                    v: s(&|p| c.Value(p))?,
                    d: s(&|p| c.Domain(p))?,
                    p: s(&|p| c.Path(p))?,
                    secure: secure.as_bool(),
                    http: http.as_bool(),
                    same: same.0,
                });
            }
        }
        if let Some(enc) = serde_json::to_vec(&out).ok().and_then(|j| dpapi(&j, true)) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, enc).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
        let _ = proxy.send_event(UserEvent::CookiesSaved);
        Ok(())
    }));
    if unsafe { mgr.GetCookies(w!(""), &handler) }.is_err() {
        let _ = p2.send_event(UserEvent::CookiesSaved);
    }
}

/// Gesicherte Session-Cookies beim Start zurückspielen (vor dem ersten Tab).
pub fn restore_session_cookies(view: &WebView, path: &std::path::Path) {
    let Some(mgr) = cookie_manager(view) else {
        return;
    };
    let Some(list) = std::fs::read(path)
        .ok()
        .and_then(|b| dpapi(&b, false))
        .and_then(|j| serde_json::from_slice::<Vec<SavedCookie>>(&j).ok())
    else {
        return;
    };
    for c in list {
        unsafe {
            let Ok(ck) = mgr.CreateCookie(
                &windows::core::HSTRING::from(c.n.as_str()),
                &windows::core::HSTRING::from(c.v.as_str()),
                &windows::core::HSTRING::from(c.d.as_str()),
                &windows::core::HSTRING::from(c.p.as_str()),
            ) else {
                continue;
            };
            let _ = ck.SetIsSecure(BOOL::from(c.secure));
            let _ = ck.SetIsHttpOnly(BOOL::from(c.http));
            let _ = ck.SetSameSite(COREWEBVIEW2_COOKIE_SAME_SITE_KIND(c.same));
            let _ = mgr.AddOrUpdateCookie(&ck);
        }
    }
}

pub fn open_downloads(view: &WebView) {
    if let Some(wv) = core(view) {
        unsafe {
            if let Ok(w9) = wv.cast::<ICoreWebView2_9>() {
                let _ = w9.OpenDefaultDownloadDialog();
            }
        }
    }
}
