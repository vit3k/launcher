use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize, Debug, Clone)]
pub struct GogGame {
    pub display_name: String,
    pub install_location: String,
    pub launch_executable: String,
    pub source_key: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct RunningGogGame {
    pub display_name: String,
    pub source_key: String,
    pub pid: u32,
    pub exe_path: String,
}

pub fn get_installed_gog_games() -> Vec<GogGame> {
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }

    #[cfg(target_os = "windows")]
    {
        use std::collections::HashSet;
        use winreg::RegKey;
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};

        let mut games = Vec::new();
        let mut seen = HashSet::new();

        let roots = [
            ("HKLM", RegKey::predef(HKEY_LOCAL_MACHINE)),
            ("HKCU", RegKey::predef(HKEY_CURRENT_USER)),
        ];
        let uninstall_paths = [
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ];

        for (root_name, root) in roots {
            for uninstall_path in uninstall_paths {
                let Ok(uninstall) = root.open_subkey_with_flags(uninstall_path, KEY_READ) else {
                    continue;
                };

                for subkey_name in uninstall.enum_keys().flatten() {
                    let Ok(entry) = uninstall.open_subkey_with_flags(&subkey_name, KEY_READ) else {
                        continue;
                    };

                    let display_name = read_string_value(&entry, "DisplayName");
                    if display_name.is_empty() {
                        continue;
                    }

                    let publisher = read_string_value(&entry, "Publisher");
                    let uninstall_string = read_string_value(&entry, "UninstallString");
                    let source_blob = format!(
                        "{} {} {} {}",
                        display_name, publisher, uninstall_string, subkey_name
                    )
                    .to_ascii_lowercase();

                    if !source_blob.contains("gog") {
                        continue;
                    }

                    if display_name.to_ascii_lowercase().contains("gog galaxy") {
                        continue;
                    }

                    let mut install_location = read_string_value(&entry, "InstallLocation");
                    let mut launch_executable = normalize_display_icon_path(
                        &read_string_value(&entry, "DisplayIcon"),
                    );
                    if !is_executable_path(&launch_executable) {
                        launch_executable.clear();
                    }

                    if launch_executable.is_empty() && !install_location.is_empty() {
                        launch_executable = guess_game_executable(&install_location, &display_name);
                    }

                    if install_location.is_empty() && !launch_executable.is_empty() {
                        if let Some(parent) = PathBuf::from(&launch_executable).parent() {
                            install_location = parent.to_string_lossy().to_string();
                        }
                    }

                    let key = format!(
                        "{}|{}|{}",
                        display_name.to_ascii_lowercase(),
                        install_location.to_ascii_lowercase(),
                        launch_executable.to_ascii_lowercase()
                    );
                    if !seen.insert(key) {
                        continue;
                    }

                    games.push(GogGame {
                        display_name,
                        install_location,
                        launch_executable,
                        source_key: format!("{}\\{}", root_name, subkey_name),
                    });
                }
            }
        }

        games
    }
}

pub fn launch_gog_game(game: &GogGame) -> io::Result<()> {
    if game.launch_executable.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "No launch executable found for GOG game",
        ));
    }
    if !is_executable_path(&game.launch_executable) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Resolved launch path is not an executable: {}", game.launch_executable),
        ));
    }

    let mut cmd = Command::new(&game.launch_executable);
    if let Some(parent) = PathBuf::from(&game.launch_executable).parent() {
        cmd.current_dir(parent);
    } else if !game.install_location.is_empty() {
        cmd.current_dir(&game.install_location);
    }
    cmd.spawn().map(|_| ())
}

#[cfg(target_os = "windows")]
fn read_string_value(key: &winreg::RegKey, value_name: &str) -> String {
    key.get_value::<String, _>(value_name)
        .unwrap_or_default()
        .trim()
        .trim_matches('"')
        .to_string()
}

fn normalize_display_icon_path(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }

    let stripped = raw.trim().trim_matches('"');
    let icon_path = stripped.split(',').next().unwrap_or(stripped).trim();
    icon_path.trim_matches('"').to_string()
}

fn guess_game_executable(install_location: &str, display_name: &str) -> String {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let root = PathBuf::from(install_location);
    if !root.exists() {
        return String::new();
    }

    collect_executables_recursive(&root, 0, 4, &mut candidates);

    if candidates.is_empty() {
        return String::new();
    }

    let lowered_name = display_name.to_ascii_lowercase();
    let name_tokens: Vec<&str> = lowered_name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() >= 3)
        .collect();

    candidates.sort_by_key(|path| {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let full = path.to_string_lossy().to_ascii_lowercase();

        let mut penalty = 0u32;
        if file_name.contains("unins")
            || file_name.contains("setup")
            || file_name.contains("install")
            || file_name.contains("uninstall")
            || file_name.contains("crash")
            || file_name.contains("config")
            || file_name.contains("support")
            || file_name.contains("redist")
        {
            penalty += 50;
        }
        if file_name.contains("launcher") {
            penalty += 10;
        }
        if full.contains("\\_redist")
            || full.contains("\\support")
            || full.contains("\\directx")
        {
            penalty += 40;
        }
        if full.contains("\\system\\") || full.contains("\\bin\\") {
            penalty = penalty.saturating_sub(10);
        }

        for token in &name_tokens {
            if file_name.contains(token) {
                penalty = penalty.saturating_sub(15);
            }
        }

        (penalty, file_name)
    });

    candidates[0].to_string_lossy().to_string()
}

fn collect_executables_recursive(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<PathBuf>,
) {
    if depth > max_depth {
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_executables_recursive(&path, depth + 1, max_depth, out);
            continue;
        }
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("exe"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

fn is_executable_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("exe"))
        .unwrap_or(false)
}

pub fn get_running_gog_games() -> Vec<RunningGogGame> {
    let games = get_installed_gog_games();
    let mut running = Vec::new();
    let mut apps = crate::process::get_all_windows();
    for app in apps.drain(..) {
        let exe_lower = app.path.to_ascii_lowercase();
        for game in &games {
            if game.install_location.is_empty() {
                continue;
            }
            if exe_lower.contains(&game.install_location.to_ascii_lowercase()) {
                running.push(RunningGogGame {
                    display_name: game.display_name.clone(),
                    source_key: game.source_key.clone(),
                    pid: app.pid,
                    exe_path: app.path.clone(),
                });
                break;
            }
        }
    }
    running
}
