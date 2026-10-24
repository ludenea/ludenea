//! An online match driven from a runtime: the launch options of a peer and the plan derived
//! from them (`OnlineOptions::plan`, one derivation for every runtime), and `OnlineMatch`,
//! which carries a `Runner`'s match over a link. Each frame, `OnlineMatch::before_frame`
//! polls the room, joins the runner once the room agreed, and gives it the packets received;
//! the runner's frame plays; `OnlineMatch::after_frame` sends the packets it made. The native
//! window, the headless peer and the web page all drive a match this way.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ludenea_app::players::Claim;
use ludenea_control::{Hash64, MatchEnd, MatchEndKind, OnlineSettings, OnlineState};
use ludenea_net::{
    END_LINGER_MICROS, Link, LinkConditions, LinkId, MatchLayout, OnlinePeer, OnlineStatus, PeerId,
    RoomCode, RoomDrop, RoomEvent, RoomRole,
};
use ludenea_webrtc::LinkSettings;

use crate::{Runner, RuntimeError};

pub use ludenea_net::RoomView;

/// The signaling listener a native peer meets the others through unless told otherwise:
/// `ludenea rooms` on this machine.
pub const DEFAULT_SIGNALING: &str = "ws://127.0.0.1:3536/rooms";

/// What this peer is in its room, as launched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnlineRole {
    /// Opens a room of `peers` peers, itself included, under `code`, or a random code.
    Host { peers: u8, code: Option<RoomCode> },
    /// Joins the room `code`.
    Join { code: RoomCode },
}

/// How a peer goes online: its role, the signaling listener's URL, the STUN servers, and the
/// conditions its own sends go through (their seed is drawn by `plan`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnlineOptions {
    pub role: OnlineRole,
    pub signaling: String,
    pub stun: Vec<String>,
    pub conditions: LinkConditions,
}

/// Everything a peer needs to open its link and its room, validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnlinePlan {
    pub role: RoomRole,
    pub code: RoomCode,
    pub link: LinkSettings,
    pub conditions: LinkConditions,
}

impl OnlineOptions {
    /// The plan of these options for 128 bits of `entropy`: bits 0 to 63 give the room code
    /// (unless the host gave one) and the seed of the conditions, bits 64 to 127 the match's
    /// identifier. Native runtimes pass a random UUID (`entropy`), the web page its own.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Online` when the signaling or a STUN URL is invalid, naming it.
    pub fn plan(&self, entropy: u128) -> Result<OnlinePlan, RuntimeError> {
        // Truncations: the low and the high 64 bits.
        let low = entropy as u64;
        let high = (entropy >> 64) as u64;
        let (role, code, host) = match &self.role {
            OnlineRole::Host { peers, code } => (
                RoomRole::Host {
                    peers: *peers,
                    match_id: high,
                },
                code.clone().unwrap_or_else(|| RoomCode::from_entropy(low)),
                Some(*peers),
            ),
            OnlineRole::Join { code } => (RoomRole::Join, code.clone(), None),
        };
        let link = LinkSettings::new(&self.signaling, code.clone(), host, &self.stun)
            .map_err(|error| RuntimeError::Online(error.to_string()))?;
        Ok(OnlinePlan {
            role,
            code,
            link,
            conditions: LinkConditions {
                seed: low,
                ..self.conditions
            },
        })
    }
}

/// 128 random bits from the operating system, for `OnlineOptions::plan`.
#[cfg(all(feature = "native", not(target_family = "wasm")))]
pub fn entropy() -> u128 {
    uuid::Uuid::new_v4().as_u128()
}

/// The WebRTC link of `settings`.
#[cfg(any(
    all(feature = "native", not(target_family = "wasm")),
    all(feature = "web", target_arch = "wasm32", target_os = "unknown")
))]
pub fn open_link(settings: LinkSettings) -> Box<dyn Link> {
    Box::new(ludenea_webrtc::WebRtcLink::open(settings))
}

/// The settings a runner joins the match of `layout` with.
pub fn settings(layout: &MatchLayout) -> OnlineSettings {
    OnlineSettings {
        local: layout.local.0,
        match_id: Hash64(layout.match_id),
        players: layout.players.iter().map(|peer| peer.0).collect(),
    }
}

/// The options and the room's status as the standalone web page exchanges them with
/// `web::WebOnline`, in JSON. Checked on every target, so their tests run natively.
#[cfg(any(test, feature = "web"))]
mod page {
    use ludenea_net::{LinkConditions, MAX_PEERS, OnlineStatus, RoomCode};
    use ludenea_webrtc::{LinkSettings, LinkSettingsError};
    use serde_json::{Map, Value, json};

    use super::{OnlineOptions, OnlineRole, settings};

    impl OnlineOptions {
        /// The options of their JSON text, as the web page gives them:
        /// `{ "role": { "host": peers } | { "join": code }, "signaling": url, "stun": [urls],
        /// "conditions": { "latency_ms", "jitter_ms", "loss_percent" } }`, every field
        /// required, in the ranges of the native command line: 2 to 4 peers, 0 to 65535 ms,
        /// and 0 to 100 percent. The URLs are checked as `plan` checks them.
        ///
        /// # Errors
        ///
        /// A refusal starts with the path of the field it names, such as `role.host: a room
        /// holds 2 to 4 peers, not 9`, or `options:` for the text as a whole.
        pub fn from_json(text: &str) -> Result<OnlineOptions, String> {
            let value: Value = serde_json::from_str(text)
                .map_err(|error| format!("options: not JSON: {error}"))?;
            let options = object(
                &value,
                "options",
                "",
                &["role", "signaling", "stun", "conditions"],
            )?;
            let role = role(field(options, "", "role")?)?;
            let signaling = string(field(options, "", "signaling")?, "signaling")?.to_owned();
            let stun = match field(options, "", "stun")? {
                Value::Array(urls) => urls
                    .iter()
                    .map(|url| string(url, "stun").map(str::to_owned))
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err("stun: must be an array of URLs".to_owned()),
            };
            // The code does not matter: only the URLs are checked.
            LinkSettings::new(&signaling, RoomCode::from_entropy(0), None, &stun).map_err(
                |error| match error {
                    LinkSettingsError::Signaling { .. } => format!("signaling: {error}"),
                    LinkSettingsError::Stun { .. } => format!("stun: {error}"),
                },
            )?;
            let conditions = field(options, "", "conditions")?;
            let conditions = object(
                conditions,
                "conditions",
                "conditions.",
                &["latency_ms", "jitter_ms", "loss_percent"],
            )?;
            let milliseconds = |name: &str, what: &str| -> Result<u32, String> {
                let path = format!("conditions.{name}");
                let value = number(field(conditions, "conditions.", name)?, &path)?;
                match u16::try_from(value) {
                    Ok(value) => Ok(u32::from(value) * 1_000),
                    Err(_) => Err(format!(
                        "{path}: a peer {what} its packets by 0 to {} ms, not {value}",
                        u16::MAX
                    )),
                }
            };
            let latency_micros = milliseconds("latency_ms", "delays")?;
            let jitter_micros = milliseconds("jitter_ms", "jitters")?;
            let loss = number(
                field(conditions, "conditions.", "loss_percent")?,
                "conditions.loss_percent",
            )?;
            let loss_per_mille = match u16::try_from(loss) {
                Ok(loss) if loss <= 100 => loss * 10,
                _ => {
                    return Err(format!(
                        "conditions.loss_percent: a peer loses 0 to 100 percent of its packets, \
                         not {loss}"
                    ));
                }
            };
            Ok(OnlineOptions {
                role,
                signaling,
                stun,
                conditions: LinkConditions {
                    latency_micros,
                    jitter_micros,
                    loss_per_mille,
                    duplicate_per_mille: 0,
                    reorder_per_mille: 0,
                    reorder_extra_micros: 0,
                    // `plan` draws it.
                    seed: 0,
                },
            })
        }
    }

    /// The JSON of `status`, `assigned` telling whether the room exists on the listener yet:
    /// `{ "state": "room", "assigned", "describe", "view" }`, `{ "state": "ready", "settings",
    /// "local" }` or `{ "state": "failed", "message" }`.
    pub fn status_json(status: &OnlineStatus, assigned: bool) -> String {
        match status {
            OnlineStatus::Room(view) => json!({
                "state": "room",
                "assigned": assigned,
                "describe": view.describe(),
                "view": {
                    "host": view.host,
                    "peers": view.peers,
                    "connected": view.connected,
                    "ready": view.ready,
                    "unmeshed": view.unmeshed,
                    "unfit": view.unfit,
                    "local_ready": view.local_ready,
                },
            }),
            OnlineStatus::Ready(layout) => json!({
                "state": "ready",
                "settings": settings(layout),
                "local": layout.local.0,
            }),
            OnlineStatus::Failed(message) => json!({ "state": "failed", "message": message }),
        }
        .to_string()
    }

    /// The role of `value`: `{ "host": peers }` or `{ "join": code }`.
    fn role(value: &Value) -> Result<OnlineRole, String> {
        let shape = || r#"role: must be { "host": peers } or { "join": code }"#.to_owned();
        let Some(role) = value.as_object().filter(|role| role.len() == 1) else {
            return Err(shape());
        };
        if let Some(peers) = role.get("host") {
            let peers = number(peers, "role.host")?;
            return match u8::try_from(peers) {
                Ok(peers) if (2..=MAX_PEERS).contains(&usize::from(peers)) => {
                    Ok(OnlineRole::Host { peers, code: None })
                }
                _ => Err(format!(
                    "role.host: a room holds 2 to {MAX_PEERS} peers, not {peers}"
                )),
            };
        }
        if let Some(code) = role.get("join") {
            let code = RoomCode::parse(string(code, "role.join")?)
                .map_err(|error| format!("role.join: {error}"))?;
            return Ok(OnlineRole::Join { code });
        }
        Err(shape())
    }

    /// `value` as an object at `path` (its fields named `prefix` and theirs) that holds no
    /// field but `known`.
    fn object<'a>(
        value: &'a Value,
        path: &str,
        prefix: &str,
        known: &[&str],
    ) -> Result<&'a Map<String, Value>, String> {
        let object = value
            .as_object()
            .ok_or_else(|| format!("{path}: must be a JSON object"))?;
        match object.keys().find(|key| !known.contains(&key.as_str())) {
            Some(key) => Err(format!("{prefix}{key}: is not an option")),
            None => Ok(object),
        }
    }

    fn field<'a>(
        object: &'a Map<String, Value>,
        prefix: &str,
        name: &str,
    ) -> Result<&'a Value, String> {
        object
            .get(name)
            .ok_or_else(|| format!("{prefix}{name}: is missing"))
    }

    fn string<'a>(value: &'a Value, path: &str) -> Result<&'a str, String> {
        value
            .as_str()
            .ok_or_else(|| format!("{path}: must be a string"))
    }

    fn number(value: &Value, path: &str) -> Result<u64, String> {
        value
            .as_u64()
            .ok_or_else(|| format!("{path}: must be a whole number"))
    }
}

#[cfg(any(test, feature = "web"))]
pub use page::status_json;

/// The most links and reasons of dropped room messages `OnlineMatch::messages` warns about:
/// a hostile signaling server can rotate through any number of links.
const MAX_WARNED: usize = 64;

/// Where an online match stands, as `OnlineMatch::before_frame` reports it.
#[derive(Clone, Debug, PartialEq)]
pub enum OnlinePhase {
    /// The room has not agreed yet; what it waits for.
    Room(RoomView),
    /// The link has its id: the room exists on the listener from now on, so a host shows its
    /// code. Reported once, in place of a `Room`.
    Assigned,
    /// The runner joined the match, which connects, runs, or is left.
    Playing,
    /// The match ended, as each call after reports.
    Ended(MatchEnd),
    /// The room cannot give this peer a match, or the runner cannot join it, with the reason
    /// (without the words of `RuntimeError::Online`, which a runtime may wrap it in).
    Failed(String),
}

/// One peer's room, then its match, carried over one link for a [`Runner`].
pub struct OnlineMatch {
    peer: OnlinePeer<Box<dyn Link>>,
    code: RoomCode,
    /// The claims the runner joins with, once ready.
    claims: Option<Vec<Claim>>,
    /// `Assigned` was reported.
    assigned: bool,
    joined: bool,
    /// Why the runner could not join, said on every call after.
    failure: Option<String>,
    /// When the end was first seen.
    ended_at: Option<u64>,
    warnings: PeerWarnings,
}

impl OnlineMatch {
    /// The match of `plan` over `link`.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Online` when the room refuses the plan's role (a room size outside 2 to
    /// 4) or its conditions are out of range.
    pub fn new(link: Box<dyn Link>, plan: &OnlinePlan) -> Result<OnlineMatch, RuntimeError> {
        let peer = OnlinePeer::new(link, plan.role, plan.conditions)
            .map_err(|error| RuntimeError::Online(error.to_string()))?;
        Ok(OnlineMatch {
            peer,
            code: plan.code.clone(),
            claims: None,
            assigned: false,
            joined: false,
            failure: None,
            ended_at: None,
            warnings: PeerWarnings::default(),
        })
    }

    /// The room's code.
    pub fn code(&self) -> &RoomCode {
        &self.code
    }

    /// The match the room agreed on, once it did.
    pub fn layout(&self) -> Option<&MatchLayout> {
        self.peer.layout()
    }

    /// This peer is ready with one local player per claim; the runner joins the match with
    /// `claims`. The lobby is frozen from then on.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Online` for 0 or more than 4 claims, or a second call.
    pub fn ready(&mut self, claims: Vec<Claim>) -> Result<(), RuntimeError> {
        let players = u8::try_from(claims.len()).unwrap_or(u8::MAX);
        self.peer
            .set_ready(players)
            .map_err(|error| RuntimeError::Online(error.to_string()))?;
        self.claims = Some(claims);
        Ok(())
    }

    /// Polls the room at `now_micros`; once it agreed, joins `runner` to the match and gives
    /// it the packets received.
    pub fn before_frame(&mut self, runner: &mut Runner, now_micros: u64) -> OnlinePhase {
        let status = self.peer.poll(now_micros);
        self.warnings.take_room_drops(&mut self.peer);
        if let Some(failure) = &self.failure {
            return OnlinePhase::Failed(failure.clone());
        }
        if !self.assigned && self.peer.id().is_some() {
            self.assigned = true;
            if !matches!(status, OnlineStatus::Failed(_)) {
                return OnlinePhase::Assigned;
            }
        }
        let layout = match status {
            OnlineStatus::Room(view) => return OnlinePhase::Room(view),
            OnlineStatus::Failed(reason) => return OnlinePhase::Failed(reason),
            OnlineStatus::Ready(layout) => layout,
        };
        if !self.joined {
            self.joined = true;
            let joined = match self.claims.clone() {
                Some(claims) => runner.join_match(settings(&layout), claims),
                None => Err(RuntimeError::Online(
                    "the room agreed on a match before this peer was ready".to_string(),
                )),
            };
            if let Err(error) = joined {
                // The reason alone, as the room's own failures give it: each runtime words it.
                let failure = match error {
                    RuntimeError::Online(reason) => reason,
                    other => other.to_string(),
                };
                self.failure = Some(failure.clone());
                return OnlinePhase::Failed(failure);
            }
        }
        for (from, packet) in self.peer.receive(now_micros) {
            runner.net_receive(from.0, packet);
        }
        match runner.online() {
            Some(OnlineState::Ended { end }) => {
                self.ended_at.get_or_insert(now_micros);
                OnlinePhase::Ended(self.with_closed(end.clone()))
            }
            _ => OnlinePhase::Playing,
        }
    }

    /// Sends the packets `runner` made, also after the end, while the match lingers.
    pub fn after_frame(&mut self, runner: &mut Runner, now_micros: u64) {
        for (to, packet) in runner.take_net_packets() {
            self.peer.send(PeerId(to), packet, now_micros);
        }
    }

    /// Leaves the match `runner` plays: every peer ends at the last tick this one has inputs
    /// for. Nothing happens before the runner joined or once it is leaving or the match
    /// ended; a refusal goes to the messages.
    pub fn leave(&mut self, runner: &mut Runner) {
        if !matches!(
            runner.online(),
            Some(OnlineState::Connecting | OnlineState::Running)
        ) {
            return;
        }
        if let Err(error) = runner.leave_match() {
            self.warnings.messages.push(error.to_string());
        }
    }

    /// Whether the match ended less than `END_LINGER_MICROS` before `now_micros`, as
    /// `before_frame` first saw it: the others are still being told why. False before the end.
    pub fn lingering(&self, now_micros: u64) -> bool {
        self.ended_at
            .is_some_and(|ended| now_micros.saturating_sub(ended) < END_LINGER_MICROS)
    }

    /// The messages since the last call: a warning for the room's dropped messages, once per
    /// link and reason for the first 64 of them, and the reason the link closed.
    pub fn messages(&mut self) -> Vec<String> {
        self.warnings.take(&self.peer)
    }

    /// The sends the link lost: match packets refused at once or when due, and room messages
    /// it refused.
    pub fn failed_sends(&self) -> u64 {
        self.peer.failed_sends()
    }

    /// The match packets dropped for coming from outside the match.
    pub fn strays(&self) -> u64 {
        self.peer.strays()
    }

    /// `end`, with the reason the link closed when the match was lost.
    fn with_closed(&self, mut end: MatchEnd) -> MatchEnd {
        if end.kind == MatchEndKind::Lost
            && let Some(reason) = self.peer.closed()
        {
            end.message = format!("{}; {reason}", end.message);
        }
        end
    }
}

/// The warnings of one peer's room and link, as every runtime gives them: one per link and
/// reason of the room's dropped messages, for the first 64 of them, and the reason the link
/// closed, once.
#[derive(Default)]
pub(crate) struct PeerWarnings {
    /// The links and reasons of the room's drops already warned about.
    warned: BTreeSet<(LinkId, String)>,
    /// The messages not taken yet.
    messages: Vec<String>,
    /// The link's `Closed` reason was given to the messages.
    closed_said: bool,
}

impl PeerWarnings {
    /// Turns the room's drops into warnings, once per link and reason. Taken on every poll:
    /// the peer keeps them until taken.
    pub(crate) fn take_room_drops(&mut self, peer: &mut OnlinePeer<Box<dyn Link>>) {
        for RoomEvent::Dropped { link, reason } in peer.room_drops() {
            let words = drop_words(reason);
            let key = (link, words);
            if self.warned.len() >= MAX_WARNED || self.warned.contains(&key) {
                continue;
            }
            self.messages
                .push(format!("dropped a room message from {}: {}", key.0, key.1));
            self.warned.insert(key);
            if self.warned.len() == MAX_WARNED {
                self.messages.push(format!(
                    "{MAX_WARNED} links and reasons of dropped room messages were warned about: \
                     further drops are not"
                ));
            }
        }
    }

    /// The messages since the last call, the reason `peer`'s link closed included once.
    pub(crate) fn take(&mut self, peer: &OnlinePeer<Box<dyn Link>>) -> Vec<String> {
        if !self.closed_said
            && let Some(reason) = peer.closed()
        {
            self.closed_said = true;
            self.messages.push(format!("the link closed: {reason}"));
        }
        std::mem::take(&mut self.messages)
    }
}

fn drop_words(reason: RoomDrop) -> String {
    match reason {
        RoomDrop::Wire(variant) => format!("it does not decode ({variant})"),
        RoomDrop::Unexpected(kind) => format!("a {kind} message is not expected"),
        RoomDrop::Invalid(field) => format!("its {field} is out of range"),
    }
}

/// Writes the snapshot of the divergent tick of a `desync` ending, `end`, of the match
/// `runner` played as peer `local`, into `dump_dir` as `desync-tick-T-peer-P.snap`, and
/// returns its path. The snapshot comes from the session the match recorded, once it
/// resimulated. Compared with the other peer's (`<game> diff`), it names the entities,
/// components and fields that differ.
///
/// # Errors
///
/// `RuntimeError::Record` when the ending names no tick, the session cannot give that tick's
/// snapshot, or the file cannot be written.
pub fn write_desync_snapshot(
    runner: &mut Runner,
    end: &MatchEnd,
    local: u8,
    dump_dir: &Path,
) -> Result<PathBuf, RuntimeError> {
    let tick = end.tick.ok_or_else(|| {
        RuntimeError::Record(format!(
            "the ending names no tick to snapshot: {}",
            end.message
        ))
    })?;
    let snapshot = runner.snapshot_at(tick)?;
    let path = dump_dir.join(format!("desync-tick-{tick}-peer-{local}.snap"));
    std::fs::create_dir_all(dump_dir)
        .and_then(|()| std::fs::write(&path, snapshot))
        .map_err(|error| {
            RuntimeError::Record(format!("cannot write {}: {error}", path.display()))
        })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use ludenea_app::players::automatic_claims;
    use ludenea_net::{Channel, LinkEvent, MemoryMesh};

    use super::*;
    use crate::GameFiles;

    fn runner() -> Runner {
        let files = GameFiles {
            scene: "Scene(version: 2, next_id: 0, resources: {}, entities: [])".to_string(),
            assets: Vec::new(),
            edits: None,
        };
        Runner::load(swarm::app, &files, None, Vec::new()).unwrap()
    }

    fn host_plan() -> OnlinePlan {
        OnlineOptions {
            role: OnlineRole::Host {
                peers: 2,
                code: None,
            },
            signaling: DEFAULT_SIGNALING.to_string(),
            stun: Vec::new(),
            conditions: LinkConditions {
                latency_micros: 0,
                jitter_micros: 0,
                loss_per_mille: 0,
                duplicate_per_mille: 0,
                reorder_per_mille: 0,
                reorder_extra_micros: 0,
                seed: 0,
            },
        }
        .plan(1)
        .unwrap()
    }

    /// The options JSON of a page that hosts a room of 2 with `stun` and `conditions`.
    fn page_options(role: serde_json::Value, stun: &[&str], loss: u64) -> String {
        serde_json::json!({
            "role": role,
            "signaling": "ws://127.0.0.1:3536/rooms",
            "stun": stun,
            "conditions": { "latency_ms": 50, "jitter_ms": 10, "loss_percent": loss },
        })
        .to_string()
    }

    #[test]
    fn the_page_options_read_every_field() {
        let options =
            OnlineOptions::from_json(&page_options(serde_json::json!({ "host": 3 }), &[], 5))
                .unwrap();
        assert_eq!(
            options,
            OnlineOptions {
                role: OnlineRole::Host {
                    peers: 3,
                    code: None
                },
                signaling: "ws://127.0.0.1:3536/rooms".to_string(),
                stun: Vec::new(),
                conditions: LinkConditions {
                    latency_micros: 50_000,
                    jitter_micros: 10_000,
                    loss_per_mille: 50,
                    duplicate_per_mille: 0,
                    reorder_per_mille: 0,
                    reorder_extra_micros: 0,
                    seed: 0,
                },
            }
        );
        let joined = OnlineOptions::from_json(&page_options(
            serde_json::json!({ "join": " abc234 " }),
            &["stun:stun.l.google.com:19302"],
            0,
        ))
        .unwrap();
        assert_eq!(
            joined.role,
            OnlineRole::Join {
                code: RoomCode::parse("ABC234").unwrap()
            }
        );
        assert_eq!(joined.stun, ["stun:stun.l.google.com:19302"]);
    }

    #[test]
    fn bad_page_options_are_refused_naming_the_field() {
        let host = || serde_json::json!({ "host": 2 });
        let mut unknown: serde_json::Value =
            serde_json::from_str(&page_options(host(), &[], 0)).unwrap();
        unknown["speed"] = serde_json::json!(2);
        let mut missing: serde_json::Value =
            serde_json::from_str(&page_options(host(), &[], 0)).unwrap();
        missing["conditions"]
            .as_object_mut()
            .unwrap()
            .remove("jitter_ms");
        let mut signaling: serde_json::Value =
            serde_json::from_str(&page_options(host(), &[], 0)).unwrap();
        signaling["signaling"] = serde_json::json!("http://127.0.0.1/rooms");
        let mut latency: serde_json::Value =
            serde_json::from_str(&page_options(host(), &[], 0)).unwrap();
        latency["conditions"]["latency_ms"] = serde_json::json!(70_000);
        let mut fraction: serde_json::Value =
            serde_json::from_str(&page_options(host(), &[], 0)).unwrap();
        fraction["conditions"]["jitter_ms"] = serde_json::json!(2.5);
        for (text, refusal) in [
            ("[]".to_string(), "options: must be a JSON object"),
            ("{".to_string(), "options: not JSON: "),
            (unknown.to_string(), "speed: is not an option"),
            (missing.to_string(), "conditions.jitter_ms: is missing"),
            (
                page_options(serde_json::json!({ "host": 9 }), &[], 0),
                "role.host: a room holds 2 to 4 peers, not 9",
            ),
            (
                page_options(serde_json::json!({ "host": 2, "join": "ABC234" }), &[], 0),
                "role: must be { \"host\": peers } or { \"join\": code }",
            ),
            (
                page_options(serde_json::json!({ "join": "0O0O0O" }), &[], 0),
                "role.join: '0' (character 1) is not in a room code",
            ),
            (
                page_options(serde_json::json!({ "join": 7 }), &[], 0),
                "role.join: must be a string",
            ),
            (
                signaling.to_string(),
                "signaling: the signaling URL 'http://127.0.0.1/rooms' must start with ws:// or \
                 wss://",
            ),
            (
                page_options(host(), &["http://x"], 0),
                "stun: the STUN URL 'http://x' must start with stun: or stuns:",
            ),
            (
                page_options(host(), &[], 150),
                "conditions.loss_percent: a peer loses 0 to 100 percent of its packets, not 150",
            ),
            (
                latency.to_string(),
                "conditions.latency_ms: a peer delays its packets by 0 to 65535 ms, not 70000",
            ),
            (
                fraction.to_string(),
                "conditions.jitter_ms: must be a whole number",
            ),
        ] {
            let refused = OnlineOptions::from_json(&text).unwrap_err();
            assert!(refused.starts_with(refusal), "{text}: {refused}");
        }
    }

    #[test]
    fn the_page_status_says_where_the_room_stands() {
        let view = RoomView {
            host: true,
            peers: Some(2),
            connected: 1,
            ready: 1,
            unmeshed: 0,
            unfit: 0,
            local_ready: true,
        };
        let room: serde_json::Value =
            serde_json::from_str(&status_json(&OnlineStatus::Room(view), true)).unwrap();
        assert_eq!(
            room,
            serde_json::json!({
                "state": "room",
                "assigned": true,
                "describe": "1 of 2 peers here",
                "view": {
                    "host": true,
                    "peers": 2,
                    "connected": 1,
                    "ready": 1,
                    "unmeshed": 0,
                    "unfit": 0,
                    "local_ready": true,
                },
            })
        );
        let layout = MatchLayout {
            local: PeerId(1),
            match_id: 0x5eed,
            players: vec![PeerId(0), PeerId(1)],
            links: Vec::new(),
        };
        let ready: serde_json::Value =
            serde_json::from_str(&status_json(&OnlineStatus::Ready(layout.clone()), true)).unwrap();
        assert_eq!(ready["state"], "ready");
        assert_eq!(ready["local"], 1);
        let settings: OnlineSettings = serde_json::from_value(ready["settings"].clone()).unwrap();
        assert_eq!(settings, super::settings(&layout));
        let failed: serde_json::Value = serde_json::from_str(&status_json(
            &OnlineStatus::Failed("the host left the room".to_string()),
            false,
        ))
        .unwrap();
        assert_eq!(
            failed,
            serde_json::json!({ "state": "failed", "message": "the host left the room" })
        );
    }

    #[test]
    fn a_room_flood_warns_once_per_link_and_reason_and_never_piles_up() {
        let mesh = MemoryMesh::new();
        let mut online = OnlineMatch::new(Box::new(mesh.join()), &host_plan()).unwrap();
        online.ready(automatic_claims(1)).unwrap();
        let mut runner = runner();
        online.before_frame(&mut runner, 0);
        let mut hostile = mesh.join();
        let host = hostile
            .poll()
            .into_iter()
            .find_map(|event| match event {
                LinkEvent::Connected(host) => Some(host),
                _ => None,
            })
            .expect("a mesh connects its members at once");
        for frame in 1..=3 {
            for _ in 0..2_000 {
                hostile.send(Channel::Room, host, vec![0xff; 8]).unwrap();
            }
            online.before_frame(&mut runner, frame * 1_000);
            assert!(online.peer.room_drops().is_empty(), "taken on every poll");
            let messages = online.messages();
            if frame == 1 {
                assert_eq!(messages.len(), 1, "{messages:?}");
                assert!(messages[0].starts_with("dropped a room message from "));
            } else {
                assert!(messages.is_empty(), "{messages:?}");
            }
        }
        assert_eq!(online.warnings.warned.len(), 1);
    }

    #[test]
    fn warnings_stop_at_a_bound_however_many_links_flood() {
        let mesh = MemoryMesh::new();
        let mut online = OnlineMatch::new(Box::new(mesh.join()), &host_plan()).unwrap();
        online.ready(automatic_claims(1)).unwrap();
        let mut runner = runner();
        online.before_frame(&mut runner, 0);
        let mut messages = Vec::new();
        for frame in 1..=(MAX_WARNED as u64 + 10) {
            // A new link each frame, as a hostile signaling server can make up, then gone.
            let mut hostile = mesh.join();
            let host = hostile
                .poll()
                .into_iter()
                .find_map(|event| match event {
                    LinkEvent::Connected(host) => Some(host),
                    _ => None,
                })
                .expect("a mesh connects its members at once");
            hostile.send(Channel::Room, host, vec![0xff; 8]).unwrap();
            online.before_frame(&mut runner, frame * 1_000);
            messages.extend(online.messages());
        }
        assert_eq!(online.warnings.warned.len(), MAX_WARNED);
        assert_eq!(messages.len(), MAX_WARNED + 1, "{messages:?}");
        assert_eq!(
            messages[MAX_WARNED],
            "64 links and reasons of dropped room messages were warned about: further drops \
             are not"
        );
    }
}
