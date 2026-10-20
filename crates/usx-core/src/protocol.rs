//! Messages between the game and the weather service, sent as one JSON object per
//! line over a localhost TCP socket. Kept small and versioned so either side can be
//! upgraded independently.

use crate::geo::Geodetic;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToService {
    Hello {
        game_version: String,
        protocol: u32,
    },
    /// Where the player is, so the service only fetches data for that area.
    Position {
        pos: Geodetic,
        speed_mps: f64,
    },
    /// Share of the bandwidth cap the service may use.
    Bandwidth {
        mbps: f64,
    },
    Ping {
        t: u64,
    },
    Shutdown,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromService {
    Hello {
        service_version: String,
        protocol: u32,
    },
    Pong {
        t: u64,
    },
    Status {
        text: String,
    },
}

pub fn encode<T: serde::Serialize>(m: &T) -> String {
    let mut s = serde_json::to_string(m).unwrap_or_default();
    s.push('\n');
    s
}
