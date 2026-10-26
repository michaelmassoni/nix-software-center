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

use relm4::gtk;

/// Icon name for a package without catalog data: an icon in the user's theme named after the package, else a generic one
pub fn themeicon(pkg: &str, pname: &str) -> String {
    let fallback = String::from("package-x-generic");
    let Some(display) = gtk::gdk::Display::default() else {
        return fallback;
    };
    let theme = gtk::IconTheme::for_display(&display);
    // Attribute without its package set, e.g. kdePackages.kate -> kate
    let attr = pkg.rsplit('.').next().unwrap_or_default();
    [attr, pname]
        .into_iter()
        .filter(|n| !n.is_empty())
        .map(str::to_lowercase)
        .find(|n| theme.has_icon(n))
        .unwrap_or(fallback)
}
