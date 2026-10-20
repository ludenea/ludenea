//! The one way game code panics are caught: [`catch_panic`] turns a panic into a
//! [`GamePanic`] that says at which tick, in which system, and where in the source it
//! happened. Native builds recover; WebAssembly aborts on a panic, so there the panic hook
//! of the host reports it instead.

use std::cell::Cell;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

/// A panic of game code: a system, a mapping, a drawer, a codec, or a default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GamePanic {
    /// The simulation's tick when it panicked: the tick a system was running from.
    pub tick: u64,
    /// The system that was running, or whose commands were applying.
    pub system: Option<&'static str>,
    /// The panic's own message.
    pub message: String,
    /// `file:line:column` of the panic. Unknown when it happened on another thread, such
    /// as one of `Parallel`'s, or when the game replaced the panic hook after the engine
    /// installed its own.
    pub location: Option<String>,
}

impl GamePanic {
    /// The message with the system and the location, without the tick: what
    /// `PauseReason::Panicked` carries next to its tick.
    pub fn detail(&self) -> String {
        let context: Vec<String> = self
            .system
            .map(|system| format!("in system `{system}`"))
            .into_iter()
            .chain(self.location.clone())
            .collect();
        if context.is_empty() {
            self.message.clone()
        } else {
            format!("{} ({})", self.message, context.join(", "))
        }
    }
}

impl fmt::Display for GamePanic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the game panicked at tick {}", self.tick)?;
        if let Some(system) = self.system {
            write!(f, " in system `{system}`")?;
        }
        write!(f, ": {}", self.message)?;
        if let Some(location) = &self.location {
            write!(f, " ({location})")?;
        }
        Ok(())
    }
}

// ponytail: the location is kept per thread, so a panic on a `Parallel` pool thread reaches
// the caller without one; forward it through `Parallel` if reports need it there too.
thread_local! {
    /// Where the last panic of this thread was raised, recorded by the engine's panic hook.
    static LOCATION: Cell<Option<String>> = const { Cell::new(None) };
}

/// Runs `f`, which runs game code at `tick`, and returns its panic instead of unwinding
/// further. The caller brings back whatever state `f` left halfway. The panic is boxed, as
/// the errors that carry it are.
pub fn catch_panic<T>(tick: u64, f: impl FnOnce() -> T) -> Result<T, Box<GamePanic>> {
    install_hook();
    LOCATION.with(Cell::take);
    catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        Box::new(GamePanic {
            tick,
            system: None,
            message: payload
                .downcast_ref::<&str>()
                .map(|message| message.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic without a message".to_string()),
            location: LOCATION.with(Cell::take),
        })
    })
}

/// Installs, once, a panic hook that records where each panic is raised for
/// `catch_panic`, then runs the hook that was there before, so the panic is still printed.
fn install_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let location = info.location().map(ToString::to_string);
            // A thread being torn down has no thread locals left: it records nothing.
            let _ = LOCATION.try_with(|cell| cell.set(location));
            previous(info);
        }));
    });
}
