use regex::Regex;
use serde::Serialize;
use std::fs;

#[derive(Serialize, Debug, Clone)]
pub struct SteamGame {
    pub appid: String,
    pub name: String,
    pub install_dir: String,
    pub manifest_path: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct RunningSteamGame {
    pub appid: String,
    pub name: String,
    pub pid: u32,
    pub exe_path: String,
}

pub fn get_steam_libraries() -> Vec<std::path::PathBuf> {
    let mut libs = Vec::new();
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_READ, RegOpenKeyExW, RegQueryValueExW,
        };
        use windows::core::PCWSTR;
        let key = "Software\\Valve\\Steam";
        let key_w: Vec<u16> = key.encode_utf16().chain([0]).collect();
        let mut hkey: HKEY = HKEY(0 as isize as _);
        unsafe {
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key_w.as_ptr()),
                0,
                KEY_READ,
                &mut hkey,
            )
            .is_ok()
            {
                let mut buf = [0u16; 260];
                let mut len = (buf.len() * 2) as u32;
                let val = "SteamPath";
                let val_w: Vec<u16> = val.encode_utf16().chain([0]).collect();
                if RegQueryValueExW(
                    hkey,
                    PCWSTR(val_w.as_ptr()),
                    None,
                    None,
                    Some(buf.as_mut_ptr() as *mut u8),
                    Some(&mut len),
                )
                .is_ok()
                {
                    let path = String::from_utf16_lossy(&buf[..len as usize / 2])
                        .trim_end_matches('\0')
                        .to_string();
                    if !path.is_empty() {
                        libs.push(std::path::PathBuf::from(path.clone() + r"\\steamapps"));
                    }
                }
            }
        }
    }
    if let Some(main) = libs.get(0) {
        let vdf = main.join("libraryfolders.vdf");
        if let Ok(content) = std::fs::read_to_string(&vdf) {
            let re = Regex::new(r#"path"\s*"([^"]+)"#).unwrap();
            for cap in re.captures_iter(&content) {
                let p = std::path::PathBuf::from(cap[1].replace("/", r"\\") + r"\\steamapps");
                if !libs.contains(&p) {
                    libs.push(p);
                }
            }
        }
    }
    libs
}

pub fn get_steam_games_from_manifests() -> Vec<SteamGame> {
    use std::collections::HashSet;
    let mut games = Vec::new();
    let libs = get_steam_libraries();
    let re_appid = Regex::new(r#"appid"\s*"(\d+)"#).unwrap();
    let re_name = Regex::new(r#"name"\s*"([^"]+)"#).unwrap();
    let re_dir = Regex::new(r#"installdir"\s*"([^"]+)"#).unwrap();
    let mut seen = HashSet::new();
    for lib in libs {
        if let Ok(entries) = fs::read_dir(&lib) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(fname) = path.file_name().and_then(|f| f.to_str()) {
                    if fname.starts_with("appmanifest_") && fname.ends_with(".acf") {
                        if let Ok(content) = fs::read_to_string(&path) {
                            let appid = re_appid
                                .captures(&content)
                                .and_then(|c| c.get(1))
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_default();
                            let name = re_name
                                .captures(&content)
                                .and_then(|c| c.get(1))
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_default();
                            let install_dir = re_dir
                                .captures(&content)
                                .and_then(|c| c.get(1))
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_default();
                            if !appid.is_empty() && !name.is_empty() {
                                if seen.insert(appid.clone()) {
                                    games.push(SteamGame {
                                        appid,
                                        name,
                                        install_dir,
                                        manifest_path: path.to_string_lossy().to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    games
}

pub fn get_running_steam_games() -> Vec<RunningSteamGame> {
    let games = get_steam_games_from_manifests();
    let mut running = Vec::new();
    let mut game_dirs = Vec::new();
    for game in &games {
        game_dirs.push((
            game.appid.clone(),
            game.name.clone(),
            game.install_dir.clone(),
        ));
    }
    let mut apps = crate::process::get_all_windows();
    for app in apps.drain(..) {
        let exe_path = app.path.to_ascii_lowercase();
        for (appid, name, install_dir) in &game_dirs {
            if let Some(_pos) = exe_path.find(&install_dir.to_ascii_lowercase()) {
                running.push(RunningSteamGame {
                    appid: appid.clone(),
                    name: name.clone(),
                    pid: app.pid,
                    exe_path: app.path.clone(),
                });
                break;
            }
        }
    }
    running
}
