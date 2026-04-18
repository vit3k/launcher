use serde::Deserialize;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize, Debug, Clone)]
pub struct EpicGame {
    pub app_name: String,
    pub display_name: String,
    pub install_location: String,
    pub launch_executable: String,
    pub manifest_path: String,
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

        if manifest.install_location.is_empty() {
            continue;
        }

        games.push(EpicGame {
            app_name: manifest.app_name,
            display_name: manifest.display_name,
            install_location: manifest.install_location,
            launch_executable: manifest.launch_executable,
            manifest_path: path.to_string_lossy().to_string(),
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
        let status = Command::new("cmd")
            .args(["/c", "start", "", &uri])
            .spawn();
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
