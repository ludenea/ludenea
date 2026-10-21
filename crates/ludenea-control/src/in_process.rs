use crate::host::Host;
use crate::protocol::{
    ClientMessage, Command, ErrorCode, Event, GameInfo, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION,
    ProtocolError, Reply, ServerMessage,
};

/// What to do about a refused handshake: client and engine come from different checkouts.
const REBUILD: &str =
    "rebuild the editor (pnpm -C editor build) and the game from the same checkout";

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
    /// Performs the handshake with this build's fingerprint; a client speaking another
    /// protocol version is refused.
    pub fn connect(host: Host, protocol_version: u32) -> Result<InProcess, ProtocolError> {
        let hello = ClientMessage::Hello {
            protocol_version,
            protocol_fingerprint: PROTOCOL_FINGERPRINT,
        };
        match handshake(&host, hello) {
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
        ClientMessage::Hello {
            protocol_version, ..
        } if protocol_version != PROTOCOL_VERSION => ServerMessage::Refused {
            error: version_mismatch(protocol_version),
        },
        ClientMessage::Hello {
            protocol_fingerprint,
            ..
        } if protocol_fingerprint != PROTOCOL_FINGERPRINT => ServerMessage::Refused {
            error: ProtocolError::new(
                ErrorCode::VersionMismatch,
                format!(
                    "client speaks another revision of protocol {PROTOCOL_VERSION} (fingerprint \
                     {:016x}, engine {:016x}); {REBUILD}",
                    protocol_fingerprint.0, PROTOCOL_FINGERPRINT.0
                ),
            ),
        },
        ClientMessage::Hello { .. } => ServerMessage::Welcome {
            protocol_version: PROTOCOL_VERSION,
            protocol_fingerprint: PROTOCOL_FINGERPRINT,
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            game: host.game_info(),
        },
        ClientMessage::Request { .. } => ServerMessage::Refused {
            error: ProtocolError::new(ErrorCode::NotConnected, "send Hello before any request"),
        },
    }
}

fn version_mismatch(protocol_version: u32) -> ProtocolError {
    ProtocolError::new(
        ErrorCode::VersionMismatch,
        format!(
            "client speaks protocol {protocol_version}, engine speaks {PROTOCOL_VERSION}; {REBUILD}"
        ),
    )
}

/// The text transports: standard input and output natively, the Web Worker in the browser.
#[cfg(any(
    not(target_family = "wasm"),
    all(target_arch = "wasm32", target_os = "unknown")
))]
pub(crate) mod text {
    use super::{handshake, version_mismatch};
    use crate::host::Host;
    use crate::protocol::{
        ClientMessage, ErrorCode, PROTOCOL_VERSION, ProtocolError, ServerMessage, unreadable_hello,
    };
    #[cfg(any(test, all(target_arch = "wasm32", target_os = "unknown")))]
    use crate::protocol::{Command, Reply};

    /// Answers one protocol line of a text transport: the handshake until `connected`, then
    /// requests. A line that is not a message gets a `Refused` or an error response. The
    /// events a request causes come before its response, so a client that has the response
    /// has every event of the command, and none comes after the response to a later one.
    pub(crate) fn answer(host: &mut Host, connected: &mut bool, line: &str) -> Vec<ServerMessage> {
        match serde_json::from_str::<ClientMessage>(line) {
            Ok(ClientMessage::Request { id, command }) if *connected => {
                let result = host.handle(command);
                let mut messages: Vec<ServerMessage> = host
                    .drain_events()
                    .into_iter()
                    .map(|event| ServerMessage::Event { event })
                    .collect();
                messages.push(ServerMessage::Response { id, result });
                messages
            }
            Ok(message) => {
                let reply = handshake(host, message);
                *connected = matches!(reply, ServerMessage::Welcome { .. });
                vec![reply]
            }
            Err(error) => vec![malformed(*connected, line, &error)],
        }
    }

    /// A host behind a text transport that also hands out the raw frame text: the messages
    /// `answer` gives, plus `render_frame_json`, the encoding of `get_render_frame` reduced to
    /// its frame. The events that call causes stay in the host's queue, so the next `handle`
    /// or `advance` delivers them first, in order, as for any command. The Web Worker's
    /// transport; the standard streams answer with `answer` alone.
    #[cfg(any(test, all(target_arch = "wasm32", target_os = "unknown")))]
    pub(crate) struct TextHost {
        pub(crate) host: Host,
        connected: bool,
    }

    #[cfg(any(test, all(target_arch = "wasm32", target_os = "unknown")))]
    impl TextHost {
        pub(crate) fn new(host: Host) -> TextHost {
            TextHost {
                host,
                connected: false,
            }
        }

        /// The messages `answer` gives for `line`; nothing for a blank line.
        pub(crate) fn handle(&mut self, line: &str) -> Vec<ServerMessage> {
            if line.trim().is_empty() {
                return Vec::new();
            }
            answer(&mut self.host, &mut self.connected, line)
        }

        /// The events of `elapsed_seconds` of play and resimulation, after those queued
        /// before.
        pub(crate) fn advance(&mut self, elapsed_seconds: f64) -> Vec<ServerMessage> {
            self.host
                .advance(elapsed_seconds)
                .into_iter()
                .map(|event| ServerMessage::Event { event })
                .collect()
        }

        /// Runs `get_render_frame` as a request would, refused before the hello as `answer`
        /// refuses a request (`NotConnected`), and returns the frame as JSON.
        pub(crate) fn render_frame_json(
            &mut self,
            debug_layers: Vec<String>,
        ) -> Result<String, ProtocolError> {
            if !self.connected {
                return Err(ProtocolError::new(
                    ErrorCode::NotConnected,
                    "send Hello before any request",
                ));
            }
            match self.host.handle(Command::GetRenderFrame { debug_layers })? {
                Reply::RenderFrame { frame } => {
                    Ok(serde_json::to_string(&frame).expect("a render frame serializes"))
                }
                other => unreachable!("GetRenderFrame answers RenderFrame, not {other:?}"),
            }
        }
    }

    fn malformed(connected: bool, line: &str, error: &serde_json::Error) -> ServerMessage {
        let id = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|value| value.get("id")?.as_u64());
        match (connected, id) {
            (false, _) => ServerMessage::Refused {
                error: match unreadable_hello(line) {
                    Some(Some(version)) if version != PROTOCOL_VERSION => version_mismatch(version),
                    Some(_) => ProtocolError::new(
                        ErrorCode::InvalidCommand,
                        format!(
                            "cannot read this hello: {error}; a hello needs protocol_version and \
                             protocol_fingerprint"
                        ),
                    ),
                    None => {
                        ProtocolError::new(ErrorCode::NotConnected, "send Hello before any request")
                    }
                },
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
}

impl Transport for InProcess {
    fn request(&mut self, command: Command) -> Result<Reply, ProtocolError> {
        self.next_id += 1;
        self.host.handle(command)
    }
}

#[cfg(test)]
mod tests {
    use super::text::{TextHost, answer};
    use crate::host::Host;
    use crate::protocol::{
        ClientMessage, Command, Event, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION, Reply, ServerMessage,
    };
    use crate::session::Document;

    fn send(
        host: &mut Host,
        connected: &mut bool,
        id: u64,
        command: Command,
    ) -> Vec<ServerMessage> {
        let line = serde_json::json!({ "type": "request", "id": id, "command": command });
        answer(host, connected, &line.to_string())
    }

    fn heads(messages: &[ServerMessage]) -> Vec<u64> {
        messages
            .iter()
            .filter_map(|message| match message {
                ServerMessage::Event {
                    event: Event::TimelineChanged { timeline },
                } => Some(timeline.head),
                _ => None,
            })
            .collect()
    }

    /// A client that sends `load_session` right after `load_scene` answered must not see the
    /// scene's timeline (head 0) after the session's reply: it would take it for the session's.
    #[test]
    fn a_response_comes_after_the_events_its_command_caused() {
        let mut host = Host::new(boxes::app).unwrap();
        let mut connected = false;
        let hello = ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            protocol_fingerprint: PROTOCOL_FINGERPRINT,
        };
        let welcome = answer(
            &mut host,
            &mut connected,
            &serde_json::json!(hello).to_string(),
        );
        assert!(matches!(welcome[..], [ServerMessage::Welcome { .. }]));
        let scene = boxes::generate_scene(30, 7);
        let load_scene = || Command::LoadScene {
            scene: scene.clone(),
            assets: Vec::new(),
            document: Document::Scene,
        };
        send(&mut host, &mut connected, 1, load_scene());
        let stepped = send(
            &mut host,
            &mut connected,
            2,
            Command::Step {
                ticks: 5,
                record_hashes: false,
            },
        );
        let exported = match &send(&mut host, &mut connected, 3, Command::ExportSession)[..] {
            [
                ..,
                ServerMessage::Response {
                    result: Ok(reply), ..
                },
            ] => reply.clone(),
            other => panic!("{other:?}"),
        };
        host.advance(0.0);

        let scene_loaded = send(&mut host, &mut connected, 4, load_scene());
        assert!(
            matches!(
                scene_loaded.last(),
                Some(ServerMessage::Response {
                    id: 4,
                    result: Ok(Reply::Loaded { .. })
                })
            ),
            "{scene_loaded:?}"
        );
        assert_eq!(heads(&scene_loaded), [0_u64]);
        let Reply::Session {
            scene,
            saved_scene,
            inputs,
            edits,
            undo,
            hashes,
            assets,
            saved_assets,
            branches,
            ..
        } = exported
        else {
            panic!("{exported:?}");
        };
        let load_session = Command::LoadSession {
            scene,
            saved_scene,
            inputs,
            edits,
            undo,
            expected: hashes,
            assets,
            saved_assets,
            branches,
            document: Document::Scene,
            keeps_assets: false,
        };
        let session_loaded = send(&mut host, &mut connected, 5, load_session);
        assert!(
            matches!(
                session_loaded.last(),
                Some(ServerMessage::Response {
                    id: 5,
                    result: Ok(Reply::Loaded { .. })
                })
            ),
            "{session_loaded:?}"
        );
        assert_eq!(heads(&session_loaded), heads(&stepped));
        // Resimulation reports the session's timeline; none is the scene's, of head 0.
        let later: Vec<ServerMessage> = host
            .advance(0.0)
            .into_iter()
            .map(|event| ServerMessage::Event { event })
            .collect();
        assert_eq!(heads(&later), heads(&stepped));
    }

    fn hello() -> String {
        serde_json::json!(ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            protocol_fingerprint: PROTOCOL_FINGERPRINT,
        })
        .to_string()
    }

    #[test]
    fn the_raw_frame_is_refused_before_the_hello_as_a_request_is() {
        let mut text = TextHost::new(Host::new(boxes::app).unwrap());
        let refused = text.render_frame_json(Vec::new()).unwrap_err();
        let request = serde_json::json!({
            "type": "request",
            "id": 1,
            "command": Command::GetRenderFrame { debug_layers: Vec::new() },
        });
        let ServerMessage::Refused { error } = &text.handle(&request.to_string())[0] else {
            panic!("a request before the hello is refused");
        };
        assert_eq!(&refused, error);
        assert!(matches!(
            &text.handle(&hello())[..],
            [ServerMessage::Welcome { .. }]
        ));
        let frame: serde_json::Value =
            serde_json::from_str(&text.render_frame_json(Vec::new()).unwrap()).unwrap();
        assert_eq!(frame["tick"], 0);
    }

    #[test]
    fn events_around_the_raw_frame_come_with_the_next_call_in_order() {
        let mut text = TextHost::new(Host::new(boxes::app).unwrap());
        text.handle(&hello());
        let load = || Command::LoadScene {
            scene: boxes::generate_scene(5, 7),
            assets: Vec::new(),
            document: Document::Scene,
        };
        // The events a load queues, from a twin host.
        let mut twin = Host::new(boxes::app).unwrap();
        twin.handle(load()).unwrap();
        let queued = twin.drain_events();
        // A command run on the host directly leaves its events queued, as a command the raw
        // frame runs would.
        text.host.handle(load()).unwrap();
        text.render_frame_json(Vec::new()).unwrap();
        // The next request's reply holds them, in the order they were queued, before its
        // response; nothing comes twice.
        let request =
            serde_json::json!({ "type": "request", "id": 9, "command": Command::GetHash });
        let replied = text.handle(&request.to_string());
        let events: Vec<ServerMessage> = queued
            .into_iter()
            .map(|event| ServerMessage::Event { event })
            .collect();
        assert!(!events.is_empty());
        assert_eq!(replied[..replied.len() - 1], events[..]);
        assert!(matches!(
            replied.last(),
            Some(ServerMessage::Response {
                id: 9,
                result: Ok(Reply::Hash { .. })
            })
        ));
        assert!(text.advance(0.0).is_empty());
    }
}
