//! WebSocket plumbing shared by the Host and the Player: connecting (TLS through the system's library for `wss://`), timeouts, and turning the Coordinator's frames
//! into something to read. The Coordinator is untrusted for secrecy and authenticity; everything that matters is checked inside the end-to-end channel.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::Duration;

use coa_control_proto::coord::{self, ErrorCode, Frame};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

pub type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

/// Why a control connection failed, in terms the interface can explain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// The Coordinator said the realm's Manager is not connected.
    HostOffline,
    HostBusy,
    RateLimited,
    /// The Coordinator or the peer did not prove who it is, or sent something that is not the protocol.
    Auth(String),
    Protocol(String),
    Io(String),
    Timeout,
}

impl LinkError {
    /// A stable code for the interface.
    pub fn code(&self) -> &'static str {
        match self {
            LinkError::HostOffline => "host_offline",
            LinkError::HostBusy => "host_busy",
            LinkError::RateLimited => "rate_limited",
            LinkError::Auth(_) => "host_not_verified",
            LinkError::Protocol(_) => "protocol",
            LinkError::Io(_) => "unreachable",
            LinkError::Timeout => "timeout",
        }
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::HostOffline => write!(f, "The server's Manager is not online right now."),
            LinkError::HostBusy => {
                write!(f, "The server's Manager is busy; try again in a moment.")
            }
            LinkError::RateLimited => write!(f, "Too many attempts; wait a minute."),
            LinkError::Auth(m) => write!(f, "The server could not be verified: {m}"),
            LinkError::Protocol(m) => write!(f, "The connection broke the protocol: {m}"),
            LinkError::Io(m) => write!(f, "The connection failed: {m}"),
            LinkError::Timeout => write!(f, "The server did not answer in time."),
        }
    }
}

impl std::error::Error for LinkError {}

impl From<coa_control_proto::ControlError> for LinkError {
    fn from(e: coa_control_proto::ControlError) -> Self {
        match e {
            coa_control_proto::ControlError::Auth(m) => LinkError::Auth(m),
            other => LinkError::Protocol(other.to_string()),
        }
    }
}

pub fn from_error_frame(code: ErrorCode, message: &str) -> LinkError {
    match code {
        ErrorCode::HostOffline | ErrorCode::UnknownRealm => LinkError::HostOffline,
        ErrorCode::HostBusy => LinkError::HostBusy,
        ErrorCode::RateLimited => LinkError::RateLimited,
        ErrorCode::Timeout => LinkError::Timeout,
        ErrorCode::BadHello => LinkError::Auth(message.to_string()),
        ErrorCode::TooLarge | ErrorCode::Internal => LinkError::Protocol(message.to_string()),
    }
}

/// `http(s)://host[:port]` -> `ws(s)://host[:port]` + `path`. The Registry's address is the Coordinator's address (one front door).
pub fn coordinator_url(registry_url: &str, path: &str) -> Option<String> {
    let base = registry_url.trim().trim_end_matches('/');
    let rest = base
        .strip_prefix("https://")
        .map(|r| format!("wss://{r}"))
        .or_else(|| base.strip_prefix("http://").map(|r| format!("ws://{r}")))?;
    Some(format!("{rest}{path}"))
}

pub fn relay_url(registry_url: &str, path: &str) -> Option<String> {
    coordinator_url(registry_url, path)
}

pub fn connect(url: &str, read_timeout: Duration) -> Result<Ws, LinkError> {
    let (mut ws, _) =
        tungstenite::connect(url).map_err(|e| LinkError::Io(clean(&e.to_string())))?;
    set_timeouts(&mut ws, read_timeout);
    Ok(ws)
}

fn clean(s: &str) -> String {
    s.chars().take(160).collect()
}

pub fn set_timeouts(ws: &mut Ws, read: Duration) {
    let tcp = match ws.get_mut() {
        MaybeTlsStream::Plain(t) => Some(&*t),
        MaybeTlsStream::NativeTls(t) => Some(t.get_ref()),
        _ => None,
    };
    if let Some(t) = tcp {
        let _ = t.set_read_timeout(Some(read));
        let _ = t.set_write_timeout(Some(Duration::from_secs(15)));
        let _ = t.set_nodelay(true);
    }
}

/// One message with the read timeout applied: `Ok(None)` when nothing arrived within it.
pub fn poll(ws: &mut Ws) -> Result<Option<Message>, LinkError> {
    match ws.read() {
        Ok(m) => Ok(Some(m)),
        Err(tungstenite::Error::Io(e))
            if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
        {
            Ok(None)
        }
        Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
            Err(LinkError::Io("the connection was closed".into()))
        }
        Err(e) => Err(LinkError::Io(clean(&e.to_string()))),
    }
}

/// The next control frame within `total`; transport pings and pongs are skipped. An error frame from the Coordinator becomes the error.
pub fn next_frame(ws: &mut Ws, total: Duration) -> Result<Frame, LinkError> {
    let end = std::time::Instant::now() + total;
    loop {
        if std::time::Instant::now() >= end {
            return Err(LinkError::Timeout);
        }
        match poll(ws)? {
            None => continue,
            Some(Message::Text(t)) => {
                let frame = coord::parse_text(t.as_str()).map_err(LinkError::from)?;
                if let Frame::Error { code, message } = &frame {
                    return Err(from_error_frame(*code, message));
                }
                return Ok(frame);
            }
            Some(Message::Ping(_) | Message::Pong(_)) => continue,
            Some(Message::Close(_)) => {
                return Err(LinkError::Io("the connection was closed".into()))
            }
            Some(_) => {
                return Err(LinkError::Protocol(
                    "a binary frame arrived where a control frame was expected".into(),
                ))
            }
        }
    }
}

pub fn send_text(ws: &mut Ws, frame: &Frame) -> Result<(), LinkError> {
    ws.send(Message::Text(coord::to_text(frame).into()))
        .map_err(|e| LinkError::Io(clean(&e.to_string())))
}

pub fn send_binary(ws: &mut Ws, data: Vec<u8>) -> Result<(), LinkError> {
    ws.send(Message::Binary(data.into()))
        .map_err(|e| LinkError::Io(clean(&e.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_coordinator_address_follows_the_registrys() {
        assert_eq!(
            coordinator_url("https://registry.example/", "/coord/v1/host").unwrap(),
            "wss://registry.example/coord/v1/host"
        );
        assert_eq!(
            coordinator_url("http://127.0.0.1:8080", "/coord/v1/player?realm=x").unwrap(),
            "ws://127.0.0.1:8080/coord/v1/player?realm=x"
        );
        assert!(
            coordinator_url("ftp://x", "/").is_none()
                && coordinator_url("registry.example", "/").is_none()
        );
    }

    #[test]
    fn coordinator_refusals_become_messages_a_player_can_act_on() {
        assert_eq!(
            from_error_frame(ErrorCode::HostOffline, "").code(),
            "host_offline"
        );
        assert_eq!(
            from_error_frame(ErrorCode::UnknownRealm, "").code(),
            "host_offline"
        );
        assert_eq!(
            from_error_frame(ErrorCode::HostBusy, "").code(),
            "host_busy"
        );
        assert_eq!(
            from_error_frame(ErrorCode::RateLimited, "").code(),
            "rate_limited"
        );
        assert_eq!(
            LinkError::HostOffline.to_string(),
            "The server's Manager is not online right now."
        );
    }
}
