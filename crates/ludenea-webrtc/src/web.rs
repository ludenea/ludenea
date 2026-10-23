//! The browser's message loop: matchbox_socket on the browser's WebRTC, as a task of the page.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use matchbox_socket::{Error, MessageLoopFuture};
use wasm_bindgen::prelude::wasm_bindgen;

use crate::settings::LinkSettings;

#[wasm_bindgen]
extern "C" {
    /// `performance.now()`: milliseconds on the page's or worker's monotonic clock.
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn performance_now() -> f64;
}

/// When the link opened, to bound the wait for its id. `std::time::Instant` is not available
/// in the browser.
pub(crate) struct Clock(f64);

impl Clock {
    pub(crate) fn start() -> Clock {
        Clock(performance_now())
    }

    pub(crate) fn elapsed(&self) -> Duration {
        // Monotonic, so the difference is never negative.
        Duration::from_millis((performance_now() - self.0) as u64)
    }
}

/// Runs `future` as a task of the page until it ends, and delivers how it ended: `Ok`
/// without an error, else the reason.
pub(crate) fn run(
    future: MessageLoopFuture,
    settings: LinkSettings,
) -> Receiver<Result<(), String>> {
    let (sender, receiver) = mpsc::channel();
    wasm_bindgen_futures::spawn_local(async move {
        let result = future.await.map_err(|error| reason(&error, &settings));
        // A link already dropped needs no result.
        let _ = sender.send(result);
    });
    receiver
}

/// Why the loop ended with `error`. The browser hides the HTTP status of a refused WebSocket,
/// so every refusal reads the same.
fn reason(error: &Error, settings: &LinkSettings) -> String {
    let signaling = settings.signaling();
    match error {
        Error::ConnectionFailed(_) => format!(
            "cannot join room {} at {signaling}: the server refused or cannot be reached (an \
             unknown, full or closed room, a code in use, or no server there)",
            settings.code()
        ),
        Error::Disconnected(failure) => {
            format!("lost the signaling server at {signaling}: {failure}")
        }
    }
}
