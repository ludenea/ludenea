//! How peers that know each other by [`LinkId`] talk before and during a match: a reliable room
//! channel and an unreliable match channel, an in-memory mesh of them for tests, and
//! [`LinkTransport`], the match channel as a [`Transport`].

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::settings::{MAX_PACKET_BYTES, PeerId};
use crate::transport::{Transport, TransportError};

/// The longest message of the room channel.
pub const MAX_ROOM_MESSAGE_BYTES: usize = 512;

/// The name of a link: 16 bytes, shown as a UUID because the signaling server names peers so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LinkId(pub [u8; 16]);

impl fmt::Display for LinkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// The two channels of a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Reliable and ordered per peer, messages of at most [`MAX_ROOM_MESSAGE_BYTES`].
    Room,
    /// Promises nothing, packets of at most [`MAX_PACKET_BYTES`].
    Match,
}

impl Channel {
    fn max_bytes(self) -> usize {
        match self {
            Self::Room => MAX_ROOM_MESSAGE_BYTES,
            Self::Match => MAX_PACKET_BYTES,
        }
    }
}

/// What happened to a link or to one of its peers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    /// This peer's own id.
    Assigned(LinkId),
    /// A peer whose channels opened.
    Connected(LinkId),
    /// A peer that is gone.
    Disconnected(LinkId),
    /// The link itself is gone, with the reason.
    Closed(String),
}

/// Why a link refused to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LinkError {
    /// The destination is not a peer currently connected.
    #[error("{link} is not connected")]
    UnknownLink { link: LinkId },
    /// The message is longer than its channel allows.
    #[error("a message of {len} bytes exceeds the {max} byte limit of its channel")]
    TooLarge { len: usize, max: usize },
    /// The link itself is gone.
    #[error("the link is closed")]
    Closed,
}

/// Channels to the peers of a room, whatever carries them.
pub trait Link {
    /// The events since the last call, oldest first.
    fn poll(&mut self) -> Vec<LinkEvent>;

    /// Sends `bytes` to `to` on `channel`.
    ///
    /// # Errors
    ///
    /// [`LinkError::UnknownLink`] when `to` is not currently connected,
    /// [`LinkError::TooLarge`] when `bytes` exceeds the channel's limit and
    /// [`LinkError::Closed`] when the link itself is gone.
    fn send(&mut self, channel: Channel, to: LinkId, bytes: Vec<u8>) -> Result<(), LinkError>;

    /// The next message that arrived on `channel`, with its sender.
    fn receive(&mut self, channel: Channel) -> Option<(LinkId, Vec<u8>)>;
}

impl<L: Link + ?Sized> Link for Box<L> {
    fn poll(&mut self) -> Vec<LinkEvent> {
        (**self).poll()
    }

    fn send(&mut self, channel: Channel, to: LinkId, bytes: Vec<u8>) -> Result<(), LinkError> {
        (**self).send(channel, to, bytes)
    }

    fn receive(&mut self, channel: Channel) -> Option<(LinkId, Vec<u8>)> {
        (**self).receive(channel)
    }
}

type Inbox = VecDeque<(LinkId, Vec<u8>)>;

#[derive(Default)]
struct Member {
    events: VecDeque<LinkEvent>,
    room: Inbox,
    matches: Inbox,
    connected: BTreeSet<LinkId>,
}

#[derive(Default)]
struct Mesh {
    next: u64,
    members: BTreeMap<LinkId, Member>,
}

impl Mesh {
    fn connect(&mut self, a: LinkId, b: LinkId) {
        if a == b || !self.members.contains_key(&a) || !self.members.contains_key(&b) {
            return;
        }
        for (me, other) in [(a, b), (b, a)] {
            if let Some(member) = self.members.get_mut(&me)
                && member.connected.insert(other)
            {
                member.events.push_back(LinkEvent::Connected(other));
            }
        }
    }

    fn disconnect(&mut self, a: LinkId, b: LinkId) {
        for (me, other) in [(a, b), (b, a)] {
            if let Some(member) = self.members.get_mut(&me)
                && member.connected.remove(&other)
            {
                member.events.push_back(LinkEvent::Disconnected(other));
            }
        }
    }
}

/// A shared, single-threaded set of [`MemoryLink`]s that can reach each other, for tests.
#[derive(Clone, Default)]
pub struct MemoryMesh {
    mesh: Rc<RefCell<Mesh>>,
}

impl MemoryMesh {
    /// An empty mesh.
    pub fn new() -> Self {
        Self::default()
    }

    /// A new link connected to every member.
    pub fn join(&self) -> MemoryLink {
        self.join_with(&self.members())
    }

    /// A new link connected only to those of `peers` that are members.
    pub fn join_with(&self, peers: &[LinkId]) -> MemoryLink {
        let mut mesh = self.mesh.borrow_mut();
        mesh.next += 1;
        let mut bytes = [0; 16];
        bytes[8..].copy_from_slice(&mesh.next.to_be_bytes());
        let id = LinkId(bytes);
        let mut member = Member::default();
        member.events.push_back(LinkEvent::Assigned(id));
        mesh.members.insert(id, member);
        for peer in peers {
            mesh.connect(id, *peer);
        }
        MemoryLink {
            id,
            mesh: Rc::clone(&self.mesh),
        }
    }

    /// Connects two members that are not yet; a no-op for a stranger or an existing connection.
    pub fn connect(&self, a: LinkId, b: LinkId) {
        self.mesh.borrow_mut().connect(a, b);
    }

    /// Cuts two members apart; a no-op when they are not connected.
    pub fn disconnect(&self, a: LinkId, b: LinkId) {
        self.mesh.borrow_mut().disconnect(a, b);
    }

    fn members(&self) -> Vec<LinkId> {
        self.mesh.borrow().members.keys().copied().collect()
    }
}

/// One member of a [`MemoryMesh`]. Delivery is immediate and in send order. Dropping it
/// disconnects it from its peers.
///
/// Messages queued before a disconnect stay readable afterwards. Its inboxes are unbounded:
/// it is a test helper, not a network.
pub struct MemoryLink {
    id: LinkId,
    mesh: Rc<RefCell<Mesh>>,
}

impl Link for MemoryLink {
    fn poll(&mut self) -> Vec<LinkEvent> {
        let mut mesh = self.mesh.borrow_mut();
        mesh.members
            .get_mut(&self.id)
            .map(|member| member.events.drain(..).collect())
            .unwrap_or_default()
    }

    fn send(&mut self, channel: Channel, to: LinkId, bytes: Vec<u8>) -> Result<(), LinkError> {
        let mut mesh = self.mesh.borrow_mut();
        let connected = mesh
            .members
            .get(&self.id)
            .is_some_and(|member| member.connected.contains(&to));
        if !connected {
            return Err(LinkError::UnknownLink { link: to });
        }
        let max = channel.max_bytes();
        if bytes.len() > max {
            return Err(LinkError::TooLarge {
                len: bytes.len(),
                max,
            });
        }
        let Some(target) = mesh.members.get_mut(&to) else {
            return Err(LinkError::UnknownLink { link: to });
        };
        match channel {
            Channel::Room => target.room.push_back((self.id, bytes)),
            Channel::Match => target.matches.push_back((self.id, bytes)),
        }
        Ok(())
    }

    fn receive(&mut self, channel: Channel) -> Option<(LinkId, Vec<u8>)> {
        let mut mesh = self.mesh.borrow_mut();
        let member = mesh.members.get_mut(&self.id)?;
        match channel {
            Channel::Room => member.room.pop_front(),
            Channel::Match => member.matches.pop_front(),
        }
    }
}

impl Drop for MemoryLink {
    fn drop(&mut self) {
        let mut mesh = self.mesh.borrow_mut();
        let Some(member) = mesh.members.remove(&self.id) else {
            return;
        };
        for peer in member.connected {
            if let Some(other) = mesh.members.get_mut(&peer) {
                other.connected.remove(&self.id);
                other.events.push_back(LinkEvent::Disconnected(self.id));
            }
        }
    }
}

/// The match channel of a [`Link`] as a [`Transport`]: `PeerId(i)` is `links[i]`.
///
/// A packet to a peer whose link is not connected, or to a closed link, is lost and reported as
/// [`TransportError::Unreachable`]. The owner learns that the link closed through
/// [`LinkEvent::Closed`] from `link_mut().poll()`; the transport never polls.
pub struct LinkTransport<L: Link> {
    link: L,
    links: Vec<LinkId>,
    strays: u64,
}

impl<L: Link> LinkTransport<L> {
    /// Wraps `link`; the peers of the match are `links`, in peer order.
    ///
    /// `links` must be distinct, as a validated layout gives: with a duplicate, packets are
    /// attributed to the lowest peer holding that link.
    pub fn new(link: L, links: Vec<LinkId>) -> Self {
        Self {
            link,
            links,
            strays: 0,
        }
    }

    /// How many packets from links outside the match were dropped.
    pub fn strays(&self) -> u64 {
        self.strays
    }

    /// The underlying link, to poll its events or use its room channel.
    pub fn link_mut(&mut self) -> &mut L {
        &mut self.link
    }
}

impl<L: Link> Transport for LinkTransport<L> {
    fn send(
        &mut self,
        to: PeerId,
        packet: Vec<u8>,
        _now_micros: u64,
    ) -> Result<(), TransportError> {
        let Some(link) = self.links.get(usize::from(to.0)).copied() else {
            return Err(TransportError::UnknownPeer { peer: to });
        };
        self.link
            .send(Channel::Match, link, packet)
            .map_err(|error| match error {
                LinkError::TooLarge { len, .. } => TransportError::TooLarge { len },
                LinkError::UnknownLink { .. } | LinkError::Closed => {
                    TransportError::Unreachable { peer: to }
                }
            })
    }

    fn receive(&mut self, _now_micros: u64) -> Option<(PeerId, Vec<u8>)> {
        while let Some((from, packet)) = self.link.receive(Channel::Match) {
            let peer = self
                .links
                .iter()
                .position(|link| *link == from)
                .and_then(|index| u8::try_from(index).ok());
            match peer {
                Some(index) => return Some((PeerId(index), packet)),
                None => self.strays += 1,
            }
        }
        None
    }
}
