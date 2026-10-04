use anyhow::{anyhow, Context};
use log::{info, warn};
use reqwest::StatusCode;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Downloaded data older than this is downloaded again
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub fn fetch_prices_and_items() -> Result<(PathBuf, PathBuf), anyhow::Error> {
    let prices = download_and_save("https://api.warframestat.us/wfinfo/prices/", "prices.json")?;
    let items = download_and_save(
        "https://api.warframestat.us/wfinfo/filtered_items/",
        "filtered_items.json",
    )?;
    Ok((prices, items))
}

/// Relic drop tables published by Digital Extremes
pub fn fetch_official_relics() -> Result<PathBuf, anyhow::Error> {
    download_and_save(
        "https://drops.warframestat.us/data/relics.json",
        "official_relics.json",
    )
}

fn is_fresh(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < MAX_AGE)
}

fn download(url: &str) -> Result<String, anyhow::Error> {
    let res = reqwest::blocking::get(url)?;
    if res.status() != StatusCode::OK {
        return Err(anyhow!("{url} answered {}", res.status()));
    }
    Ok(res.text()?)
}

fn download_and_save(url: &str, filename: &str) -> Result<PathBuf, anyhow::Error> {
    let path = std::env::temp_dir().join(filename);
    if is_fresh(&path) {
        return Ok(path);
    }

    info!("Downloading {filename}");
    match download(url) {
        Ok(text) => {
            fs::write(&path, text).with_context(|| format!("Failed to write {}", path.display()))?
        }
        // Outdated data is still better than none
        Err(err) if path.exists() => warn!("Failed to update {filename}, using old copy: {err}"),
        Err(err) => return Err(err.context(format!("Failed to download {filename}"))),
    }

    Ok(path)
}
