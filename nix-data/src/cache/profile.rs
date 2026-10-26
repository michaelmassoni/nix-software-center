use crate::{cache::nixos, CACHEDIR};
use anyhow::{Context, Result};
use log::{debug, error};
use serde::Deserialize;
use sqlx::SqlitePool;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    path::Path,
    process::Command,
};

use super::nixos::nixospkgs;

#[derive(Debug, Deserialize)]
struct ProfilePkgsRoot {
    elements: ProfileElements,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProfileElements {
    // Manifest version 3 (Nix 2.20+): keyed by element name
    Named(HashMap<String, ProfilePkgOut>),
    // Older manifests: a list
    List(Vec<ProfilePkgOut>),
}

#[derive(Debug, Deserialize)]
struct ProfilePkgOut {
    #[serde(rename = "attrPath")]
    attrpath: Option<String>,
    #[serde(rename = "originalUrl")]
    originalurl: Option<String>,
    #[serde(rename = "storePaths")]
    storepaths: Vec<String>,
}

/// Struct containing information about a package installed with `nix profile`.
#[derive(Debug)]
pub struct ProfilePkg {
    /// Store path name, e.g. `hello-2.12.3`
    pub name: String,
    pub originalurl: String,
    /// Name `nix profile` commands use for the package (Nix 2.20+)
    pub element: Option<String>,
    /// Installed version, from the store path name
    pub version: Option<String>,
}

/// Returns the elements of `~/.nix-profile/manifest.json`, with their names when the manifest has them.
fn manifestelements() -> Result<Vec<(Option<String>, ProfilePkgOut)>> {
    let manifest = format!("{}/.nix-profile/manifest.json", std::env::var("HOME")?);
    if !Path::new(&manifest).exists() {
        return Ok(vec![]);
    }
    let profileroot: ProfilePkgsRoot = serde_json::from_reader(File::open(&manifest)?)?;
    Ok(match profileroot.elements {
        ProfileElements::Named(elements) => elements
            .into_iter()
            .map(|(name, pkg)| (Some(name), pkg))
            .collect(),
        ProfileElements::List(elements) => elements.into_iter().map(|pkg| (None, pkg)).collect(),
    })
}

/// Splits a nixpkgs attribute path like `legacyPackages.x86_64-linux.hello` into its package set and attribute.
fn legacyattr(attrpath: &str) -> Option<(String, String)> {
    let parts = attrpath.split('.').collect::<Vec<_>>();
    if parts.len() < 3 || parts[0] != "legacyPackages" {
        return None;
    }
    Some((parts[..2].join("."), parts[2..].join(".")))
}

/// The version in a store path name, split the way `builtins.parseDrvName` does:
/// it starts after the first `-` that isn't followed by a letter.
fn drvversion(name: &str) -> Option<&str> {
    let bytes = name.as_bytes();
    (0..bytes.len().saturating_sub(1))
        .find(|&i| bytes[i] == b'-' && !bytes[i + 1].is_ascii_alphabetic())
        .map(|i| &name[i + 1..])
}

/// Returns a list of all packages installed with `nix profile`.
/// Keyed by nixpkgs attribute, or by `flake#attrPath` for packages from other flakes.
pub fn getprofilepkgs() -> Result<HashMap<String, ProfilePkg>> {
    let mut out = HashMap::new();
    for (element, pkg) in manifestelements()? {
        if let (Some(attrpath), Some(originalurl)) = (pkg.attrpath, pkg.originalurl) {
            let attr = match legacyattr(&attrpath) {
                Some((_, attr)) => attr,
                None => format!("{}#{}", originalurl, attrpath),
            };
            // Extra outputs (e.g. hello-2.12.3-man) have longer names than the main one
            let name = pkg
                .storepaths
                .iter()
                .filter_map(|p| p.get(44..))
                .min_by_key(|n| n.len())
                .context("Failed to get pkg name from store path")?
                .to_string();
            out.insert(
                attr,
                ProfilePkg {
                    version: drvversion(&name).map(str::to_string),
                    name,
                    originalurl,
                    element,
                },
            );
        }
    }
    Ok(out)
}

/// Returns the name `nix profile` commands use for the package installed from nixpkgs attribute `attr`.
pub fn elementname(attr: &str) -> Option<String> {
    getprofilepkgs().ok()?.remove(attr)?.element
}

/// Returns the argument `nix profile remove` and `upgrade` take for the package installed from nixpkgs
/// attribute `attr`: its element name on Nix 2.20+, else its attribute path as older Nix expects.
pub fn profilearg(attr: &str) -> String {
    elementname(attr).unwrap_or_else(|| format!("legacyPackages.x86_64-linux.{}", attr))
}

/// Whether the profile manifest names its elements (Nix 2.20+), which `nix profile upgrade --all` needs.
pub fn namedmanifest() -> bool {
    getprofilepkgs().map_or(false, |pkgs| pkgs.values().any(|pkg| pkg.element.is_some()))
}

/// Returns a list of all packages installed with `nix profile` with their installed version.
pub async fn getprofilepkgs_versioned() -> Result<HashMap<String, String>> {
    Ok(getprofilepkgs()?
        .into_iter()
        .filter_map(|(attr, pkg)| Some((attr, pkg.version?)))
        .collect())
}

/// Builds `nixpkgs.db` with the version each `nix profile` package would be upgraded to, and returns its path.
/// Versions are evaluated in the flake each package was installed from (e.g. `flake:nixpkgs`,
/// resolved through the registry the same way `nix profile upgrade` resolves it).
pub async fn nixpkgslatest() -> Result<String> {
    // If cache directory doesn't exist, create it
    if !Path::new(&*CACHEDIR).exists() {
        std::fs::create_dir_all(&*CACHEDIR)?;
    }

    // Attributes to evaluate, grouped by flake and package set
    let mut groups: HashMap<(String, String), Vec<String>> = HashMap::new();
    for (_, pkg) in manifestelements()? {
        if let (Some(attrpath), Some(originalurl)) = (pkg.attrpath, pkg.originalurl) {
            if let Some((set, attr)) = legacyattr(&attrpath) {
                groups.entry((originalurl, set)).or_default().push(attr);
            }
        }
    }

    let mut versions = HashMap::new();
    for ((url, set), attrs) in groups {
        debug!("Evaluating latest versions in {}#{}", url, set);
        // A version of null means the attribute is gone or fails to evaluate
        let apply = format!(
            r#"p: builtins.listToAttrs (map (a: {{ name = a; value = let r = builtins.tryEval ((builtins.foldl' (s: k: s.${{k}} or null) p (builtins.filter builtins.isString (builtins.split "\\." a))).version or null); in if r.success then r.value else null; }}) (builtins.fromJSON ''{}''))"#,
            serde_json::to_string(&attrs)?
        );
        let out = Command::new("nix")
            .arg("eval")
            .arg("--json")
            .arg(format!("{}#{}", url, set))
            .arg("--apply")
            .arg(&apply)
            .output()?;
        if !out.status.success() {
            error!(
                "Failed to evaluate versions in {}: {}",
                url,
                String::from_utf8_lossy(&out.stderr)
            );
            continue;
        }
        let evaluated: HashMap<String, Option<String>> = serde_json::from_slice(&out.stdout)?;
        versions.extend(
            evaluated
                .into_iter()
                .filter_map(|(attr, version)| Some((attr, version?))),
        );
    }

    let dbfile = format!("{}/nixpkgs.db", &*CACHEDIR);
    nixos::createdb(&dbfile, &versions).await?;
    Ok(dbfile)
}


pub async fn unavailablepkgs() -> Result<HashMap<String, String>> {
    let nixpath = Command::new("nix")
        .arg("eval")
        .arg("nixpkgs#path")
        .output()?
        .stdout;
    let nixpath = String::from_utf8(nixpath)?;
    let nixpath = nixpath.trim();

    let aliases = Command::new("nix-instantiate")
        .arg("--eval")
        .arg("-E")
        .arg(&format!("with import {} {{}}; builtins.attrNames ((self: super: lib.optionalAttrs config.allowAliases (import {}/pkgs/top-level/aliases.nix lib self super)) {{}} {{}})", nixpath, nixpath))
        .arg("--json")
        .output()?;
    let aliasstr = String::from_utf8(aliases.stdout)?;
    let aliasesout: HashSet<String> = serde_json::from_str(&aliasstr)?;

    let flakespkgs = getprofilepkgs()?;
    let mut unavailable = HashMap::new();
    for pkg in flakespkgs.keys() {
        if aliasesout.contains(pkg) && Command::new("nix-instantiate")
                .arg("--eval")
                .arg("-E")
                .arg(&format!("with import {} {{}}; builtins.tryEval ((self: super: lib.optionalAttrs config.allowAliases (import {}/pkgs/top-level/aliases.nix lib self super)) {{}} {{}}).{}", nixpath, nixpath, pkg))
                .output()?.status.success() {
            let out = Command::new("nix-instantiate")
                .arg("--eval")
                .arg("-E")
                .arg(&format!("with import {} {{}}; ((self: super: lib.optionalAttrs config.allowAliases (import {}/pkgs/top-level/aliases.nix lib self super)) {{}} {{}}).{}", nixpath, nixpath, pkg))
                .output()?;
            let err = String::from_utf8(out.stderr)?;
            let err = err.strip_prefix("error: ").unwrap_or(&err).trim();
            unavailable.insert(pkg.to_string(), err.to_string());
        }
    }

    let nixospkgs = nixospkgs().await?;
    let pool = SqlitePool::connect(&format!("sqlite://{}", nixospkgs)).await?;

    for pkg in flakespkgs.keys() {
        // Packages from other flakes aren't in the nixpkgs catalog
        if pkg.contains('#') {
            continue;
        }
        let meta: Option<(u8, u8)> =
            sqlx::query_as("SELECT broken,insecure FROM meta WHERE attribute = $1")
                .bind(pkg)
                .fetch_optional(&pool)
                .await?;
        match meta {
            None => {
                unavailable.insert(
                    pkg.to_string(),
                    String::from("Package not found in newer version of nixpkgs"),
                );
            }
            Some((1, _)) => {
                unavailable.insert(pkg.to_string(), String::from("Package is marked as broken"));
            }
            Some((_, 1)) => {
                unavailable.insert(
                    pkg.to_string(),
                    String::from("Package is marked as insecure"),
                );
            }
            _ => {}
        }
    }
    Ok(unavailable)
}
