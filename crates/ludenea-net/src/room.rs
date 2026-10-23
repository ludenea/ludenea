//! The room: where a host and its joiners agree on the peers of a match, its players and its
//! identifier, before any of them starts a session.
//!
//! A [`Room`] is sans-IO. Its owner feeds it the events of its [`Link`](crate::Link)
//! ([`Room::assigned`], [`Room::connected`], [`Room::disconnected`]) and the messages of the
//! room channel ([`Room::receive`]), sends what [`Room::outgoing`] gives on
//! [`Channel::Room`](crate::Channel::Room), and reads [`Room::state`] until it is
//! [`RoomState::Ready`] or [`RoomState::Failed`].
//!
//! Every other member of a room is untrusted: a message that breaks a rule is dropped and
//! reported as a [`RoomEvent::Dropped`], never a panic, and nothing a member sends makes the
//! room hold more than one record per connected link.
//!
//! # The host's announcement
//!
//! - The host sends `Host { peers }` to each link that connects, in every state, before
//!   anything else it sends that link.
//! - After the layout, or while `peers - 1` joiners are already in the layout candidates (the
//!   joiners the layout rule below picks, counting the host as one player until it is ready),
//!   a newcomer gets `Refuse { Full }` right after `Host`.
//!
//! # Joiners and their host
//!
//! - A joiner takes the first connected link that sends `Host` as its host. A repeat from the
//!   same link is ignored.
//! - A `Host` from another link fails the joiner with [`RoomError::TwoHosts`]. A host drops a
//!   `Host` it receives as `Unexpected`, so one joiner cannot fail it.
//! - A joiner accepts `Layout` and `Refuse` only from its host; from anyone else they are
//!   `Unexpected`.
//!
//! # Readiness
//!
//! - Once ready and its host known, a joiner sends `Ready { players, connected }` to the
//!   host, `connected` being its other connected links, sorted. It sends it again whenever
//!   that set changes before the layout.
//! - The host drops a `Ready` whose `players` is outside `1..=MAX_LOCAL_PLAYERS`, or whose
//!   `connected` lists more than `MAX_PEERS - 1` links, as `Invalid`.
//! - A `Ready` whose players can never fit (its players plus the host's plus one per other
//!   missing peer above [`MAX_PLAYERS`]) gets `Refuse { TooManyPlayers }` at once. The host
//!   counts itself as one player until it is ready, and checks every ready joiner again when
//!   it becomes ready.
//!
//! # The layout
//!
//! - Once the host is ready, it picks joiners in connection order. A joiner qualifies when it
//!   is ready, is connected to every joiner already picked, those report being connected to
//!   it, and its players still fit in [`MAX_PLAYERS`].
//! - When it holds `peers - 1` of them, it builds the layout: the host, then the picked
//!   joiners in that order.
//! - It checks the layout with [`SessionSettings::validate`], on settings built from it with
//!   the room's rules, the default timeouts and placeholder identity values. With the default
//!   rules every layout of at most [`MAX_PLAYERS`] players passes; the check guards other
//!   rules. A refusal sends `Refuse { Unplayable }` to the picked joiners, `Refuse { Full }`
//!   to every other connected joiner, and fails the host with [`RoomError::Unplayable`],
//!   naming the settings error.
//! - Otherwise it sends `Layout` to the picked joiners, `Refuse { Full }` to every other
//!   connected joiner, and is [`RoomState::Ready`] as `PeerId(0)`.
//!
//! # The joiner's checks
//!
//! A joiner accepts a `Layout` only when:
//!
//! - both lists have the size its host announced;
//! - the links are distinct and `links[0]` is the host;
//! - its own link (from [`Room::assigned`]) appears once, with the players it declared;
//! - every other link of the layout is connected to it;
//! - each count is 1 to `MAX_LOCAL_PLAYERS`, and the total at most [`MAX_PLAYERS`].
//!
//! Otherwise it fails with [`RoomError::InvalidLayout`], naming the broken rule.
//!
//! # Departures, bad messages and the end
//!
//! - Before the layout, the host forgets a joiner that disconnects, and a joiner whose host
//!   disconnects fails with [`RoomError::HostLeft`].
//! - Bytes that do not decode are dropped as [`RoomDrop::Wire`].
//! - After [`RoomState::Ready`] or [`RoomState::Failed`], the room still answers newcomers as
//!   above and drops everything else, as [`RoomDrop::Unexpected`].
//! - [`Room::set_ready`] takes 1 to `MAX_LOCAL_PLAYERS`, once.

use ludenea_app::players::MAX_LOCAL_PLAYERS;
use thiserror::Error;

use crate::link::LinkId;
use crate::settings::{
    MAX_PEERS, MAX_PLAYERS, MatchIdentity, MatchRules, PeerId, SessionSettings, Timeouts,
};
use crate::wire::{RoomMessage, RoomRefusal};

/// What this peer is in its room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomRole {
    /// It opens a room of `peers` peers, itself included, for the match `match_id`.
    Host { peers: u8, match_id: u64 },
    /// It joins the room of whichever peer announces itself as host.
    Join,
}

/// Where a room stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomState {
    /// The room has not agreed yet.
    Waiting,
    /// Every peer of the match agreed on this layout.
    Ready(MatchLayout),
    /// The room cannot give this peer a match.
    Failed(RoomError),
}

/// The match a room agreed on, as one peer sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchLayout {
    /// This peer.
    pub local: PeerId,
    /// Tells this match's packets from any other's.
    pub match_id: u64,
    /// The owner of each global player, in order.
    pub players: Vec<PeerId>,
    /// The link of each peer, in peer order: the host first.
    pub links: Vec<LinkId>,
}

impl MatchLayout {
    /// The global indices of the players this peer owns, in order.
    pub fn local_players(&self) -> Vec<u8> {
        (0u8..)
            .zip(&self.players)
            .filter(|(_, owner)| **owner == self.local)
            .map(|(index, _)| index)
            .collect()
    }
}

/// Why a room refused a call or failed. Each message names its values.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RoomError {
    /// A host announced a room size outside `2..=MAX_PEERS`.
    #[error("a room holds 2 to {MAX_PEERS} peers, not {peers}")]
    Peers { peers: u8 },
    /// [`Room::set_ready`] got a player count outside `1..=MAX_LOCAL_PLAYERS`.
    #[error("a peer brings 1 to {MAX_LOCAL_PLAYERS} local players, not {players}")]
    LocalPlayers { players: u8 },
    /// [`Room::set_ready`] was called twice.
    #[error("this peer is already ready")]
    AlreadyReady,
    /// The host has all its peers.
    #[error("the room is full")]
    Full,
    /// The host found the players this peer brings can never fit.
    #[error("the peers bring at least {total} players, more than the {MAX_PLAYERS} of a match")]
    TooManyPlayers { total: u8 },
    /// The game cannot be played with the layout the host built.
    #[error("the game cannot be played with the players that came: {reason}")]
    Unplayable { reason: String },
    /// Two links announced themselves as the host.
    #[error("two links claim to host the room: {first} and {second}")]
    TwoHosts { first: LinkId, second: LinkId },
    /// The host disconnected before the layout.
    #[error("the host left the room")]
    HostLeft,
    /// The host sent a layout that breaks the named rule.
    #[error("the host sent an invalid layout: {0}")]
    InvalidLayout(String),
}

/// Something a room did that its owner may want to log or count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomEvent {
    /// A message from `link` was dropped.
    Dropped { link: LinkId, reason: RoomDrop },
}

/// Why a room message was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomDrop {
    /// The bytes do not decode: the [`WireError`](crate::WireError) variant.
    Wire(&'static str),
    /// A message kind this link may not send, now or ever.
    Unexpected(&'static str),
    /// A field out of range: its name.
    Invalid(&'static str),
}

/// What a room waits for, as every runtime shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoomView {
    /// This peer hosts the room.
    pub host: bool,
    /// The room's size, once known.
    pub peers: Option<u8>,
    /// The peers in the room, this one included.
    pub connected: u8,
    /// The peers ready, this one included: the host counts itself and its ready joiners.
    pub ready: u8,
    /// Host only: pairs of joiners whose `Ready` reports they are not connected to each other.
    pub unmeshed: u8,
    /// Host only: ready joiners whose players do not fit.
    pub unfit: u8,
    /// This peer is ready.
    pub local_ready: bool,
}

impl RoomView {
    /// What the room waits for, in one line.
    pub fn describe(&self) -> String {
        let Some(peers) = self.peers else {
            return "waiting for the host".to_owned();
        };
        if !self.host {
            let waiting = if self.local_ready {
                "waiting for the host to start"
            } else {
                "not ready"
            };
            return format!(
                "connected to {} of {peers} peers, {waiting}",
                self.connected
            );
        }
        let here = if self.connected > peers {
            format!("{} peers here for {peers} places", self.connected)
        } else {
            format!("{} of {peers} peers here", self.connected)
        };
        let mut parts = vec![here];
        let not_ready = self.connected.saturating_sub(self.ready);
        if not_ready > 0 {
            parts.push(format!("{not_ready} not ready"));
        }
        if self.unmeshed > 0 {
            let pairs = plural(self.unmeshed, "pair", "pairs");
            parts.push(format!(
                "{} {pairs} of peers not connected to each other",
                self.unmeshed
            ));
        }
        if self.unfit > 0 {
            let peers = plural(self.unfit, "peer", "peers");
            parts.push(format!("{} {peers} with too many players", self.unfit));
        }
        parts.join(", ")
    }
}

fn plural(count: u8, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

/// The room of one peer. See the [module documentation](self) for its rules.
pub struct Room {
    rules: MatchRules,
    me: Option<LinkId>,
    local_players: Option<u8>,
    state: RoomState,
    outgoing: Vec<(LinkId, Vec<u8>)>,
    events: Vec<RoomEvent>,
    side: Side,
}

enum Side {
    Host(HostSide),
    Join(JoinSide),
}

struct HostSide {
    peers: u8,
    match_id: u64,
    /// Every connected joiner, in connection order.
    joiners: Vec<Joiner>,
}

struct Joiner {
    link: LinkId,
    /// Its latest valid `Ready`.
    ready: Option<Declared>,
    /// It got a `Refuse` and is no longer in the room.
    refused: bool,
}

struct Declared {
    players: u8,
    connected: Vec<LinkId>,
}

/// The joiners the layout rule picks, as indices into the joiners.
struct Pick {
    picked: Vec<usize>,
    unfit: u8,
}

#[derive(Default)]
struct JoinSide {
    /// Every connected link, in connection order.
    connected: Vec<LinkId>,
    /// The host's link and the room size it announced.
    host: Option<(LinkId, u8)>,
    /// The `connected` of the latest `Ready` sent.
    reported: Option<Vec<LinkId>>,
}

impl Room {
    /// A room for `role`; `rules` serve only to check the layout.
    ///
    /// # Errors
    ///
    /// [`RoomError::Peers`] when a host's `peers` is outside `2..=MAX_PEERS`.
    pub fn new(role: RoomRole, rules: MatchRules) -> Result<Room, RoomError> {
        let side = match role {
            RoomRole::Host { peers, match_id } => {
                if !(2..=MAX_PEERS).contains(&usize::from(peers)) {
                    return Err(RoomError::Peers { peers });
                }
                Side::Host(HostSide {
                    peers,
                    match_id,
                    joiners: Vec::new(),
                })
            }
            RoomRole::Join => Side::Join(JoinSide::default()),
        };
        Ok(Room {
            rules,
            me: None,
            local_players: None,
            state: RoomState::Waiting,
            outgoing: Vec::new(),
            events: Vec::new(),
            side,
        })
    }

    /// This peer's own link; the first assignment holds.
    pub fn assigned(&mut self, me: LinkId) {
        if self.me.is_none() {
            self.me = Some(me);
            self.try_layout();
        }
    }

    /// A link whose channels opened.
    pub fn connected(&mut self, link: LinkId) {
        if self.me == Some(link) {
            return;
        }
        match &mut self.side {
            Side::Host(host) => {
                if host.joiners.iter().any(|joiner| joiner.link == link) {
                    return;
                }
                let peers = host.peers;
                send(&mut self.outgoing, link, &RoomMessage::Host { peers });
                let full = self.state != RoomState::Waiting
                    || self.pick(self.local_players.unwrap_or(1)).picked.len()
                        >= usize::from(peers - 1);
                if full {
                    send(&mut self.outgoing, link, &refuse(RoomRefusal::Full));
                }
                if let Side::Host(host) = &mut self.side {
                    host.joiners.push(Joiner {
                        link,
                        ready: None,
                        refused: full,
                    });
                }
            }
            Side::Join(join) => {
                if !join.connected.contains(&link) {
                    join.connected.push(link);
                    self.report();
                }
            }
        }
    }

    /// A link that is gone.
    pub fn disconnected(&mut self, link: LinkId) {
        match &mut self.side {
            Side::Host(host) => {
                host.joiners.retain(|joiner| joiner.link != link);
                self.try_layout();
            }
            Side::Join(join) => {
                join.connected.retain(|other| *other != link);
                let host_left = join.host.is_some_and(|(host, _)| host == link);
                if host_left && self.state == RoomState::Waiting {
                    self.state = RoomState::Failed(RoomError::HostLeft);
                } else {
                    self.report();
                }
            }
        }
    }

    /// A message of the room channel from `from`.
    pub fn receive(&mut self, from: LinkId, bytes: &[u8]) {
        let message = match RoomMessage::decode(bytes) {
            Ok(message) => message,
            Err(error) => return self.discard(from, RoomDrop::Wire(error.variant())),
        };
        if self.state != RoomState::Waiting {
            return self.discard(from, RoomDrop::Unexpected(kind(&message)));
        }
        match &self.side {
            Side::Host(_) => self.host_receive(from, message),
            Side::Join(_) => self.join_receive(from, message),
        }
    }

    /// This peer is ready with `players` local players.
    ///
    /// # Errors
    ///
    /// [`RoomError::LocalPlayers`] outside `1..=MAX_LOCAL_PLAYERS`, and
    /// [`RoomError::AlreadyReady`] on a second call.
    pub fn set_ready(&mut self, players: u8) -> Result<(), RoomError> {
        if !(1..=MAX_LOCAL_PLAYERS).contains(&usize::from(players)) {
            return Err(RoomError::LocalPlayers { players });
        }
        if self.local_players.is_some() {
            return Err(RoomError::AlreadyReady);
        }
        self.local_players = Some(players);
        match self.side {
            Side::Host(_) => {
                self.refuse_hopeless();
                self.try_layout();
            }
            Side::Join(_) => self.report(),
        }
        Ok(())
    }

    /// The messages to send on the room channel since the last call, in order.
    pub fn outgoing(&mut self) -> Vec<(LinkId, Vec<u8>)> {
        std::mem::take(&mut self.outgoing)
    }

    /// Where the room stands.
    pub fn state(&self) -> &RoomState {
        &self.state
    }

    /// The events since the last call, oldest first.
    pub fn events(&mut self) -> Vec<RoomEvent> {
        std::mem::take(&mut self.events)
    }

    /// What the room waits for.
    pub fn view(&self) -> RoomView {
        let local_ready = self.local_players.is_some();
        match &self.side {
            Side::Host(host) => {
                let present: Vec<&Joiner> = host
                    .joiners
                    .iter()
                    .filter(|joiner| !joiner.refused)
                    .collect();
                let ready: Vec<&Declared> = present
                    .iter()
                    .filter_map(|joiner| joiner.ready.as_ref())
                    .collect();
                let unmeshed = present
                    .iter()
                    .enumerate()
                    .flat_map(|(index, a)| present[index + 1..].iter().map(move |b| (a, b)))
                    .filter(|(a, b)| match (&a.ready, &b.ready) {
                        (Some(ready_a), Some(ready_b)) => !meshed(a.link, ready_a, b.link, ready_b),
                        _ => false,
                    })
                    .count();
                RoomView {
                    host: true,
                    peers: Some(host.peers),
                    connected: count(1 + present.len()),
                    ready: count(usize::from(local_ready) + ready.len()),
                    unmeshed: count(unmeshed),
                    unfit: self.pick(self.local_players.unwrap_or(1)).unfit,
                    local_ready,
                }
            }
            Side::Join(join) => RoomView {
                host: false,
                peers: join.host.map(|(_, peers)| peers),
                connected: count(1 + join.connected.len()),
                ready: u8::from(local_ready),
                unmeshed: 0,
                unfit: 0,
                local_ready,
            },
        }
    }

    fn discard(&mut self, link: LinkId, reason: RoomDrop) {
        self.events.push(RoomEvent::Dropped { link, reason });
    }

    fn fail(&mut self, error: RoomError) {
        self.state = RoomState::Failed(error);
    }

    fn host_receive(&mut self, from: LinkId, message: RoomMessage) {
        let RoomMessage::Ready { players, connected } = message else {
            return self.discard(from, RoomDrop::Unexpected(kind(&message)));
        };
        let Side::Host(host) = &mut self.side else {
            return;
        };
        let Some(joiner) = host
            .joiners
            .iter_mut()
            .find(|joiner| joiner.link == from && !joiner.refused)
        else {
            return self.discard(from, RoomDrop::Unexpected("Ready"));
        };
        if !(1..=MAX_LOCAL_PLAYERS).contains(&usize::from(players)) {
            return self.discard(from, RoomDrop::Invalid("players"));
        }
        if connected.len() > MAX_PEERS - 1 {
            return self.discard(from, RoomDrop::Invalid("connected"));
        }
        joiner.ready = Some(Declared { players, connected });
        self.refuse_hopeless();
        self.try_layout();
    }

    /// Refuses every ready joiner whose players can never fit beside the host's and one per
    /// other missing peer.
    fn refuse_hopeless(&mut self) {
        let host_players = self.local_players.unwrap_or(1);
        let Side::Host(host) = &mut self.side else {
            return;
        };
        let others = host.peers - 2;
        for joiner in host.joiners.iter_mut().filter(|joiner| !joiner.refused) {
            let Some(declared) = &joiner.ready else {
                continue;
            };
            // At most 4 + 4 + 2: no overflow.
            let total = declared.players + host_players + others;
            if usize::from(total) > MAX_PLAYERS {
                let reason = RoomRefusal::TooManyPlayers { total };
                send(&mut self.outgoing, joiner.link, &refuse(reason));
                joiner.refused = true;
                joiner.ready = None;
            }
        }
    }

    /// The joiners the layout rule picks, the host bringing `host_players`.
    fn pick(&self, host_players: u8) -> Pick {
        let mut pick = Pick {
            picked: Vec::new(),
            unfit: 0,
        };
        let Side::Host(host) = &self.side else {
            return pick;
        };
        let wanted = usize::from(host.peers - 1);
        let mut players = usize::from(host_players);
        for (index, joiner) in host.joiners.iter().enumerate() {
            if pick.picked.len() == wanted {
                break;
            }
            let Some(declared) = joiner.ready.as_ref().filter(|_| !joiner.refused) else {
                continue;
            };
            let with_picked = pick.picked.iter().all(|picked| {
                let other = &host.joiners[*picked];
                other
                    .ready
                    .as_ref()
                    .is_some_and(|ready| meshed(joiner.link, declared, other.link, ready))
            });
            if !with_picked {
                continue;
            }
            if players + usize::from(declared.players) > MAX_PLAYERS {
                pick.unfit = pick.unfit.saturating_add(1);
                continue;
            }
            players += usize::from(declared.players);
            pick.picked.push(index);
        }
        pick
    }

    fn try_layout(&mut self) {
        if self.state != RoomState::Waiting {
            return;
        }
        let (Some(me), Some(host_players)) = (self.me, self.local_players) else {
            return;
        };
        let picked = self.pick(host_players).picked;
        let Side::Host(host) = &mut self.side else {
            return;
        };
        if picked.len() < usize::from(host.peers - 1) {
            return;
        }
        let mut links = vec![me];
        let mut counts = vec![host_players];
        let mut players = vec![PeerId(0); usize::from(host_players)];
        for (peer, index) in (1u8..).zip(&picked) {
            let joiner = &host.joiners[*index];
            let joiner_players = joiner.ready.as_ref().map_or(0, |ready| ready.players);
            links.push(joiner.link);
            counts.push(joiner_players);
            players.extend(std::iter::repeat_n(
                PeerId(peer),
                usize::from(joiner_players),
            ));
        }
        let settings = SessionSettings {
            local: PeerId(0),
            match_id: host.match_id,
            identity: MatchIdentity {
                engine: String::new(),
                fingerprint: 0,
                scene_and_assets: 0,
                tick_rate: 1,
                players: players.clone(),
                rules: self.rules,
            },
            timeouts: Timeouts::default(),
        };
        // The picked joiners get the layout or the refusal; every other joiner waiting in the
        // room is refused as full either way.
        let (verdict, state) = match settings.validate() {
            Ok(()) => (
                RoomMessage::Layout {
                    match_id: host.match_id,
                    links: links.clone(),
                    counts,
                },
                RoomState::Ready(MatchLayout {
                    local: PeerId(0),
                    match_id: host.match_id,
                    players,
                    links,
                }),
            ),
            Err(error) => (
                refuse(RoomRefusal::Unplayable),
                RoomState::Failed(RoomError::Unplayable {
                    reason: error.to_string(),
                }),
            ),
        };
        for (index, joiner) in host.joiners.iter_mut().enumerate() {
            if picked.contains(&index) {
                send(&mut self.outgoing, joiner.link, &verdict);
            } else if !joiner.refused {
                send(&mut self.outgoing, joiner.link, &refuse(RoomRefusal::Full));
                joiner.refused = true;
            }
        }
        self.state = state;
    }

    fn join_receive(&mut self, from: LinkId, message: RoomMessage) {
        let Side::Join(join) = &mut self.side else {
            return;
        };
        match message {
            RoomMessage::Host { peers } => match join.host {
                Some((host, _)) if host == from => {}
                Some((first, _)) => self.fail(RoomError::TwoHosts {
                    first,
                    second: from,
                }),
                None if !join.connected.contains(&from) => {
                    self.discard(from, RoomDrop::Unexpected("Host"));
                }
                None if !(2..=MAX_PEERS).contains(&usize::from(peers)) => {
                    self.discard(from, RoomDrop::Invalid("peers"));
                }
                None => {
                    join.host = Some((from, peers));
                    self.report();
                }
            },
            RoomMessage::Ready { .. } => self.discard(from, RoomDrop::Unexpected("Ready")),
            RoomMessage::Layout { .. } | RoomMessage::Refuse { .. } => {
                let Some((host, peers)) = join.host.filter(|(host, _)| *host == from) else {
                    return self.discard(from, RoomDrop::Unexpected(kind(&message)));
                };
                let result = match message {
                    RoomMessage::Layout {
                        match_id,
                        links,
                        counts,
                    } => self
                        .check_layout(host, peers, links, &counts)
                        .map(|(local, players, links)| {
                            RoomState::Ready(MatchLayout {
                                local,
                                match_id,
                                players,
                                links,
                            })
                        })
                        .unwrap_or_else(|rule| RoomState::Failed(RoomError::InvalidLayout(rule))),
                    RoomMessage::Refuse { reason } => RoomState::Failed(match reason {
                        RoomRefusal::Full => RoomError::Full,
                        RoomRefusal::TooManyPlayers { total } => {
                            RoomError::TooManyPlayers { total }
                        }
                        RoomRefusal::Unplayable => RoomError::Unplayable {
                            reason: "the host refused the layout".to_owned(),
                        },
                    }),
                    RoomMessage::Host { .. } | RoomMessage::Ready { .. } => return,
                };
                self.state = result;
            }
        }
    }

    /// The joiner's checks of a layout: this peer, the owner of each player, and the links.
    fn check_layout(
        &self,
        host: LinkId,
        peers: u8,
        links: Vec<LinkId>,
        counts: &[u8],
    ) -> Result<(PeerId, Vec<PeerId>, Vec<LinkId>), String> {
        let Side::Join(join) = &self.side else {
            return Err("this peer hosts the room".to_owned());
        };
        if links.len() != usize::from(peers) || counts.len() != usize::from(peers) {
            return Err(format!(
                "the layout lists {} links and {} counts, the host announced {peers} peers",
                links.len(),
                counts.len()
            ));
        }
        let distinct = links
            .iter()
            .enumerate()
            .all(|(index, link)| !links[..index].contains(link));
        if !distinct {
            return Err("the layout lists a link twice".to_owned());
        }
        if links.first() != Some(&host) {
            return Err("the layout does not start with the host".to_owned());
        }
        let Some(me) = self.me else {
            return Err("own link unknown".to_owned());
        };
        let Some((local, _)) = (0u8..).zip(&links).find(|(_, link)| **link == me) else {
            return Err("the layout leaves this peer out".to_owned());
        };
        let given = counts[usize::from(local)];
        match self.local_players {
            Some(declared) if declared == given => {}
            Some(declared) => {
                return Err(format!(
                    "the layout gives this peer {given} players, it declared {declared}"
                ));
            }
            None => return Err("the layout came before this peer declared its players".to_owned()),
        }
        let strangers = links
            .iter()
            .any(|link| *link != me && !join.connected.contains(link));
        if strangers {
            return Err("the layout names a link this peer is not connected to".to_owned());
        }
        if let Some(count) = counts
            .iter()
            .find(|count| !(1..=MAX_LOCAL_PLAYERS).contains(&usize::from(**count)))
        {
            return Err(format!(
                "the layout gives a peer {count} players, expected 1 to {MAX_LOCAL_PLAYERS}"
            ));
        }
        let total: usize = counts.iter().map(|count| usize::from(*count)).sum();
        if total > MAX_PLAYERS {
            return Err(format!(
                "the layout holds {total} players, at most {MAX_PLAYERS} allowed"
            ));
        }
        let players = (0u8..)
            .zip(counts)
            .flat_map(|(peer, count)| std::iter::repeat_n(PeerId(peer), usize::from(*count)))
            .collect();
        Ok((PeerId(local), players, links))
    }

    /// A joiner sends `Ready` to its host when it is ready and its other connected links
    /// changed since the last one it sent.
    fn report(&mut self) {
        let Side::Join(join) = &mut self.side else {
            return;
        };
        let (Some(players), Some((host, _))) = (self.local_players, join.host) else {
            return;
        };
        if self.state != RoomState::Waiting {
            return;
        }
        let mut connected: Vec<LinkId> = join
            .connected
            .iter()
            .copied()
            .filter(|link| *link != host)
            .collect();
        connected.sort_unstable();
        if join.reported.as_ref() == Some(&connected) {
            return;
        }
        join.reported = Some(connected.clone());
        send(
            &mut self.outgoing,
            host,
            &RoomMessage::Ready { players, connected },
        );
    }
}

/// Two ready joiners each report being connected to the other.
fn meshed(a: LinkId, ready_a: &Declared, b: LinkId, ready_b: &Declared) -> bool {
    ready_a.connected.contains(&b) && ready_b.connected.contains(&a)
}

fn send(outgoing: &mut Vec<(LinkId, Vec<u8>)>, to: LinkId, message: &RoomMessage) {
    outgoing.push((to, message.encode()));
}

fn refuse(reason: RoomRefusal) -> RoomMessage {
    RoomMessage::Refuse { reason }
}

fn kind(message: &RoomMessage) -> &'static str {
    match message {
        RoomMessage::Host { .. } => "Host",
        RoomMessage::Ready { .. } => "Ready",
        RoomMessage::Layout { .. } => "Layout",
        RoomMessage::Refuse { .. } => "Refuse",
    }
}

/// A count of room members, which the link layer bounds far below `u8::MAX`.
fn count(value: usize) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}
