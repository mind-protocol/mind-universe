use std::{net::SocketAddr, thread, time::Duration};
use tauri::{AppHandle, Emitter};
use universe_protocol::{AuthenticationSecret, ProtocolHello, ProtocolTransportConfig};
use universe_stream_bridge::{run_stream_bridge, FrameFlow};

/// Opens the authenticated loopback stream to a running universe-server and
/// forwards every frame to the webview as a `universe-frame` event. A webview
/// cannot raw-TCP, so this native command is the real bridge; the drain loop and
/// HMAC authentication live in `universe-stream-bridge` (unit-tested against a
/// real server). The secret is supplied by the caller and never stored.
#[tauri::command]
fn start_universe_stream(app: AppHandle, address: String, secret: String) -> Result<(), String> {
    let address: SocketAddr = address
        .parse()
        .map_err(|error| format!("invalid stream address: {error}"))?;
    let secret =
        AuthenticationSecret::new(secret.as_bytes()).map_err(|error| error.to_string())?;
    let config = ProtocolTransportConfig {
        wire_max_frame_bytes: 16 * 1024 * 1024,
        max_connections: 1,
        io_timeout: Duration::from_secs(10),
        heartbeat_interval: Duration::from_secs(2),
    };
    let hello = ProtocolHello {
        minimum_version: 0,
        maximum_version: 0,
        client_id: "mind-desktop".to_owned(),
        resume_after: None,
    };

    thread::spawn(move || {
        let sink_app = app.clone();
        let outcome = run_stream_bridge(address, &secret, hello, config, move |frame| {
            let _ = sink_app.emit("universe-frame", &frame);
            FrameFlow::Continue
        });
        let _ = app.emit("universe-stream-closed", format!("{outcome:?}"));
    });
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![start_universe_stream])
        .run(tauri::generate_context!())
        .expect("error while running Mind Desktop");
}
