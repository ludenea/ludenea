use crate::host::Host;
use crate::protocol::{
    ClientMessage, Command, ErrorCode, Event, GameInfo, PROTOCOL_VERSION, ProtocolError, Reply,
    ServerMessage,
};

/// How a client sends commands to an engine host.
pub trait Transport {
    fn request(&mut self, command: Command) -> Result<Reply, ProtocolError>;
}

/// A host in the same process; values are passed without serializing.
pub struct InProcess {
    host: Host,
    game: GameInfo,
    next_id: u64,
}

impl InProcess {
    /// Performs the handshake; a client speaking another protocol version is refused.
    pub fn connect(host: Host, protocol_version: u32) -> Result<InProcess, ProtocolError> {
        match handshake(&host, ClientMessage::Hello { protocol_version }) {
            ServerMessage::Welcome { game, .. } => Ok(InProcess {
                host,
                game,
                next_id: 0,
            }),
            ServerMessage::Refused { error } => Err(error),
            other => unreachable!("a handshake answers Welcome or Refused, not {other:?}"),
        }
    }

    /// Lets the host work through time that passed: resimulation and play. Returns the
    /// events since the last call.
    pub fn advance(&mut self, elapsed_seconds: f64) -> Vec<Event> {
        self.host.advance(elapsed_seconds)
    }

    /// The events queued by commands since the last `advance` or `drain_events`.
    pub fn drain_events(&mut self) -> Vec<Event> {
        self.host.drain_events()
    }

    /// The game described in the welcome.
    pub fn game(&self) -> &GameInfo {
        &self.game
    }
}

/// Answers a message that arrives before the connection is established.
pub(crate) fn handshake(host: &Host, hello: ClientMessage) -> ServerMessage {
    match hello {
        ClientMessage::Hello { protocol_version } if protocol_version == PROTOCOL_VERSION => {
            ServerMessage::Welcome {
                protocol_version: PROTOCOL_VERSION,
                engine_version: env!("CARGO_PKG_VERSION").to_string(),
                game: host.game_info(),
            }
        }
        ClientMessage::Hello { protocol_version } => ServerMessage::Refused {
            error: ProtocolError::new(
                ErrorCode::VersionMismatch,
                format!(
                    "client speaks protocol {protocol_version}, engine speaks {PROTOCOL_VERSION}"
                ),
            ),
        },
        ClientMessage::Request { .. } => ServerMessage::Refused {
            error: ProtocolError::new(ErrorCode::NotConnected, "send Hello before any request"),
        },
    }
}

/// Answers one protocol line of a text transport: the handshake until `connected`, then
/// requests. A line that is not a message gets a `Refused` or an error response.
// The text transports: standard input and output natively, the Web Worker in the browser.
#[cfg(any(
    not(target_family = "wasm"),
    all(target_arch = "wasm32", target_os = "unknown")
))]
pub(crate) fn answer(host: &mut Host, connected: &mut bool, line: &str) -> ServerMessage {
    match serde_json::from_str::<ClientMessage>(line) {
        Ok(ClientMessage::Request { id, command }) if *connected => ServerMessage::Response {
            id,
            result: host.handle(command),
        },
        Ok(message) => {
            let reply = handshake(host, message);
            *connected = matches!(reply, ServerMessage::Welcome { .. });
            reply
        }
        Err(error) => malformed(*connected, line, &error),
    }
}

#[cfg(any(
    not(target_family = "wasm"),
    all(target_arch = "wasm32", target_os = "unknown")
))]
fn malformed(connected: bool, line: &str, error: &serde_json::Error) -> ServerMessage {
    let id = serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| value.get("id")?.as_u64());
    match (connected, id) {
        (false, _) => ServerMessage::Refused {
            error: ProtocolError::new(ErrorCode::NotConnected, "send Hello before any request"),
        },
        (true, Some(id)) => ServerMessage::Response {
            id,
            result: Err(ProtocolError::new(
                ErrorCode::InvalidCommand,
                format!("cannot read this request: {error}"),
            )),
        },
        (true, None) => ServerMessage::Refused {
            error: ProtocolError::new(
                ErrorCode::InvalidCommand,
                format!("cannot read this message: {error}"),
            ),
        },
    }
}

impl Transport for InProcess {
    fn request(&mut self, command: Command) -> Result<Reply, ProtocolError> {
        self.next_id += 1;
        self.host.handle(command)
    }
}
