pub mod about;
pub mod categories;
pub mod categorypage;
pub mod categorytile;
pub mod installedpage;
pub mod installworker;
pub mod pkgpage;
pub mod pkgtile;
pub mod preferencespage;
pub mod rebuild;
pub mod screenshotfactory;
pub mod searchpage;
pub mod unavailabledialog;
pub mod updatepage;
pub mod updateworker;
pub mod welcome;
pub mod window;
pub mod windowloading;

use std::path::Path;
use crate::APPINFO;

/// The package shown under an app's name: its nixpkgs attribute, or "Flathub · <app ID>"
pub fn pkglabel(pkg: &str) -> String {
    match crate::parse::flathub::flathubid(pkg) {
        Some(id) => format!("Flathub · {}", id),
        None => pkg.to_string(),
    }
}

/// Icon file for an app: a cached Flathub icon (an absolute path) or an icon in the nixpkgs AppStream data
pub fn iconpath(icon: &Option<String>) -> Option<String> {
    let i = icon.as_ref()?;
    if i.starts_with('/') {
        return Path::new(i).is_file().then(|| i.to_string());
    }
    ["128x128", "64x64"]
        .iter()
        .map(|size| format!("{}/icons/nixos/{}/{}", APPINFO, size, i))
        .find(|p| Path::new(p).is_file())
}

