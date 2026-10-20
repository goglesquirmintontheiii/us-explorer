//! Starts the weather service next to the game and keeps a socket open to it.

use crate::globe::Viewer;
use bevy::prelude::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use usx_core::protocol::{self, FromService, PROTOCOL_VERSION, ToService};

#[derive(Clone, Debug, PartialEq)]
pub enum LinkState {
    Disabled,
    Starting,
    Connected { version: String, rtt_ms: Option<f64> },
    Failed(String),
}

#[derive(Resource)]
pub struct WeatherLink {
    pub state: LinkState,
    child: Option<Child>,
    stream: Option<TcpStream>,
    rx: Option<crossbeam_channel::Receiver<FromService>>,
    setup: Option<crossbeam_channel::Receiver<Result<(Child, TcpStream), String>>>,
    last_ping: Instant,
    last_pos: Instant,
}

impl Drop for WeatherLink {
    fn drop(&mut self) {
        if let Some(s) = &mut self.stream {
            let _ = s.write_all(protocol::encode(&ToService::Shutdown).as_bytes());
        }
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn service_path() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_else(|_| "us-explorer".into())
}

fn launch() -> Result<(Child, TcpStream), String> {
    let path = service_path();
    let mut child = Command::new(&path);
    child.arg("--weather-service").stdout(Stdio::piped()).stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console window for the background service.
        child.creation_flags(0x0800_0000);
    }
    let mut child = child.spawn().map_err(|e| format!("couldn't start {}: {e}", path.display()))?;
    let out = child.stdout.take().ok_or("no stdout")?;
    let mut line = String::new();
    BufReader::new(out).read_line(&mut line).map_err(|e| e.to_string())?;
    let port: u16 =
        line.trim().strip_prefix("USX_WEATHER_PORT=").and_then(|p| p.parse().ok()).ok_or_else(|| format!("unexpected output: {line:?}"))?;
    let stream = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true);
    Ok((child, stream))
}

pub struct WeatherLinkPlugin;

impl Plugin for WeatherLinkPlugin {
    fn build(&self, app: &mut App) {
        let disabled = app.world().resource::<crate::Args>().no_weather_service;
        let setup = (!disabled).then(|| {
            let (tx, rx) = crossbeam_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send(launch());
            });
            rx
        });
        app.insert_resource(WeatherLink {
            state: if disabled { LinkState::Disabled } else { LinkState::Starting },
            child: None,
            stream: None,
            rx: None,
            setup,
            last_ping: Instant::now(),
            last_pos: Instant::now(),
        })
        .add_systems(Update, tick);
    }
}

fn send(link: &mut WeatherLink, m: &ToService) {
    if let Some(s) = &mut link.stream
        && s.write_all(protocol::encode(m).as_bytes()).is_err()
    {
        link.state = LinkState::Failed("the weather service stopped".into());
        link.stream = None;
    }
}

fn tick(mut link: ResMut<WeatherLink>, viewer: Res<Viewer>, settings: Res<crate::GameSettings>) {
    if let Some(rx) = &link.setup
        && let Ok(r) = rx.try_recv()
    {
        link.setup = None;
        match r {
            Ok((child, stream)) => {
                let (tx, rx) = crossbeam_channel::unbounded();
                if let Ok(read) = stream.try_clone() {
                    std::thread::spawn(move || {
                        for line in BufReader::new(read).lines() {
                            let Ok(line) = line else { break };
                            if let Ok(m) = serde_json::from_str::<FromService>(&line)
                                && tx.send(m).is_err()
                            {
                                break;
                            }
                        }
                    });
                }
                link.child = Some(child);
                link.stream = Some(stream);
                link.rx = Some(rx);
                send(&mut link, &ToService::Hello { game_version: usx_core::VERSION.into(), protocol: PROTOCOL_VERSION });
                send(&mut link, &ToService::Bandwidth { mbps: settings.0.bandwidth_mbps * 0.1 });
            }
            Err(e) => link.state = LinkState::Failed(e),
        }
    }
    let msgs: Vec<FromService> = link.rx.as_ref().map(|r| r.try_iter().collect()).unwrap_or_default();
    for m in msgs {
        match m {
            FromService::Hello { service_version, .. } => link.state = LinkState::Connected { version: service_version, rtt_ms: None },
            FromService::Pong { t } => {
                let now = epoch_ms();
                if let LinkState::Connected { rtt_ms, .. } = &mut link.state {
                    *rtt_ms = Some(now.saturating_sub(t) as f64);
                }
            }
            FromService::Status { .. } => {}
        }
    }
    if link.stream.is_some() {
        if link.last_ping.elapsed() > Duration::from_secs(2) {
            link.last_ping = Instant::now();
            send(&mut link, &ToService::Ping { t: epoch_ms() });
        }
        if link.last_pos.elapsed() > Duration::from_secs(1) {
            link.last_pos = Instant::now();
            let m = ToService::Position { pos: viewer.geo(), speed_mps: viewer.velocity.length() };
            send(&mut link, &m);
        }
    }
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
