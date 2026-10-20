//! US Explorer weather service.
//!
//! Runs beside the game as its own process. In Phase 0 it only handshakes and answers
//! pings; later phases add NOAA feeds (HRRR, METAR, NEXRAD, GOES) here so decoding
//! never costs the game frames.
//!
//! The game starts it by running its own executable with `--weather-service`, so there is
//! one program to ship. Prints `USX_WEATHER_PORT=<port>` once listening and exits when
//! the game disconnects.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
use usx_core::protocol::{self, FromService, PROTOCOL_VERSION, ToService};

/// Run the service with the given command-line arguments (`--port N`, `--version`).
pub fn run(args: impl IntoIterator<Item = String>) {
    let mut port = 0u16;
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().and_then(|p| p.parse().ok()).unwrap_or(0),
            "--version" => {
                println!("usx-weather {}", usx_core::VERSION);
                return;
            }
            _ => {}
        }
    }
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("usx-weather: can't listen on port {port}: {e}");
            std::process::exit(1);
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    println!("USX_WEATHER_PORT={port}");
    let _ = std::io::stdout().flush();

    // Give the game a minute to connect, then give up so no orphan stays around.
    let _ = listener.set_nonblocking(true);
    let start = std::time::Instant::now();
    let stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() > Duration::from_secs(60) {
                    eprintln!("usx-weather: no game connected; exiting");
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                eprintln!("usx-weather: accept failed: {e}");
                return;
            }
        }
    };
    serve(stream);
}

fn serve(stream: TcpStream) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let mut out = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<ToService>(&line) else {
            continue;
        };
        let reply = match msg {
            ToService::Hello { protocol: p, .. } => {
                Some(FromService::Hello { service_version: usx_core::VERSION.to_string(), protocol: PROTOCOL_VERSION.min(p) })
            }
            ToService::Ping { t } => Some(FromService::Pong { t }),
            ToService::Bandwidth { mbps } => {
                usx_core::net::limiter().set_mbps(mbps);
                None
            }
            ToService::Position { .. } => None,
            ToService::Shutdown => break,
        };
        if let Some(r) = reply
            && out.write_all(protocol::encode(&r).as_bytes()).is_err()
        {
            break;
        }
    }
}
