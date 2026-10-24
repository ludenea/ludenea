//! One peer online: [`OnlinePeer`] drives its [`Room`] over one [`Link`], then carries its
//! match's packets over the same link, behind the peer's own [`LinkConditions`].
//!
//! The caller polls it with the current time until [`OnlineStatus::Ready`], builds its
//! session from the [`MatchLayout`], then passes the session's packets through
//! [`OnlinePeer::receive`] and [`OnlinePeer::send`], polling on every frame so the room keeps
//! answering newcomers and its messages never pile up.

use thiserror::Error;

use crate::link::{Channel, Link, LinkEvent, LinkId, LinkTransport};
use crate::room::{MatchLayout, Room, RoomError, RoomEvent, RoomRole, RoomState, RoomView};
use crate::settings::{MatchRules, PeerId};
use crate::transport::{Conditioned, ConditionsError, LinkConditions, Transport};

/// Why an [`OnlinePeer`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OnlineError {
    /// The room refused its role.
    #[error(transparent)]
    Room(RoomError),
    /// The link conditions are out of range.
    #[error(transparent)]
    Conditions(ConditionsError),
}

/// Where an [`OnlinePeer`] stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnlineStatus {
    /// The room has not agreed yet; what it waits for.
    Room(RoomView),
    /// The room agreed on this match. It stays so: a link lost afterwards is the session's
    /// silence to handle, and [`OnlinePeer::closed`] gives its reason.
    Ready(MatchLayout),
    /// The room cannot give this peer a match, with the reason.
    Failed(String),
}

/// One peer's room, then its match, over one link.
///
/// Before the layout, match packets that arrive are dropped and counted by
/// [`OnlinePeer::strays`], and [`OnlinePeer::send`] sends nothing. Room messages are read on
/// every poll, before and after the layout, so a flood never accumulates in the link.
pub struct OnlinePeer<L: Link> {
    room: Room,
    transport: Conditioned<LinkTransport<L>>,
    /// The conditions add nothing: sends skip the conditioner.
    clear: bool,
    layout: Option<MatchLayout>,
    /// Why the link closed before the layout.
    failure: Option<String>,
    /// Why the link closed after the layout.
    closed: Option<String>,
    failed_sends: u64,
    room_drops: Vec<RoomEvent>,
}

impl<L: Link> OnlinePeer<L> {
    /// A peer of `role` over `link`, whose match sends go through `conditions`.
    ///
    /// # Errors
    ///
    /// [`OnlineError::Room`] when the room refuses `role`, and [`OnlineError::Conditions`]
    /// when `conditions` are out of range.
    pub fn new(link: L, role: RoomRole, conditions: LinkConditions) -> Result<Self, OnlineError> {
        let room = Room::new(role, MatchRules::default()).map_err(OnlineError::Room)?;
        let transport = Conditioned::new(LinkTransport::new(link, Vec::new()), conditions)
            .map_err(OnlineError::Conditions)?;
        let clear = LinkConditions {
            seed: conditions.seed,
            ..CLEAR
        } == conditions;
        Ok(Self {
            room,
            transport,
            clear,
            layout: None,
            failure: None,
            closed: None,
            failed_sends: 0,
            room_drops: Vec::new(),
        })
    }

    /// Feeds the link's events and room messages to the room, sends what the room has to
    /// send, and says where the peer stands.
    pub fn poll(&mut self, now_micros: u64) -> OnlineStatus {
        let link = self.transport.inner_mut().link_mut();
        for event in link.poll() {
            match event {
                LinkEvent::Assigned(me) => self.room.assigned(me),
                LinkEvent::Connected(other) => self.room.connected(other),
                LinkEvent::Disconnected(other) => self.room.disconnected(other),
                LinkEvent::Closed(reason) if self.layout.is_some() => self.closed = Some(reason),
                // A room that already failed keeps its own reason.
                LinkEvent::Closed(reason) if *self.room.state() == RoomState::Waiting => {
                    self.failure
                        .get_or_insert(format!("the connection to the room closed: {reason}"));
                }
                LinkEvent::Closed(_) => {}
            }
        }
        while let Some((from, bytes)) = link.receive(Channel::Room) {
            self.room.receive(from, &bytes);
        }
        for (to, bytes) in self.room.outgoing() {
            if link.send(Channel::Room, to, bytes).is_err() {
                self.failed_sends += 1;
            }
        }
        self.room_drops.extend(self.room.events());
        if self.layout.is_none()
            && self.failure.is_none()
            && let RoomState::Ready(layout) = self.room.state()
        {
            // Adopted before the match channel is read: a peer that got the layout first
            // may already have sent its first packet.
            self.transport.inner_mut().set_links(layout.links.clone());
            self.layout = Some(layout.clone());
        }
        if self.layout.is_none() {
            // No link is in the match yet: every match packet counts as a stray.
            while self.transport.inner_mut().receive(now_micros).is_some() {}
        }
        if let Some(layout) = &self.layout {
            return OnlineStatus::Ready(layout.clone());
        }
        if let Some(failure) = &self.failure {
            return OnlineStatus::Failed(failure.clone());
        }
        match self.room.state() {
            RoomState::Failed(error) => OnlineStatus::Failed(error.to_string()),
            RoomState::Waiting | RoomState::Ready(_) => OnlineStatus::Room(self.room.view()),
        }
    }

    /// This peer is ready with `players` local players.
    ///
    /// # Errors
    ///
    /// As [`Room::set_ready`].
    pub fn set_ready(&mut self, players: u8) -> Result<(), RoomError> {
        self.room.set_ready(players)
    }

    /// This peer's own link, once the link reported it: from then on the room exists where
    /// the link meets the others.
    pub fn id(&self) -> Option<LinkId> {
        self.room.me()
    }

    /// The match the room agreed on, once [`OnlineStatus::Ready`].
    pub fn layout(&self) -> Option<&MatchLayout> {
        self.layout.as_ref()
    }

    /// Why the link closed after the layout, for the runtime to add to the end's message.
    pub fn closed(&self) -> Option<&str> {
        self.closed.as_deref()
    }

    /// Every match packet that arrived by `now_micros`, with its sender; nothing before the
    /// layout. It also releases the delayed sends that are due.
    pub fn receive(&mut self, now_micros: u64) -> Vec<(PeerId, Vec<u8>)> {
        if self.layout.is_none() {
            return Vec::new();
        }
        std::iter::from_fn(|| self.transport.receive(now_micros)).collect()
    }

    /// Sends one match packet to `to`. Before the layout, or when the link refuses it, the
    /// packet is lost and counted by [`OnlinePeer::failed_sends`].
    pub fn send(&mut self, to: PeerId, packet: Vec<u8>, now_micros: u64) {
        let sent = match (&self.layout, self.clear) {
            (None, _) => false,
            (Some(_), true) => self
                .transport
                .inner_mut()
                .send(to, packet, now_micros)
                .is_ok(),
            (Some(_), false) => self.transport.send(to, packet, now_micros).is_ok(),
        };
        if !sent {
            self.failed_sends += 1;
        }
    }

    /// How many sends were lost: match packets refused at once or when due, and room
    /// messages the link refused.
    pub fn failed_sends(&self) -> u64 {
        self.failed_sends + self.transport.failed_sends()
    }

    /// How many match packets were dropped for coming from outside the match, every one
    /// before the layout included.
    pub fn strays(&self) -> u64 {
        self.transport.inner().strays()
    }

    /// How many messages the link dropped on arrival for exceeding their channel's limit
    /// ([`Link::oversized`]).
    pub fn oversized(&self) -> u64 {
        self.transport.inner().link().oversized()
    }

    /// The room messages dropped since the last call, oldest first. They pile up until taken:
    /// take them on every poll.
    pub fn room_drops(&mut self) -> Vec<RoomEvent> {
        std::mem::take(&mut self.room_drops)
    }
}

/// Conditions that add nothing, whatever the seed.
const CLEAR: LinkConditions = LinkConditions {
    latency_micros: 0,
    jitter_micros: 0,
    loss_per_mille: 0,
    duplicate_per_mille: 0,
    reorder_per_mille: 0,
    reorder_extra_micros: 0,
    seed: 0,
};
