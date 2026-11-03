use log::*;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

static FLATHUB_API: &str = "https://flathub.org/api/v2";
pub static FLATHUB_REPO: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";
/// Apps that are only on Flathub are keyed "flathub:<app ID>" wherever nixpkgs attributes are used
pub static FLATHUB_PREFIX: &str = "flathub:";

pub fn flathubkey(id: &str) -> String {
    format!("{}{}", FLATHUB_PREFIX, id)
}

pub fn flathubid(pkg: &str) -> Option<&str> {
    pkg.strip_prefix(FLATHUB_PREFIX)
}

/// An app on Flathub that matches a nixpkgs package
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlathubApp {
    pub id: String,
    pub version: Option<String>,
}

/// A Flatpak app installed on this system
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledFlatpak {
    pub name: String,
    pub version: Option<String>,
    /// "user" or "system"
    pub installation: String,
}

/// Details for the page of an app that's only on Flathub
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlathubDetails {
    pub id: String,
    pub name: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub screenshots: Vec<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub developer: Option<String>,
    pub version: Option<String>,
}

/// An app in the Flathub catalog
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogApp {
    pub id: String,
    pub name: String,
    pub summary: Option<String>,
    /// A URL; see [cacheicon]
    pub icon: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Installs last month, to rank search results
    #[serde(default)]
    pub installs: u64,
    /// The nixpkgs package for this app, if there is one. Worked out after loading, so not cached.
    #[serde(skip)]
    pub nixattr: Option<String>,
}

#[derive(Deserialize)]
struct AppStream {
    name: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    screenshots: Option<Vec<Screenshot>>,
    urls: Option<Urls>,
    project_license: Option<String>,
    developer_name: Option<String>,
    releases: Option<Vec<Release>>,
}

#[derive(Deserialize)]
struct Release {
    version: Option<String>,
}

#[derive(Deserialize)]
struct Screenshot {
    sizes: Vec<ScreenshotSize>,
}

#[derive(Deserialize)]
struct ScreenshotSize {
    src: String,
    width: String,
}

#[derive(Deserialize)]
struct Urls {
    homepage: Option<String>,
}

#[derive(Deserialize)]
struct SearchResults {
    hits: Vec<SearchHit>,
    #[serde(rename = "totalPages")]
    total_pages: u32,
}

#[derive(Deserialize)]
struct SearchHit {
    app_id: String,
    name: String,
    summary: Option<String>,
    icon: Option<String>,
    keywords: Option<Vec<String>>,
    installs_last_month: Option<u64>,
}

pub fn flatpakinstalled() -> bool {
    Command::new("flatpak").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Flatpak apps installed for the user and system-wide, by app ID
pub fn installedflatpaks() -> HashMap<String, InstalledFlatpak> {
    let mut out = HashMap::new();
    let output = match Command::new("flatpak")
        .args(["list", "--app", "--columns=application,name,version,installation"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return out,
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let cols = line.split('\t').collect::<Vec<_>>();
        if let [id, name, version, installation] = cols[..] {
            out.insert(
                id.to_string(),
                InstalledFlatpak {
                    name: name.to_string(),
                    version: if version.is_empty() { None } else { Some(version.to_string()) },
                    installation: installation.to_string(),
                },
            );
        }
    }
    out
}

/// The icon an installed Flatpak exports, if there is one
pub fn installedicon(id: &str, installation: &str) -> Option<String> {
    let exports = if installation == "user" {
        format!("{}/.local/share/flatpak/exports/share/icons/hicolor", std::env::var("HOME").ok()?)
    } else {
        "/var/lib/flatpak/exports/share/icons/hicolor".to_string()
    };
    ["128x128/apps/{}.png", "256x256/apps/{}.png", "64x64/apps/{}.png", "scalable/apps/{}.svg"]
        .iter()
        .map(|f| format!("{}/{}", exports, f.replace("{}", id)))
        .find(|p| Path::new(p).is_file())
}

async fn getappstream(id: &str) -> Option<AppStream> {
    let response = reqwest::get(format!("{}/appstream/{}", FLATHUB_API, id)).await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    serde_json::from_str::<AppStream>(&response.text().await.ok()?)
        .map_err(|e| warn!("Failed to parse Flathub data for {}: {}", id, e))
        .ok()
}

pub async fn getapp(id: &str) -> Option<FlathubApp> {
    let appstream = getappstream(id).await?;
    Some(FlathubApp {
        id: id.to_string(),
        version: appstream.releases.and_then(|r| r.into_iter().next()).and_then(|r| r.version),
    })
}

pub async fn appdetails(id: &str) -> Option<FlathubDetails> {
    let appstream = getappstream(id).await?;
    let screenshots = appstream
        .screenshots
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| {
            // The largest size that's at most 1000px wide; the page scales screenshots to 640px
            s.sizes
                .into_iter()
                .filter_map(|size| Some((size.width.parse::<u32>().ok()?, size.src)))
                .filter(|(w, _)| *w <= 1000)
                .max_by_key(|(w, _)| *w)
                .map(|(_, src)| src)
        })
        .collect();
    let icon = match appstream.icon {
        Some(url) => cacheicon(id, &url).await,
        None => None,
    };
    Some(FlathubDetails {
        id: id.to_string(),
        name: appstream.name.unwrap_or_else(|| id.to_string()),
        summary: appstream.summary,
        description: appstream.description,
        icon,
        screenshots,
        homepage: appstream.urls.and_then(|u| u.homepage),
        license: appstream.project_license,
        developer: appstream.developer_name,
        version: appstream.releases.and_then(|r| r.into_iter().next()).and_then(|r| r.version),
    })
}

/// Every app on Flathub. The list is cached for a day; an older one is used if Flathub can't be reached.
pub async fn catalog() -> Vec<CatalogApp> {
    let Ok(home) = std::env::var("HOME") else {
        return vec![];
    };
    let dir = format!("{}/.cache/nix-software-center", home);
    let path = format!("{}/flathub-catalog.json", dir);
    let cached = tokio::fs::read_to_string(&path).await.ok().and_then(|s| serde_json::from_str::<Vec<CatalogApp>>(&s).ok());
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map_or(false, |age| age < Duration::from_secs(24 * 60 * 60));
    if let (Some(apps), true) = (&cached, fresh) {
        return apps.clone();
    }
    match fetchcatalog().await {
        Some(apps) => {
            if let Ok(json) = serde_json::to_string(&apps) {
                let _ = tokio::fs::create_dir_all(&dir).await;
                if let Err(e) = tokio::fs::write(&path, json).await {
                    warn!("Failed to cache the Flathub catalog: {}", e);
                }
            }
            apps
        }
        None => cached.unwrap_or_default(),
    }
}

async fn fetchcatalog() -> Option<Vec<CatalogApp>> {
    let first = catalogpage(1).await?;
    let rest = (2..=first.total_pages).map(|page| tokio::spawn(catalogpage(page))).collect::<Vec<_>>();
    let mut hits = first.hits;
    for page in rest {
        // A partial catalog would hide apps, so any failed page fails the whole fetch
        hits.extend(page.await.ok()??.hits);
    }
    Some(
        hits.into_iter()
            .map(|hit| CatalogApp {
                id: hit.app_id,
                name: hit.name,
                summary: hit.summary,
                icon: hit.icon,
                keywords: hit.keywords.unwrap_or_default(),
                installs: hit.installs_last_month.unwrap_or_default(),
                nixattr: None,
            })
            .collect(),
    )
}

async fn catalogpage(page: u32) -> Option<SearchResults> {
    let response = reqwest::Client::new()
        .post(format!("{}/search", FLATHUB_API))
        .header("Content-Type", "application/json")
        .body(serde_json::json!({ "query": "", "hits_per_page": 1000, "page": page }).to_string())
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        warn!("Failed to get Flathub catalog page {}: {}", page, response.status());
        return None;
    }
    serde_json::from_str::<SearchResults>(&response.text().await.ok()?)
        .map_err(|e| warn!("Failed to parse Flathub catalog page {}: {}", page, e))
        .ok()
}

/// Catalog apps matching every word of a search, best first: name matches, then the most installed
pub fn searchcatalog<'a>(apps: &'a [CatalogApp], words: &[String], limit: usize) -> Vec<&'a CatalogApp> {
    if words.is_empty() {
        return vec![];
    }
    let words = words.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>();
    let mut matches = apps
        .iter()
        .filter_map(|app| {
            let name = app.name.to_lowercase();
            let text = format!(
                "{} {} {} {}",
                name,
                app.summary.as_deref().unwrap_or_default().to_lowercase(),
                app.id.to_lowercase(),
                app.keywords.join(" ").to_lowercase()
            );
            if !words.iter().all(|w| text.contains(w.as_str())) {
                return None;
            }
            let inname = words.iter().all(|w| name.contains(w.as_str()));
            Some(((name == words.join(" "), inname), app))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(a, x), (b, y)| b.cmp(a).then(y.installs.cmp(&x.installs)));
    matches.into_iter().take(limit).map(|(_, app)| app).collect()
}

/// Downloads an app's icon into the cache, returning its path
pub async fn cacheicon(id: &str, url: &str) -> Option<String> {
    let dir = format!("{}/.cache/nix-software-center/flathub-icons", std::env::var("HOME").ok()?);
    let path = format!("{}/{}.png", dir, id);
    if Path::new(&path).is_file() {
        return Some(path);
    }
    let response = reqwest::get(url).await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    tokio::fs::create_dir_all(&dir).await.ok()?;
    tokio::fs::write(&path, bytes).await.ok()?;
    Some(path)
}

/// Normalized forms of an app's name and the last part of its ID, for matching to a nixpkgs pname
pub fn normalizedids(id: &str, name: &str) -> Vec<String> {
    vec![normalize(name), normalize(id.rsplit('.').next().unwrap_or_default())]
}

/// Whether a homepage belongs to the owner in a Flathub app ID: com.spotify.Client -> spotify,
/// io.github.hrkfdn.ncspot -> hrkfdn. Confirms matches made by name, which can pair unrelated
/// projects (the Flow browser and the flow type checker).
pub fn sameproject(id: &str, homepage: &str) -> bool {
    let parts = id.split('.').collect::<Vec<_>>();
    let owner = match parts[..] {
        [_, host, owner, _, ..] if ["github", "gitlab", "codeberg", "sourceforge", "sr"].contains(&host) => owner,
        [_, owner, _, ..] => owner,
        _ => return false,
    };
    let owner = normalize(owner);
    owner.len() >= 3 && normalize(homepage).contains(&owner)
}

pub fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase()
}

/// Compares two version strings part by part, numerically where both parts are numbers.
/// Returns None if either has no numbers at all (for example a git revision).
pub fn compareversions(a: &str, b: &str) -> Option<Ordering> {
    fn parts(v: &str) -> Vec<&str> {
        v.trim_start_matches('v').split(|c: char| !c.is_ascii_alphanumeric()).filter(|p| !p.is_empty()).collect()
    }
    let (a, b) = (parts(a), parts(b));
    if !a.iter().any(|p| p.parse::<u64>().is_ok()) || !b.iter().any(|p| p.parse::<u64>().is_ok()) {
        return None;
    }
    for (x, y) in a.iter().zip(b.iter()) {
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => x.cmp(y),
        };
        if ord != Ordering::Equal {
            return Some(ord);
        }
    }
    Some(a.len().cmp(&b.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(compareversions("1.14.0", "2.0.0"), Some(Ordering::Less));
        assert_eq!(compareversions("10.0.0", "9.4.0"), Some(Ordering::Greater));
        assert_eq!(compareversions("1.18.2", "1.18.2"), Some(Ordering::Equal));
        assert_eq!(compareversions("1.2.95.453.g0eeebbed", "1.2.95.453.g0eeebbed"), Some(Ordering::Equal));
        assert_eq!(compareversions("v51.0", "50.2.2"), Some(Ordering::Greater));
        assert_eq!(compareversions("1.0", "1.0.1"), Some(Ordering::Less));
        assert_eq!(compareversions("unstable", "1.0"), None);
    }

    #[test]
    fn projects() {
        assert!(sameproject("com.spotify.Client", "https://www.spotify.com"));
        assert!(sameproject("io.github.hrkfdn.ncspot", "https://github.com/hrkfdn/ncspot"));
        assert!(sameproject("org.clementine_player.Clementine", "https://www.clementine-player.org/"));
        assert!(!sameproject("com.flow_browser.flow", "https://flow.org/"));
        // Owners under 3 characters match too much to be trusted
        assert!(!sameproject("org.x.Warpinator", "https://github.com/linuxmint/warpinator"));
        assert!(!sameproject("Fluxer", "https://fluxer.app"));
    }
}
