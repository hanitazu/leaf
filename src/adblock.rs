//! Domain-basierter Werbeblocker: O(Labels) HashSet-Lookup pro Request.
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static LIST: OnceLock<HashSet<String>> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Kosmetische Filter + YouTube-Werbung überspringen. Läuft nur im Hauptframe.
pub const COSMETIC_JS: &str = r#"(function(){if(window.top!==window)return;
var css='ins.adsbygoogle,.adsbygoogle,[id^="google_ads_iframe"],[id^="div-gpt-ad"],.trc_related_container,.OUTBRAIN{display:none!important}';
var h=location.hostname;
if(/(^|\.)youtube\.com$/.test(h)){
 css+='ytd-ad-slot-renderer,ytd-in-feed-ad-layout-renderer,ytd-promoted-sparkles-web-renderer,ytd-display-ad-renderer,ytd-companion-slot-renderer,ytd-action-companion-ad-renderer,ytd-banner-promo-renderer,#masthead-ad,#player-ads,.ytp-ad-overlay-container,ytd-rich-item-renderer:has(ytd-ad-slot-renderer){display:none!important}';
 setInterval(function(){var p=document.querySelector('.ad-showing');if(!p)return;
  var v=p.querySelector('video');if(v&&isFinite(v.duration)){v.muted=true;v.currentTime=v.duration;}
  var b=document.querySelector('.ytp-ad-skip-button,.ytp-ad-skip-button-modern,.ytp-skip-ad-button');if(b)b.click();},400);
}
var s=document.createElement('style');s.textContent=css;
(document.head||document.documentElement).appendChild(s);})();"#;

pub fn init(data_dir: &Path, enabled: bool) {
    let mut set: HashSet<String> = HashSet::new();
    let builtin = include_str!("blocklist.txt");
    let user = std::fs::read_to_string(data_dir.join("blocklist.txt")).unwrap_or_default();
    for line in builtin.lines().chain(user.lines()) {
        let l = line.trim();
        if !l.is_empty() && !l.starts_with('#') {
            set.insert(l.to_ascii_lowercase());
        }
    }
    let _ = LIST.set(set);
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = &rest[..end];
    let auth = auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth);
    Some(auth.split(':').next().unwrap_or(auth))
}

pub fn is_blocked(url: &str) -> bool {
    if !enabled() || !url.starts_with("http") {
        return false;
    }
    let (Some(list), Some(h)) = (LIST.get(), host(url)) else {
        return false;
    };
    let h = h.to_ascii_lowercase();
    let mut d = h.as_str();
    loop {
        if list.contains(d) {
            return true;
        }
        match d.find('.') {
            Some(i) => d = &d[i + 1..],
            None => return false,
        }
    }
}
