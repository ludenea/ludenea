//! The native message loop: matchbox_socket on webrtc-rs, on a thread of its own.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use async_tungstenite::tungstenite;
use matchbox_socket::{Error, MessageLoopFuture, SignalingError};

use crate::settings::LinkSettings;

/// When the link opened, to bound the wait for its id.
pub(crate) struct Clock(Instant);

impl Clock {
    pub(crate) fn start() -> Clock {
        #[expect(
            clippy::disallowed_methods,
            reason = "network pacing, never feeds the simulation"
        )]
        let now = Instant::now();
        Clock(now)
    }

    pub(crate) fn elapsed(&self) -> Duration {
        #[expect(
            clippy::disallowed_methods,
            reason = "network pacing, never feeds the simulation"
        )]
        let now = Instant::now();
        now.duration_since(self.0)
    }
}

/// Runs `future` on a thread named `ludenea-webrtc` until it ends, and delivers how it ended:
/// `Ok` without an error, else the reason, a panic's included.
pub(crate) fn run(
    future: MessageLoopFuture,
    settings: LinkSettings,
) -> Receiver<Result<(), String>> {
    let (sender, receiver) = mpsc::channel();
    let thread_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("ludenea-webrtc".to_owned())
        .spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(|| futures::executor::block_on(future)));
            let result = match outcome {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(reason(&error, &settings)),
                Err(panic) => Err(format!(
                    "the WebRTC link to {} panicked: {}",
                    settings.signaling(),
                    panic_message(panic.as_ref())
                )),
            };
            // A link already dropped needs no result.
            let _ = thread_sender.send(result);
        });
    if let Err(error) = spawned {
        let _ = sender.send(Err(format!("cannot start the WebRTC thread: {error}")));
    }
    receiver
}

fn panic_message(panic: &(dyn Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("a panic without a message")
}

/// Why the loop ended with `error`.
fn reason(error: &Error, settings: &LinkSettings) -> String {
    let signaling = settings.signaling();
    match error {
        Error::ConnectionFailed(SignalingError::WebSocket(failure)) => {
            if let tungstenite::Error::Http(response) = failure
                && let Some(refusal) = refusal(response.status().as_u16(), settings)
            {
                return refusal;
            }
            format!("cannot reach the signaling server at {signaling}: {failure}")
        }
        Error::ConnectionFailed(failure) => {
            format!("cannot reach the signaling server at {signaling}: {failure}")
        }
        Error::Disconnected(failure) => {
            format!("lost the signaling server at {signaling}: {failure}")
        }
    }
}

/// What the HTTP `status` of a refused room means, for the statuses the Ludenea listener
/// answers.
fn refusal(status: u16, settings: &LinkSettings) -> Option<String> {
    let signaling = settings.signaling();
    let code = settings.code();
    Some(match status {
        404 => format!(
            "no room {code} at {signaling}: check the code, or there is no Ludenea listener there"
        ),
        409 if settings.host().is_some() => {
            format!("room {code} is in use at {signaling}: host again for a new code")
        }
        409 => format!("room {code} is full"),
        410 => format!("the host of room {code} left"),
        400 => format!("the signaling server at {signaling} refused the room code {code}"),
        403 => format!("the signaling server at {signaling} refused this origin"),
        429 => format!(
            "the signaling server at {signaling} refused: too many rooms or attempts from this \
             address"
        ),
        503 => format!("the signaling server at {signaling} holds too many rooms"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use ludenea_net::RoomCode;

    use super::*;

    const SIGNALING: &str = "ws://127.0.0.1:3536/rooms";

    fn settings(host: Option<u8>) -> LinkSettings {
        LinkSettings::new(SIGNALING, RoomCode::parse("ABC234").unwrap(), host, &[]).unwrap()
    }

    fn refused(status: u16, host: Option<u8>) -> String {
        let response = tungstenite::http::Response::builder()
            .status(status)
            .body(None)
            .unwrap();
        let error = Error::ConnectionFailed(SignalingError::WebSocket(tungstenite::Error::Http(
            Box::new(response),
        )));
        reason(&error, &settings(host))
    }

    #[test]
    fn each_refusal_of_the_listener_names_its_cause() {
        let cases = [
            (
                404,
                None,
                "no room ABC234 at ws://127.0.0.1:3536/rooms: check the code, or there is no \
                 Ludenea listener there",
            ),
            (
                409,
                Some(2),
                "room ABC234 is in use at ws://127.0.0.1:3536/rooms: host again for a new code",
            ),
            (409, None, "room ABC234 is full"),
            (410, None, "the host of room ABC234 left"),
            (
                400,
                None,
                "the signaling server at ws://127.0.0.1:3536/rooms refused the room code ABC234",
            ),
            (
                403,
                None,
                "the signaling server at ws://127.0.0.1:3536/rooms refused this origin",
            ),
            (
                429,
                Some(2),
                "the signaling server at ws://127.0.0.1:3536/rooms refused: too many rooms or \
                 attempts from this address",
            ),
            (
                503,
                Some(2),
                "the signaling server at ws://127.0.0.1:3536/rooms holds too many rooms",
            ),
            (
                500,
                None,
                "cannot reach the signaling server at ws://127.0.0.1:3536/rooms: HTTP error: 500 \
                 Internal Server Error",
            ),
        ];
        for (status, host, expected) in cases {
            assert_eq!(refused(status, host), expected, "{status}");
        }
    }

    #[test]
    fn a_lost_server_is_named() {
        let error = Error::Disconnected(SignalingError::StreamExhausted);
        assert_eq!(
            reason(&error, &settings(None)),
            "lost the signaling server at ws://127.0.0.1:3536/rooms: The stream is exhausted"
        );
    }

    #[test]
    fn a_panic_message_is_read_from_either_payload() {
        assert_eq!(panic_message(&"static"), "static");
        assert_eq!(panic_message(&String::from("owned")), "owned");
        assert_eq!(panic_message(&7), "a panic without a message");
    }
}
