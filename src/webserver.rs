use std::sync::{Arc, Mutex};
use std::thread;
use tiny_http::{Header, Response, Server};

use crate::epic::EpicGame;
use crate::gog::GogGame;
use crate::steam::SteamGame;
use crate::steam::RunningSteamGame;
use crate::epic::RunningEpicGame;
use crate::gog::RunningGogGame;

const BIND_ADDR: &str = "0.0.0.0:7878";

#[derive(serde::Serialize, Default, Clone)]
pub struct GamesPayload {
    pub steam: Vec<SteamGame>,
    pub epic: Vec<EpicGame>,
    pub gog: Vec<GogGame>,
}

#[derive(serde::Serialize)]
struct RunningGamesPayload {
    steam: Vec<RunningSteamGame>,
    epic: Vec<RunningEpicGame>,
    gog: Vec<RunningGogGame>,
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
    source: String,
    id: String,
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
                Response::from_string(r#"{"error":"invalid JSON body, expected {\"source\":\"steam|epic|gog\",\"id\":\"...\"}"}"}"#)
                    .with_status_code(400),
            );
            return;
        }
    };

    let games = shared_games.lock().unwrap();
    let result: Result<(), String> = match launch_req.source.as_str() {
        "steam" => {
            if let Some(game) = games.steam.iter().find(|g| g.appid == launch_req.id) {
                let url = format!("steam://rungameid/{}", game.appid);
                std::process::Command::new("cmd")
                    .args(["/c", "start", "", &url])
                    .spawn()
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            } else {
                Err(format!("steam game '{}' not found", launch_req.id))
            }
        }
        "epic" => {
            if let Some(game) = games.epic.iter().find(|g| g.app_name == launch_req.id) {
                crate::epic::launch_epic_game(game).map_err(|e| e.to_string())
            } else {
                Err(format!("epic game '{}' not found", launch_req.id))
            }
        }
        "gog" => {
            if let Some(game) = games.gog.iter().find(|g| g.source_key == launch_req.id) {
                crate::gog::launch_gog_game(game).map_err(|e| e.to_string())
            } else {
                Err(format!("gog game '{}' not found", launch_req.id))
            }
        }
        other => Err(format!("unknown source '{other}', expected steam/epic/gog")),
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
    let payload = RunningGamesPayload {
        steam: crate::steam::get_running_steam_games(),
        epic: crate::epic::get_running_epic_games(),
        gog: crate::gog::get_running_gog_games(),
    };
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

fn handle_games(request: tiny_http::Request, shared_games: &SharedGames) {
    let payload = shared_games.lock().unwrap().clone();
    match serde_json::to_string(&payload) {
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
