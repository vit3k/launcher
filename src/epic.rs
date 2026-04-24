use serde::Deserialize;
use serde::Serialize;
use std::fs;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize, Debug, Clone)]
pub struct EpicGame {
    pub app_name: String,
    pub display_name: String,
    pub install_location: String,
    pub launch_executable: String,
    pub manifest_path: String,
    pub poster_url: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct RunningEpicGame {
    pub app_name: String,
    pub display_name: String,
    pub pid: u32,
    pub exe_path: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct EpicManifest {
    #[serde(default)]
    app_name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    install_location: String,
    #[serde(default)]
    launch_executable: String,
}

pub fn get_epic_manifest_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        // Default Epic Launcher manifest path for installed games.
        let program_data = std::env::var_os("PROGRAMDATA")?;
        Some(
            PathBuf::from(program_data)
                .join("Epic")
                .join("EpicGamesLauncher")
                .join("Data")
                .join("Manifests"),
        )
    }

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

pub fn get_installed_epic_games() -> Vec<EpicGame> {
    let mut games = Vec::new();

    let manifest_dir = match get_epic_manifest_dir() {
        Some(dir) => dir,
        None => return games,
    };

    let entries = match fs::read_dir(&manifest_dir) {
        Ok(entries) => entries,
        Err(_) => return games,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !is_epic_manifest_file(&path) {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };

        let manifest: EpicManifest = match serde_json::from_str(&content) {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };

        let manifest_json: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => serde_json::Value::Null,
        };

        if manifest.install_location.is_empty() {
            continue;
        }

        games.push(EpicGame {
            app_name: manifest.app_name,
            display_name: manifest.display_name,
            install_location: manifest.install_location,
            launch_executable: manifest.launch_executable,
            manifest_path: path.to_string_lossy().to_string(),
            poster_url: extract_epic_portrait_url(&manifest_json),
        });
    }

    games
}

pub fn launch_epic_game(game: &EpicGame) -> std::io::Result<()> {
    if !game.app_name.is_empty() {
        let uri = format!(
            "com.epicgames.launcher://apps/{}?action=launch&silent=true",
            game.app_name
        );
        let status = Command::new("explorer").arg(&uri).spawn();
        if status.is_ok() {
            return Ok(());
        }
    }

    let launch_path = PathBuf::from(&game.launch_executable);
    let exe_path = if launch_path.is_absolute() {
        launch_path
    } else {
        PathBuf::from(&game.install_location).join(launch_path)
    };

    let mut cmd = Command::new(exe_path);
    if !game.install_location.is_empty() {
        cmd.current_dir(&game.install_location);
    }
    cmd.spawn().map(|_| ())
}

fn is_epic_manifest_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("item"))
        .unwrap_or(false)
}

fn extract_epic_portrait_url(manifest_json: &serde_json::Value) -> Option<String> {
    let mut candidates = Vec::new();
    collect_epic_portrait_urls(manifest_json, false, &mut candidates);

    let mut seen = HashSet::new();
    candidates.retain(|u| seen.insert(u.clone()));
    candidates.into_iter().next()
}

fn collect_epic_portrait_urls(value: &serde_json::Value, key_is_portrait_hint: bool, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            let type_value = map
                .get("type")
                .or_else(|| map.get("Type"))
                .and_then(|v| v.as_str());
            let url_value = map
                .get("url")
                .or_else(|| map.get("Url"))
                .or_else(|| map.get("src"))
                .or_else(|| map.get("Src"))
                .or_else(|| map.get("imageUrl"))
                .or_else(|| map.get("ImageUrl"))
                .and_then(|v| v.as_str());
            if let (Some(t), Some(u)) = (type_value, url_value) {
                if is_portrait_hint(t) && looks_like_image_url(u) {
                    out.push(u.to_string());
                }
            }

            for (k, v) in map {
                let key_hint = key_is_portrait_hint || is_portrait_hint(k);
                collect_epic_portrait_urls(v, key_hint, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_epic_portrait_urls(item, key_is_portrait_hint, out);
            }
        }
        serde_json::Value::String(s) => {
            if looks_like_image_url(s) && (key_is_portrait_hint || is_portrait_hint(s)) {
                out.push(s.clone());
            }
        }
        _ => {}
    }
}

fn looks_like_image_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && (lower.contains(".jpg")
            || lower.contains(".jpeg")
            || lower.contains(".png")
            || lower.contains(".webp")
            || lower.contains(".avif"))
}

fn is_portrait_hint(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("portrait")
        || lower.contains("vertical")
        || lower.contains("tall")
        || lower.contains("poster")
        || lower.contains("boxtall")
        || lower.contains("600x900")
        || lower.contains("900x1200")
        || lower.contains("1200x1600")
}

pub fn get_running_epic_games() -> Vec<RunningEpicGame> {
    let games = get_installed_epic_games();
    let mut running = Vec::new();
    let mut apps = crate::process::get_all_windows();
    for app in apps.drain(..) {
        let exe_lower = app.path.to_ascii_lowercase();
        for game in &games {
            if game.install_location.is_empty() {
                continue;
            }
            if exe_lower.contains(&game.install_location.to_ascii_lowercase()) {
                running.push(RunningEpicGame {
                    app_name: game.app_name.clone(),
                    display_name: game.display_name.clone(),
                    pid: app.pid,
                    exe_path: app.path.clone(),
                });
                break;
            }
        }
    }
    running
}
