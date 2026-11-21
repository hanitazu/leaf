//! Werbeblocker: Domain-Liste (HashSet, O(Labels) pro Request) + Pfad-Regeln,
//! dazu Seiten-Skripte gegen YouTube- und SoundCloud-Werbung.
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

struct Lists {
    domains: HashSet<String>,
    /// (Host, Pfad-Anfang)
    paths: Vec<(String, String)>,
}

static LISTS: OnceLock<Lists> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Läuft in jeder Seite, bevor deren eigene Skripte starten (nur Hauptframe).
/// - entfernt Werbe-Felder aus YouTube-/SoundCloud-Antworten, bevor der Player sie sieht
/// - überspringt trotzdem auftauchende YouTube-Werbung (stumm, 16×, „Überspringen“ klicken)
/// - blendet übliche Werbeflächen aus
pub const PAGE_JS: &str = r#"(function(){if(window.top!==window)return;
var h=location.hostname,yt=/(^|\.)youtube\.com$/.test(h),sc=/(^|\.)soundcloud\.com$/.test(h);
if(yt||sc){
 var KEYS=yt?['adPlacements','adSlots','playerAds','adBreakHeartbeatParams']:['audio_ads','ads'];
 var prune=function(o,d){try{if(!o||typeof o!=='object'||d>3)return o;
  for(var i=0;i<KEYS.length;i++){if(KEYS[i] in o)delete o[KEYS[i]];}
  if(o.playerResponse)prune(o.playerResponse,d+1);if(o.data)prune(o.data,d+1);
  if(Array.isArray(o))for(var j=0;j<o.length&&j<50;j++)prune(o[j],d+1);}catch(e){}return o;};
 var jp=JSON.parse;JSON.parse=function(){return prune(jp.apply(this,arguments),0);};
 var rj=Response.prototype.json;Response.prototype.json=function(){return rj.apply(this,arguments).then(function(o){return prune(o,0);});};
 if(yt){var ipr;try{Object.defineProperty(window,'ytInitialPlayerResponse',{configurable:true,get:function(){return ipr;},set:function(v){ipr=prune(v,0);}});}catch(e){}}
}
var css='ins.adsbygoogle,.adsbygoogle,[id^="google_ads_iframe"],[id^="div-gpt-ad"],.trc_related_container,.OUTBRAIN{display:none!important}';
if(yt){
 css+='ytd-ad-slot-renderer,ytd-in-feed-ad-layout-renderer,ytd-promoted-sparkles-web-renderer,ytd-display-ad-renderer,ytd-companion-slot-renderer,ytd-action-companion-ad-renderer,ytd-banner-promo-renderer,ytd-statement-banner-renderer,ytd-player-legacy-desktop-watch-ads-renderer,#masthead-ad,#player-ads,.ytp-ad-overlay-container,.ytp-ad-overlay-slot,ytd-rich-item-renderer:has(ytd-ad-slot-renderer),ytd-reel-video-renderer:has(.ytd-ad-slot-renderer){display:none!important}';
 setInterval(function(){
  var b=document.querySelector('.ytp-ad-skip-button,.ytp-ad-skip-button-modern,.ytp-skip-ad-button,[class*="ytp-ad-skip"],[id^="skip-button"] button');if(b)b.click();
  var p=document.querySelector('.html5-video-player.ad-showing,.html5-video-player.ad-interrupting');if(!p)return;
  var v=p.querySelector('video');if(v){v.muted=true;v.playbackRate=16;if(isFinite(v.duration)&&v.duration>0)v.currentTime=v.duration;}
 },300);
}
var s=document.createElement('style');s.textContent=css;
(document.head||document.documentElement).appendChild(s);})();"#;

pub fn init(data_dir: &Path, enabled: bool) {
    let mut domains = HashSet::new();
    let mut paths = Vec::new();
    let builtin = include_str!("blocklist.txt");
    let user = std::fs::read_to_string(data_dir.join("blocklist.txt")).unwrap_or_default();
    for line in builtin.lines().chain(user.lines()) {
        let l = line.trim().to_ascii_lowercase();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        match l.split_once('/') {
            Some((host, path)) => paths.push((host.to_string(), format!("/{path}"))),
            None => {
                domains.insert(l);
            }
        }
    }
    let _ = LISTS.set(Lists { domains, paths });
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// URL-Muster für WebView2: Nur passende Anfragen werden überhaupt an Leaf
/// gemeldet – alle anderen laufen ohne Umweg über den UI-Thread durch.
pub fn filters() -> Vec<String> {
    let Some(l) = LISTS.get() else {
        return Vec::new();
    };
    l.domains
        .iter()
        .map(|d| format!("*{d}/*"))
        .chain(l.paths.iter().map(|(h, p)| format!("*{h}{p}*")))
        .collect()
}

/// (Host, Pfad inkl. Query) einer URL
fn split(url: &str) -> Option<(&str, &str)> {
    let rest = url.split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (auth, path) = rest.split_at(end);
    let auth = auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth);
    Some((auth.split(':').next().unwrap_or(auth), path))
}

fn host_matches(host: &str, rule: &str) -> bool {
    host == rule
        || (host.len() > rule.len()
            && host.ends_with(rule)
            && host.as_bytes()[host.len() - rule.len() - 1] == b'.')
}

pub fn is_blocked(url: &str) -> bool {
    if !enabled() || !url.starts_with("http") {
        return false;
    }
    let (Some(l), Some((host, path))) = (LISTS.get(), split(url)) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let mut d = host.as_str();
    loop {
        if l.domains.contains(d) {
            return true;
        }
        match d.find('.') {
            Some(i) => d = &d[i + 1..],
            None => break,
        }
    }
    l.paths
        .iter()
        .any(|(h, p)| host_matches(&host, h) && path.starts_with(p.as_str()))
}
