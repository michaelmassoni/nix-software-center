{
  pkgs ? import <nixpkgs> { },
  lib ? import <nixpkgs/lib>,
  appstream-data ? (
    let
      src = pkgs.fetchFromGitHub {
        owner = "snowfallorg";
        repo = "nixos-appstream-data";
        rev = "df1a03d7d838026f3c70ca18b1d8867f605ed51c";
        hash = "sha256-uXNpJv687gJMITVYsfuGOywoElKxEcZMRd6HYVHFu24=";
      };
    in
    pkgs.symlinkJoin {
      name = "nixos-appstream-data-all";
      paths = map (section: pkgs.callPackage "${src}/default.nix" { inherit section; }) [
        "nixos-unstable"
        "nixos-unstable-unfree"
      ];
    }
  ),
}:
let
  # The app reads the older app-info layout (one DEP-11 YAML catalog, icons/nixos/<size>/),
  # so merge Snowfall's per-section swcatalog XML into one catalog and convert it
  nixos-app-info = pkgs.runCommand "nixos-app-info" { nativeBuildInputs = [ pkgs.appstream ]; } ''
    {
      echo '<?xml version="1.0" encoding="utf-8"?>'
      echo '<components version="1.0" origin="nixos">'
      for f in ${appstream-data}/share/swcatalog/xml/*.xml.gz; do
        zcat "$f" | sed -e '1,/<components[^>]*>/d' -e '/<\/components>/d'
      done
      echo '</components>'
    } > nixos.xml
    mkdir -p $out/share/app-info/xmls
    appstreamcli convert nixos.xml $out/share/app-info/xmls/nixos_x86_64_linux.yml
    gzip $out/share/app-info/xmls/nixos_x86_64_linux.yml

    for size in ${appstream-data}/share/swcatalog/icons/*/*; do
      mkdir -p $out/share/app-info/icons/nixos/$(basename $size)
      cp -rs $size/. $out/share/app-info/icons/nixos/$(basename $size)/
    done
  '';
in
pkgs.stdenv.mkDerivation {
  pname = "nix-software-center";
  version = "0.1.2";

  src = [ ./. ];

  cargoDeps = pkgs.rustPlatform.importCargoLock {
    lockFile = ./Cargo.lock;
  };

  nativeBuildInputs =
    with pkgs;
    [
      appstream-glib
      polkit
      gettext
      desktop-file-utils
      meson
      ninja
      pkg-config
      git
      wrapGAppsHook4
    ]
    ++ (with pkgs.rustPlatform; [
      cargoSetupHook
      cargo
      rustc
    ]);

  buildInputs = with pkgs; [
    gdk-pixbuf
    glib
    gtk4
    gtksourceview5
    libadwaita
    libxml2
    openssl
    wayland
    adwaita-icon-theme
    desktop-file-utils
    nixos-app-info
  ];

  patchPhase = ''
    substituteInPlace ./src/lib.rs \
        --replace "/usr/share/app-info" "${nixos-app-info}/share/app-info"
  '';

  postInstall = ''
    wrapProgram $out/bin/nix-software-center --prefix PATH : '${
      lib.makeBinPath [
        pkgs.gnome-console
        pkgs.gtk3 # provides gtk-launch
        pkgs.sqlite
      ]
    }'
  '';
}
