use crate::CACHEDIR;
use anyhow::{anyhow, Context, Result};
use log::{debug, info};
use serde::Deserialize;
use serde_json::{value::RawValue, Value};
use sqlx::{
    migrate::MigrateDatabase,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    Row, Sqlite, SqlitePool,
};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufReader, Write},
    path::Path,
    process::{Command, Stdio},
};

use super::{channel, flakes};

/// Downloads the latest `packages.json` for the system from the NixOS cache and returns the path to an SQLite database `nixospkgs.db` which contains package data.
/// Will only work on NixOS systems.
pub async fn nixospkgs() -> Result<String> {
    let versionout = Command::new("nixos-version").output()?;
    let version = &String::from_utf8(versionout.stdout)?[0..5];
    // Unstable systems report the upcoming release (e.g. 26.11), which has no channel yet
    channeldb(&[&format!("nixos-{}", version), "nixos-unstable"], "nixospkgs").await
}

#[derive(Deserialize)]
struct ChannelPkgList {
    packages: HashMap<String, ChannelPkg>,
}

#[derive(Deserialize)]
struct ChannelPkg {
    system: Option<String>,
    pname: Option<String>,
    version: Option<String>,
    #[serde(default)]
    meta: ChannelMeta,
}

// JSON fields stay raw text: they are stored as-is and parsed by the app
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ChannelMeta {
    broken: Option<bool>,
    insecure: Option<bool>,
    unsupported: Option<bool>,
    unfree: Option<bool>,
    description: Option<String>,
    long_description: Option<String>,
    homepage: Option<Value>,
    maintainers: Option<Box<RawValue>>,
    position: Option<String>,
    license: Option<Box<RawValue>>,
    platforms: Option<Box<RawValue>>,
}

/// Builds `{name}.db` in the cache from the `packages.json` of the first of `channels` that exists on
/// channels.nixos.org, and returns its path. Only rebuilt when the channel moves to a new release.
pub(super) async fn channeldb(channels: &[&str], name: &str) -> Result<String> {
    let dbpath = format!("{}/{}.db", &*CACHEDIR, name);
    let verpath = format!("{}/{}.ver", &*CACHEDIR, name);

    // If cache directory doesn't exist, create it
    if !Path::new(&*CACHEDIR).exists() {
        fs::create_dir_all(&*CACHEDIR)?;
    }

    // channels.nixos.org/<channel> redirects to its current release, e.g. .../nixos-26.11pre1100280.3ec636362e18
    debug!("Checking channel versions");
    let mut found = None;
    for channel in channels {
        match reqwest::get(format!("https://channels.nixos.org/{}", channel)).await {
            Ok(resp) if resp.status().is_success() => {
                let release = resp
                    .url()
                    .path_segments()
                    .and_then(|mut s| s.next_back())
                    .context("No release found")?
                    .to_string();
                found = Some((*channel, release));
                break;
            }
            Ok(_) => continue,
            Err(_) => {
                // Internet connection failed
                // Check if we can use the old database
                if Path::new(&dbpath).exists() {
                    info!("Using old database");
                    return Ok(dbpath);
                }
                return Err(anyhow!("Could not reach channels.nixos.org"));
            }
        }
    }
    let (channel, release) =
        found.with_context(|| format!("None of these channels exist: {}", channels.join(", ")))?;
    info!("Latest {} release: {}", channel, release);

    // Check if latest version is already downloaded
    if fs::read_to_string(&verpath).map_or(false, |v| v == release) && Path::new(&dbpath).exists()
    {
        debug!("No new version of {} found", channel);
        return Ok(dbpath);
    }

    debug!("Downloading {} packages.json", channel);
    // Decompressed by hand so the ~400 MB of JSON is parsed as a stream instead of held in memory
    let client = reqwest::Client::builder().brotli(false).build()?;
    let resp = client
        .get(format!("https://channels.nixos.org/{}/packages.json.br", channel))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("Failed to download {} packages.json", channel));
    }
    let bytes = resp.bytes().await?;
    let pkglist: ChannelPkgList = tokio::task::spawn_blocking(move || {
        serde_json::from_reader(BufReader::new(brotli::Decompressor::new(bytes.as_ref(), 4096)))
    })
    .await??;

    debug!("Writing {} database", name);
    // Built beside the old database and swapped in, so a failed build never replaces a working one
    let tmppath = format!("{}.tmp", dbpath);
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{}", tmppath, suffix));
    }
    // One connection and no WAL, so the file holds everything once the pool closes and can be renamed
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&tmppath)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Delete),
        )
        .await?;
    sqlx::query(
        r#"
            CREATE TABLE "pkgs" (
                "attribute"	TEXT NOT NULL UNIQUE,
                "system"	TEXT,
                "pname"	TEXT,
                "version"	TEXT,
                PRIMARY KEY("attribute")
            )
            "#,
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        r#"
            CREATE TABLE "meta" (
                "attribute"	TEXT NOT NULL UNIQUE,
                "broken"	INTEGER,
                "insecure"	INTEGER,
                "unsupported"	INTEGER,
                "unfree"	INTEGER,
                "description"	TEXT,
                "longdescription"	TEXT,
                "homepage"	TEXT,
                "maintainers"	JSON,
                "position"	TEXT,
                "license"	JSON,
                "platforms"	JSON,
                FOREIGN KEY("attribute") REFERENCES "pkgs"("attribute"),
                PRIMARY KEY("attribute")
            )
            "#,
    )
    .execute(&pool)
    .await?;

    // The app reads every column as a string, so missing values are stored empty rather than NULL
    let raw = |v: &Option<Box<RawValue>>, empty: &str| {
        v.as_ref().map_or(empty.to_string(), |v| v.get().to_string())
    };
    let mut tx = pool.begin().await?;
    for (attribute, pkg) in &pkglist.packages {
        sqlx::query(r#"INSERT INTO "pkgs" VALUES ($1, $2, $3, $4)"#)
            .bind(attribute)
            .bind(pkg.system.as_deref().unwrap_or_default())
            .bind(pkg.pname.as_deref().unwrap_or_default())
            .bind(pkg.version.as_deref().unwrap_or_default())
            .execute(&mut tx)
            .await?;
        let meta = &pkg.meta;
        // A few packages list several homepages; the app shows one
        let homepage = match &meta.homepage {
            Some(Value::String(h)) => h.as_str(),
            Some(Value::Array(h)) => h.first().and_then(|h| h.as_str()).unwrap_or_default(),
            _ => "",
        };
        sqlx::query(
            r#"INSERT INTO "meta" VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
        )
        .bind(attribute)
        .bind(meta.broken.unwrap_or(false))
        .bind(meta.insecure.unwrap_or(false))
        .bind(meta.unsupported.unwrap_or(false))
        .bind(meta.unfree.unwrap_or(false))
        .bind(meta.description.as_deref().unwrap_or_default())
        .bind(meta.long_description.as_deref().unwrap_or_default())
        .bind(homepage)
        .bind(raw(&meta.maintainers, "[]"))
        .bind(meta.position.as_deref().unwrap_or_default())
        .bind(raw(&meta.license, ""))
        .bind(raw(&meta.platforms, "[]"))
        .execute(&mut tx)
        .await?;
    }
    tx.commit().await?;
    for index in [
        r#"CREATE UNIQUE INDEX "attributes" ON "pkgs" ("attribute")"#,
        r#"CREATE UNIQUE INDEX "metaattributes" ON "meta" ("attribute")"#,
        r#"CREATE INDEX "pnames" ON "pkgs" ("pname")"#,
    ] {
        sqlx::query(index).execute(&pool).await?;
    }
    pool.close().await;

    // A leftover journal from the old database would be replayed onto the new one
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{}", dbpath, suffix));
    }
    fs::rename(&tmppath, &dbpath)?;
    debug!("Writing {} version", name);
    // Write version downloaded to file
    File::create(&verpath)?.write_all(release.as_bytes())?;
    Ok(dbpath)
}

/// Returns the version of every package in `nixospkgs.db` if it was built from nixpkgs revision `rev`.
/// Spares flake systems on the current channel release a full `nix search` of nixpkgs.
pub(super) async fn catalogversions(rev: &str) -> Option<HashMap<String, String>> {
    let release = fs::read_to_string(format!("{}/nixospkgs.ver", &*CACHEDIR)).ok()?;
    // Release names end in the first 12 characters of their revision
    if !release.ends_with(rev.get(0..12)?) {
        return None;
    }
    let pool = SqlitePool::connect(&format!("sqlite://{}/nixospkgs.db", &*CACHEDIR))
        .await
        .ok()?;
    let pkgs: Vec<(String, String)> = sqlx::query_as("SELECT attribute, version FROM pkgs")
        .fetch_all(&pool)
        .await
        .ok()?;
    debug!("Reusing {} package versions from nixospkgs.db", pkgs.len());
    Some(pkgs.into_iter().collect())
}

/// Downloads the latest 'options.json' for the system from the NixOS cache and returns the path to the file.
/// Will only work on NixOS systems.
pub fn nixosoptions() -> Result<String> {
    let versionout = Command::new("nixos-version").output()?;
    let mut version = &String::from_utf8(versionout.stdout)?[0..5];

    // If cache directory doesn't exist, create it
    if !std::path::Path::new(&*CACHEDIR).exists() {
        std::fs::create_dir_all(&*CACHEDIR)?;
    }

    let verurl = format!("https://channels.nixos.org/nixos-{}", version);
    debug!("Checking NixOS version");
    let resp = reqwest::blocking::get(&verurl)?;
    let latestnixosver = if resp.status().is_success() {
        resp.url()
            .path_segments()
            .context("No path segments found")?
            .last()
            .context("Last element not found")?
            .to_string()
    } else {
        let resp = reqwest::blocking::get("https://channels.nixos.org/nixos-unstable")?;
        if resp.status().is_success() {
            version = "unstable";
            resp.url()
                .path_segments()
                .context("No path segments found")?
                .last()
                .context("Last element not found")?
                .to_string()
        } else {
            return Err(anyhow!("Could not find latest NixOS version"));
        }
    };
    debug!("Latest NixOS version: {}", latestnixosver);

    let url = format!(
        "https://channels.nixos.org/nixos-{}/options.json.br",
        version
    );

    // Download file with reqwest blocking
    let client = reqwest::blocking::Client::builder().brotli(true).build()?;
    let mut resp = client.get(url).send()?;
    if resp.status().is_success() {
        let mut out = File::create(&format!("{}/nixosoptions.json", &*CACHEDIR))?;
        resp.copy_to(&mut out)?;
        // Write version downloaded to file
        File::create(format!("{}/nixosoptions.ver", &*CACHEDIR))?
            .write_all(latestnixosver.as_bytes())?;
    } else {
        return Err(anyhow!("Failed to download latest options.json"));
    }

    Ok(format!("{}/nixosoptions.json", &*CACHEDIR))
}

pub(super) enum NixosType {
    Flake,
    Legacy,
}

pub(super) async fn getnixospkgs(
    paths: &[&str],
    nixos: NixosType,
) -> Result<HashMap<String, String>> {
    let pkgs = {
        let mut allpkgs: HashSet<String> = HashSet::new();
        for path in paths {
            if let Ok(filepkgs) = nix_editor::read::getarrvals(
                &fs::read_to_string(path)?,
                "environment.systemPackages",
            ) {
                let filepkgset = filepkgs
                    .into_iter()
                    .map(|x| x.strip_prefix("pkgs.").unwrap_or(&x).to_string())
                    .collect::<HashSet<_>>();
                allpkgs = allpkgs.union(&filepkgset).map(|x| x.to_string()).collect();
            }
        }
        allpkgs
    };
    debug!("getnixospkgs: {:?}", pkgs);
    let pkgsdb = match nixos {
        NixosType::Flake => flakes::flakespkgs().await?,
        NixosType::Legacy => channel::legacypkgs().await?,
    };
    let mut out = HashMap::new();
    let pool = SqlitePool::connect(&format!("sqlite://{}", pkgsdb)).await?;
    for pkg in pkgs {
        let mut sqlout = sqlx::query(
            r#"
            SELECT version FROM pkgs WHERE attribute = $1
            "#,
        )
        .bind(&pkg)
        .fetch_all(&pool)
        .await?;
        if sqlout.len() == 1 {
            let row = sqlout.pop().unwrap();
            let version: String = row.get("version");
            out.insert(pkg, version);
        }
    }
    Ok(out)
}

pub(super) async fn createdb(dbfile: &str, pkgjson: &HashMap<String, String>) -> Result<()> {
    let db = format!("sqlite://{}", dbfile);
    if Path::new(dbfile).exists() {
        fs::remove_file(dbfile)?;
    }
    Sqlite::create_database(&db).await?;
    let pool = SqlitePool::connect(&db).await?;
    sqlx::query(
        r#"
            CREATE TABLE "pkgs" (
                "attribute"	TEXT NOT NULL UNIQUE,
                "version"	TEXT,
                PRIMARY KEY("attribute")
            )
            "#,
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        r#"
        CREATE UNIQUE INDEX "attributes" ON "pkgs" ("attribute")
        "#,
    )
    .execute(&pool)
    .await?;

    let mut wtr = csv::Writer::from_writer(vec![]);
    for (pkg, version) in pkgjson {
        wtr.serialize((pkg.to_string(), version.to_string()))?;
    }
    let data = String::from_utf8(wtr.into_inner()?)?;
    let mut cmd = Command::new("sqlite3")
        .arg("-csv")
        .arg(dbfile)
        .arg(".import '|cat -' pkgs")
        .stdin(Stdio::piped())
        .spawn()?;
    let cmd_stdin = cmd.stdin.as_mut().unwrap();
    cmd_stdin.write_all(data.as_bytes())?;
    let _status = cmd.wait()?;
    Ok(())
}
