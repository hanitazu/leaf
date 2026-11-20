//! Verlauf, Sitzung und Einstellungen als kleine JSON-Dateien in %LOCALAPPDATA%\Leaf.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_HISTORY: usize = 5000;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save<T: Serialize>(path: &Path, v: &T) {
    if let Ok(b) = serde_json::to_vec(v) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub n: u32,
    pub t: u64,
}

#[derive(Default)]
pub struct History {
    pub items: Vec<Visit>,
    path: PathBuf,
    dirty: bool,
}

/// Für Vergleiche: ohne Schema und "www."
pub fn bare(url: &str) -> &str {
    let u = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let u = u.strip_prefix("www.").unwrap_or(u);
    u.strip_suffix('/').unwrap_or(u)
}

impl History {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("history.json");
        History {
            items: load(&path),
            path,
            dirty: false,
        }
    }

    pub fn visit(&mut self, url: &str) {
        if !url.starts_with("http") {
            return;
        }
        if let Some(v) = self.items.iter_mut().find(|v| v.url == url) {
            v.n += 1;
            v.t = now();
        } else {
            self.items.push(Visit {
                url: url.into(),
                title: String::new(),
                n: 1,
                t: now(),
            });
            if self.items.len() > MAX_HISTORY {
                // ältesten, selten besuchten Eintrag verwerfen
                if let Some(i) =
                    (0..self.items.len()).min_by_key(|&i| (self.items[i].n, self.items[i].t))
                {
                    self.items.swap_remove(i);
                }
            }
        }
        self.dirty = true;
    }

    pub fn set_title(&mut self, url: &str, title: &str) {
        if let Some(v) = self.items.iter_mut().find(|v| v.url == url) {
            if v.title != title {
                v.title = title.into();
                self.dirty = true;
            }
        }
    }

    fn score(v: &Visit, t: u64) -> f64 {
        let age_days = (t.saturating_sub(v.t)) as f64 / 86400.0;
        v.n as f64 / (1.0 + age_days / 7.0)
    }

    pub fn search(&self, q: &str, limit: usize) -> Vec<&Visit> {
        let q = q.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let t = now();
        let mut hits: Vec<(f64, &Visit)> = self
            .items
            .iter()
            .filter_map(|v| {
                let u = bare(&v.url).to_lowercase();
                let boost = if u.starts_with(&q) {
                    4.0
                } else if u.contains(&q) || v.title.to_lowercase().contains(&q) {
                    1.0
                } else {
                    return None;
                };
                Some((Self::score(v, t) * boost, v))
            })
            .collect();
        hits.sort_by(|a, b| b.0.total_cmp(&a.0));
        hits.into_iter().take(limit).map(|(_, v)| v).collect()
    }

    /// Meistbesuchte Seiten – eine pro Domain.
    pub fn top(&self, limit: usize) -> Vec<&Visit> {
        let t = now();
        let is_search = |v: &&Visit| v.url.contains("google.") && v.url.contains("/search");
        let mut all: Vec<&Visit> = self.items.iter().filter(|v| !is_search(v)).collect();
        all.sort_by(|a, b| Self::score(b, t).total_cmp(&Self::score(a, t)));
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for v in all {
            let host = bare(&v.url).split('/').next().unwrap_or("").to_string();
            if !seen.contains(&host) {
                seen.push(host);
                out.push(v);
                if out.len() == limit {
                    break;
                }
            }
        }
        out
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.dirty = true;
    }

    pub fn flush(&mut self) {
        if self.dirty {
            save(&self.path, &self.items);
            self.dirty = false;
        }
    }
}

#[derive(Serialize, Deserialize, Default, PartialEq, Clone)]
#[serde(default)]
pub struct Session {
    pub tabs: Vec<String>,
    /// Tab-Titel, damit wiederhergestellte Tabs sofort richtig beschriftet sind
    pub titles: Vec<String>,
    pub active: usize,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Fav {
    pub url: String,
    pub title: String,
}

/// Favoriten – erscheinen auf der Startseite und zuerst in den Vorschlägen.
pub struct Favorites {
    pub items: Vec<Fav>,
    path: PathBuf,
}

impl Favorites {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("favorites.json");
        Favorites {
            items: load(&path),
            path,
        }
    }
    pub fn has(&self, url: &str) -> bool {
        !url.is_empty() && self.items.iter().any(|f| bare(&f.url) == bare(url))
    }
    pub fn toggle(&mut self, url: &str, title: &str) {
        if self.has(url) {
            self.remove(url);
        } else {
            self.items.push(Fav {
                url: url.into(),
                title: title.into(),
            });
            save(&self.path, &self.items);
        }
    }
    pub fn remove(&mut self, url: &str) {
        self.items.retain(|f| bare(&f.url) != bare(url));
        save(&self.path, &self.items);
    }
    pub fn set_title(&mut self, url: &str, title: &str) {
        if let Some(f) = self
            .items
            .iter_mut()
            .find(|f| f.url == url && f.title != title)
        {
            f.title = title.into();
            save(&self.path, &self.items);
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WinState {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub max: bool,
}

impl Default for WinState {
    fn default() -> Self {
        WinState {
            x: 0.0,
            y: 0.0,
            w: 1280.0,
            h: 820.0,
            max: false,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub adblock: bool,
    pub window: Option<WinState>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            adblock: true,
            window: None,
        }
    }
}

pub fn load_session(dir: &Path) -> Session {
    load(&dir.join("session.json"))
}
pub fn save_session(dir: &Path, s: &Session) {
    save(&dir.join("session.json"), s)
}
pub fn load_settings(dir: &Path) -> Settings {
    load(&dir.join("settings.json"))
}
pub fn save_settings(dir: &Path, s: &Settings) {
    save(&dir.join("settings.json"), s)
}
