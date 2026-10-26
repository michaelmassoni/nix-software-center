use anyhow::Result;

use super::nixos;

/// Downloads the latest `packages.json` for the system from the Nix cache and returns the path to an SQLite database `nonnixospkgs.db` which contains package data.
/// Mean for non-NixOS systems.
pub async fn nixpkgs() -> Result<String> {
    nixos::channeldb(&["nixpkgs-unstable"], "nonnixospkgs").await
}
