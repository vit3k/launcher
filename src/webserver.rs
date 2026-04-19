use std::thread;
use tiny_http::{Response, Server};

const BIND_ADDR: &str = "0.0.0.0:7878";

pub fn start() {
    thread::Builder::new()
        .name("webserver".into())
        .spawn(run)
        .expect("failed to spawn webserver thread");
}

fn run() {
    let server = match Server::http(BIND_ADDR) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[webserver] failed to bind {BIND_ADDR}: {e}");
            return;
        }
    };
    eprintln!("[webserver] listening on http://{BIND_ADDR}");

    for request in server.incoming_requests() {
        let url = request.url().split('?').next().unwrap_or("").to_string();
        match url.trim_end_matches('/') {
            "/suspend" => handle_suspend(request),
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
