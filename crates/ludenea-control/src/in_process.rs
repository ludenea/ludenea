use crate::host::Host;
use crate::protocol::{
    ClientMessage, Command, ErrorCode, GameInfo, PROTOCOL_VERSION, ProtocolError, Reply,
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

    /// The game described in the welcome.
    pub fn game(&self) -> &GameInfo {
        &self.game
    }
}

fn handshake(host: &Host, hello: ClientMessage) -> ServerMessage {
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

impl Transport for InProcess {
    fn request(&mut self, command: Command) -> Result<Reply, ProtocolError> {
        self.next_id += 1;
        self.host.handle(command)
    }
}
