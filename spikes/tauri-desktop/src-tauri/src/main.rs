use rand::{distributions::Uniform, Rng};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct AppDiagnostics {
    app: &'static str,
    version: &'static str,
    profile: &'static str,
    platform: &'static str,
    arch: &'static str,
}

#[tauri::command]
fn app_diagnostics() -> AppDiagnostics {
    AppDiagnostics {
        app: "aethos-tauri-spike",
        version: env!("CARGO_PKG_VERSION"),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        platform: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    }
}

#[tauri::command]
fn generate_wayfarer_id_mock() -> String {
    let mut rng = rand::thread_rng();
    let dist = Uniform::new_inclusive(0_u8, 15_u8);
    let mut out = String::with_capacity(64);

    for _ in 0..64 {
        let nibble = rng.sample(dist);
        out.push(std::char::from_digit(nibble as u32, 16).unwrap_or('0'));
    }

    out
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            app_diagnostics,
            generate_wayfarer_id_mock
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    run();
}
