<div align="center">

<img src="data/icons/dev.vlinkz.NixSoftwareCenter.svg"/>

Nix Software Center
===

[![Built with Nix][builtwithnix badge]][builtwithnix]
[![License: GPLv3][GPLv3 badge]][GPLv3]

A graphical app store for Nix, built with [GTK4](https://www.gtk.org/), [libadwaita](https://gitlab.gnome.org/GNOME/libadwaita) and [Relm4](https://relm4.org/), and inspired by [GNOME Software](https://gitlab.gnome.org/GNOME/gnome-software).

<img src="data/screenshots/overview-light.png#gh-light-mode-only"/>
<img src="data/screenshots/overview-dark.png#gh-dark-mode-only"/>

</div>

## About this fork

This is a maintained fork of [snowfallorg/nix-software-center](https://github.com/snowfallorg/nix-software-center). Upstream downloads its package list from a database that stopped updating in 2025, so packages added to nixpkgs since then don't appear in it.

Changes in this fork:

- **Current package list:** built from the `packages.json` that [channels.nixos.org](https://channels.nixos.org) publishes for your channel. It's rebuilt automatically when the channel moves to a new release.
- **Newer app details:** icons, screenshots and descriptions come from [snowfallorg/nixos-appstream-data](https://github.com/snowfallorg/nixos-appstream-data), which covers about 1,500 apps. Packages it doesn't cover use a matching icon from your icon theme, if there is one.
- **Bug fixes:** installed-version lookups no longer fail on current NixOS, which added a `specialisations` field to `nixos-version --json`.

## Features

- Browse and search nixpkgs by category
- Install and remove packages in your `configuration.nix`, then rebuild the system. Flake-based systems are supported; set your flake path in Preferences.
- Install packages for your user with `nix profile` or `nix-env`
- See available updates for installed packages
- Try an app without installing it, using `nix-shell` or `nix run`

## Installation on NixOS

Installing or removing system packages runs `nixos-rebuild` through `pkexec`. Recent NixOS releases no longer install a setuid `pkexec` by default, so enable it in your configuration:

```nix
security.polkit.enablePkexecWrapper = true;
```

### With flakes

Add the input to `flake.nix`:

```nix
{
  inputs = {
    nix-software-center.url = "github:michaelmassoni/nix-software-center";
    # ...
  };
}
```

Pass `inputs` to your modules (for example with `specialArgs = { inherit inputs; };`), then add the package in `configuration.nix`:

```nix
environment.systemPackages = [
  inputs.nix-software-center.packages.${pkgs.stdenv.hostPlatform.system}.default
];
```

### Without flakes

In `configuration.nix`:

```nix
{ config, pkgs, ... }:
let
  nix-software-center = import (builtins.fetchTarball
    "https://github.com/michaelmassoni/nix-software-center/archive/main.tar.gz") {
    inherit pkgs;
    inherit (pkgs) lib;
  };
in
{
  environment.systemPackages = [ nix-software-center ];
}
```

This always builds the latest `main`. To pin a version, replace `main` with a commit hash and add its `sha256`.

## Other ways to run it

Install for your user with `nix profile`:

```bash
nix profile install github:michaelmassoni/nix-software-center
```

Install for your user with `nix-env`:

```bash
git clone https://github.com/michaelmassoni/nix-software-center
nix-env -f nix-software-center -i nix-software-center
```

Run it once without installing:

```bash
nix run github:michaelmassoni/nix-software-center
```

If flakes aren't enabled on your system, add `--extra-experimental-features "nix-command flakes"` to the `nix run` command.

## Known limitations

- Update checks for packages installed with `nix profile` still use upstream's old version data.
- Apps added to nixpkgs since February 2026 have no icons or screenshots until Snowfall regenerates its AppStream data.

## Debugging

```bash
RUST_LOG=nix_software_center=trace,nix_data=debug nix-software-center
```

## Screenshots

<p align="middle">
  <img src="data/screenshots/frontpage-light.png#gh-light-mode-only"/>
  <img src="data/screenshots/frontpage-dark.png#gh-dark-mode-only"/>
</p>

<p align="middle">
  <img src="data/screenshots/application-light.png#gh-light-mode-only"/>
  <img src="data/screenshots/application-dark.png#gh-dark-mode-only"/>
</p>

<p align="middle">
  <img src="data/screenshots/searchpage-light.png#gh-light-mode-only"/>
  <img src="data/screenshots/searchpage-dark.png#gh-dark-mode-only"/>
</p>

## License

The code is licensed under the [GPLv3](LICENSE).

Some icons in [data/icons](data/icons/) contain assets from the [NixOS logo](https://github.com/NixOS/nixos-artwork/tree/master/logo), licensed under [CC-BY 4.0](https://creativecommons.org/licenses/by/4.0/), and from [GNOME Software](https://gitlab.gnome.org/GNOME/gnome-software/-/tree/main/data/icons/hicolor/scalable), licensed under [CC0-1.0](https://creativecommons.org/publicdomain/zero/1.0/).

App icons, screenshots and descriptions shown in the app come from [snowfallorg/nixos-appstream-data](https://github.com/snowfallorg/nixos-appstream-data) and keep the licenses stated in each app's metadata.

[builtwithnix badge]: https://img.shields.io/badge/Built%20With-Nix-41439A?style=for-the-badge&logo=nixos&logoColor=white
[builtwithnix]: https://builtwithnix.org/
[GPLv3 badge]: https://img.shields.io/badge/License-GPLv3-blue.svg?style=for-the-badge
[GPLv3]: https://opensource.org/licenses/GPL-3.0
