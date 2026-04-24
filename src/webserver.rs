use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use tiny_http::{Header, Response, Server};

use crate::epic::EpicGame;
use crate::gog::GogGame;
use crate::steam::SteamGame;

const BIND_ADDR: &str = "0.0.0.0:7878";
static STEAM_POSTER_CACHE: std::sync::OnceLock<Mutex<HashMap<String, Option<String>>>> =
    std::sync::OnceLock::new();

#[derive(serde::Serialize, Default, Clone)]
pub struct GamesPayload {
    pub steam: Vec<SteamGame>,
    pub epic: Vec<EpicGame>,
    pub gog: Vec<GogGame>,
}

#[derive(serde::Serialize)]
struct ApiGame {
    id: String,
    source: &'static str,
    name: String,
    poster_url: Option<String>,
}

#[derive(serde::Serialize)]
struct ApiRunningGame {
    id: String,
    source: &'static str,
    name: String,
    pid: u32,
    exe_path: String,
}

pub type SharedGames = Arc<Mutex<GamesPayload>>;

pub fn start(shared_games: SharedGames) {
    thread::Builder::new()
        .name("webserver".into())
        .spawn(move || run(shared_games))
        .expect("failed to spawn webserver thread");
}

fn run(shared_games: SharedGames) {
    let server = match Server::http(BIND_ADDR) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[webserver] failed to bind {BIND_ADDR}: {e}");
            return;
        }
    };
    eprintln!("[webserver] listening on http://{BIND_ADDR}");

    for request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().split('?').next().unwrap_or("").to_string();
        match (method.as_str(), url.trim_end_matches('/')) {
            ("GET", "/suspend") | ("POST", "/suspend") => handle_suspend(request),
            ("GET", "/games") => handle_games(request, &shared_games),
            ("GET", "/games/running") => handle_running_games(request),
            ("GET", "/windows") => handle_windows(request),
            ("POST", "/games/launch") => handle_launch(request, &shared_games),
            _ => {
                let _ = request.respond(Response::from_string("Not Found").with_status_code(404));
            }
        }
    }
}

fn handle_suspend(request: tiny_http::Request) {
    // let own_pid = std::process::id();

    // // Suspend whatever foreground process is running (e.g. the game).
    // if let Some(pid) = crate::process::get_foreground_pid() {
    //     if pid != own_pid {
    //         if let Err(e) = crate::process::suspend_process_by_pid_and_minimize(pid) {
    //             eprintln!("[webserver] suspend pid {pid} failed: {e:?}");
    //         }
    //     }
    // }

    // Respond before sleeping so the caller can receive the 200 OK.
    let _ = request.respond(Response::from_string("OK"));

    // Brief pause so the TCP response flushes before the system sleeps.
    thread::sleep(std::time::Duration::from_millis(300));

    put_system_to_sleep();
}

fn put_system_to_sleep() {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::Foundation::BOOLEAN;
        use windows::Win32::System::Power::SetSuspendState;
        // SetSuspendState(hibernate, force, disable_wake_events)
        let result = SetSuspendState(BOOLEAN(0), BOOLEAN(0), BOOLEAN(0));
        if result.0 == 0 {
            eprintln!("[webserver] SetSuspendState failed");
        }
    }
}

#[derive(serde::Deserialize)]
struct LaunchRequest {
    id: String,
}

fn steam_game_id(game: &SteamGame) -> String {
    game.appid.clone()
}

fn epic_game_id(game: &EpicGame) -> String {
    if !game.app_name.is_empty() {
        game.app_name.clone()
    } else {
        format!("manifest:{}", game.manifest_path)
    }
}

fn gog_game_id(game: &GogGame) -> String {
    game.source_key.clone()
}

fn steam_poster_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    STEAM_POSTER_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn steam_vertical_poster_candidates(appid: &str) -> [String; 4] {
    [
        format!("https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/{appid}/library_600x900.jpg"),
        format!("https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/{appid}/library_600x900_2x.jpg"),
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_600x900.jpg"),
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_600x900_2x.jpg"),
    ]
}

fn url_exists(url: &str) -> bool {
    ureq::get(url).call().is_ok()
}

fn steam_poster_url(appid: &str) -> Option<String> {
    if let Ok(cache) = steam_poster_cache().lock() {
        if let Some(cached) = cache.get(appid) {
            return cached.clone();
        }
    }

    // Keep posters portrait-only; do not fall back to horizontal header/capsule art.
    let resolved = steam_vertical_poster_candidates(appid)
        .into_iter()
        .find(|url| url_exists(url));
    if let Ok(mut cache) = steam_poster_cache().lock() {
        cache.insert(appid.to_string(), resolved.clone());
    }
    resolved
}

fn build_games_list(payload: &GamesPayload) -> Vec<ApiGame> {
    let mut out = Vec::new();

    for g in &payload.steam {
        out.push(ApiGame {
            id: steam_game_id(g),
            source: "steam",
            name: g.name.clone(),
            poster_url: steam_poster_url(&g.appid),
        });
    }

    for g in &payload.epic {
        out.push(ApiGame {
            id: epic_game_id(g),
            source: "epic",
            name: g.display_name.clone(),
            poster_url: g.poster_url.clone(),
        });
    }

    for g in &payload.gog {
        out.push(ApiGame {
            id: gog_game_id(g),
            source: "gog",
            name: g.display_name.clone(),
            poster_url: None,
        });
    }

    out
}

fn build_running_games_list() -> Vec<ApiRunningGame> {
    let mut out = Vec::new();

    for g in crate::steam::get_running_steam_games() {
        out.push(ApiRunningGame {
            id: g.appid,
            source: "steam",
            name: g.name,
            pid: g.pid,
            exe_path: g.exe_path,
        });
    }

    for g in crate::epic::get_running_epic_games() {
        let id = if !g.app_name.is_empty() {
            g.app_name
        } else {
            format!("pid:{}", g.pid)
        };
        out.push(ApiRunningGame {
            id,
            source: "epic",
            name: g.display_name,
            pid: g.pid,
            exe_path: g.exe_path,
        });
    }

    for g in crate::gog::get_running_gog_games() {
        out.push(ApiRunningGame {
            id: g.source_key,
            source: "gog",
            name: g.display_name,
            pid: g.pid,
            exe_path: g.exe_path,
        });
    }

    out
}

fn handle_launch(mut request: tiny_http::Request, shared_games: &SharedGames) {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        let _ = request.respond(Response::from_string("Bad Request").with_status_code(400));
        return;
    }
    let launch_req: LaunchRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(_) => {
            let _ = request.respond(
                Response::from_string(r#"{"error":"invalid JSON body, expected {\"id\":\"...\"}"}"#)
                    .with_status_code(400),
            );
            return;
        }
    };

    let games = shared_games.lock().unwrap();
    let steam_match = games
        .steam
        .iter()
        .find(|g| steam_game_id(g) == launch_req.id);
    let epic_match = games.epic.iter().find(|g| epic_game_id(g) == launch_req.id);
    let gog_match = games.gog.iter().find(|g| gog_game_id(g) == launch_req.id);
    let match_count = steam_match.is_some() as u8 + epic_match.is_some() as u8 + gog_match.is_some() as u8;

    let result: Result<(), String> = if match_count == 0 {
        Err(format!("game '{}' not found", launch_req.id))
    } else if match_count > 1 {
        Err(format!("game id '{}' is ambiguous across sources", launch_req.id))
    } else if let Some(game) = steam_match {
        if let Some(running) = crate::steam::get_running_steam_games()
            .into_iter()
            .find(|g| g.appid == game.appid)
        {
            crate::process::focus_window_by_pid(running.pid).map_err(|e| e.to_string())
        } else {
            let url = format!("steam://rungameid/{}", game.appid);
            std::process::Command::new("cmd")
                .args(["/c", "start", "", &url])
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    } else if let Some(game) = epic_match {
        if let Some(running) = crate::epic::get_running_epic_games()
            .into_iter()
            .find(|g| !game.app_name.is_empty() && g.app_name == game.app_name)
        {
            crate::process::focus_window_by_pid(running.pid).map_err(|e| e.to_string())
        } else {
            crate::epic::launch_epic_game(game).map_err(|e| e.to_string())
        }
    } else if let Some(game) = gog_match {
        if let Some(running) = crate::gog::get_running_gog_games()
            .into_iter()
            .find(|g| g.source_key == game.source_key)
        {
            crate::process::focus_window_by_pid(running.pid).map_err(|e| e.to_string())
        } else {
            crate::gog::launch_gog_game(game).map_err(|e| e.to_string())
        }
    } else {
        Err("internal launch resolution error".to_string())
    };
    drop(games);

    let content_type = Header::from_bytes("Content-Type", "application/json").unwrap();
    match result {
        Ok(()) => {
            let _ = request.respond(
                Response::from_string(r#"{"ok":true}"#).with_header(content_type),
            );
        }
        Err(e) => {
            let body = serde_json::json!({"ok": false, "error": e}).to_string();
            let _ = request.respond(
                Response::from_string(body)
                    .with_header(content_type)
                    .with_status_code(422),
            );
        }
    }
}

fn handle_running_games(request: tiny_http::Request) {
    let payload = build_running_games_list();
    let content_type = Header::from_bytes("Content-Type", "application/json").unwrap();
    match serde_json::to_string(&payload) {
        Ok(json) => {
            let _ = request.respond(Response::from_string(json).with_header(content_type));
        }
        Err(e) => {
            eprintln!("[webserver] /games/running serialization error: {e}");
            let _ = request
                .respond(Response::from_string("Internal Server Error").with_status_code(500));
        }
    }
}

fn handle_windows(request: tiny_http::Request) {
    let windows = crate::process::get_all_windows();
    let content_type = Header::from_bytes("Content-Type", "application/json").unwrap();
    match serde_json::to_string(&windows) {
        Ok(json) => {
            let _ = request.respond(Response::from_string(json).with_header(content_type));
        }
        Err(e) => {
            eprintln!("[webserver] /windows serialization error: {e}");
            let _ = request
                .respond(Response::from_string("Internal Server Error").with_status_code(500));
        }
    }
}

fn handle_games(request: tiny_http::Request, shared_games: &SharedGames) {
    let payload = shared_games.lock().unwrap().clone();
    let games = build_games_list(&payload);
    match serde_json::to_string(&games) {
        Ok(json) => {
            let content_type = Header::from_bytes("Content-Type", "application/json").unwrap();
            let _ = request.respond(Response::from_string(json).with_header(content_type));
        }
        Err(e) => {
            eprintln!("[webserver] /games serialization error: {e}");
            let _ = request
                .respond(Response::from_string("Internal Server Error").with_status_code(500));
        }
    }
}
