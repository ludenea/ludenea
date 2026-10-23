//! The rooms of the listener: who may enter, and what each member is sent.
//!
//! Every change happens under one lock, so admission never overfills a room, and a member is
//! announced in the same section that makes it reachable: no signal to it can be lost.

use std::collections::{BTreeMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use axum::http::StatusCode;
use ludenea_net::RoomCode;
use matchbox_protocol::{JsonPeerEvent, PeerId};
use thiserror::Error;
use tokio::sync::mpsc;

use crate::Limits;
use crate::limits::{Refusals, client_key};

/// Why a request for a room is refused, before its WebSocket opens.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum Refusal {
    #[error("room {0} is in use")]
    InUse(RoomCode),
    #[error("no room {0}")]
    Missing(RoomCode),
    #[error("the host of room {0} left")]
    HostLeft(RoomCode),
    #[error("room {0} is full")]
    Full(RoomCode),
    #[error("this server holds its maximum of {0} rooms")]
    MaxRooms(usize),
    #[error("this address hosts {0} rooms already")]
    Hosting(usize),
    #[error("too many refused joins from this address; wait a minute")]
    TooManyRefused,
}

impl Refusal {
    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Refusal::InUse(_) | Refusal::Full(_) => StatusCode::CONFLICT,
            Refusal::Missing(_) => StatusCode::NOT_FOUND,
            Refusal::HostLeft(_) => StatusCode::GONE,
            Refusal::MaxRooms(_) => StatusCode::SERVICE_UNAVAILABLE,
            Refusal::Hosting(_) | Refusal::TooManyRefused => StatusCode::TOO_MANY_REQUESTS,
        }
    }

    /// Whether this refusal counts toward [`Limits::refused_joins_per_minute`]: those that
    /// tell a guess of a code from another.
    fn counts(&self) -> bool {
        matches!(
            self,
            Refusal::InUse(_) | Refusal::Missing(_) | Refusal::HostLeft(_) | Refusal::Full(_)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Host {
    /// Its place is reserved; its WebSocket is not open yet.
    Arriving,
    Present(PeerId),
    Left,
}

#[derive(Debug)]
struct Room {
    size: usize,
    host: Host,
    host_address: IpAddr,
    /// Places admitted whose WebSocket is not open yet.
    reserved: usize,
    members: BTreeMap<PeerId, mpsc::Sender<String>>,
}

impl Room {
    fn occupants(&self) -> usize {
        self.reserved + self.members.len()
    }

    /// Queues each event for its member, in order. A member whose queue is full is removed,
    /// which closes its connection, and its leaving is announced to the others in turn.
    fn deliver(&mut self, events: impl IntoIterator<Item = (PeerId, String)>) {
        let mut pending: VecDeque<(PeerId, String)> = events.into_iter().collect();
        while let Some((to, text)) = pending.pop_front() {
            let Some(queue) = self.members.get(&to) else {
                continue;
            };
            if queue.try_send(text).is_err() {
                pending.extend(self.remove(to));
            }
        }
    }

    /// Removes the member `id`, and returns the announcement of its leaving to the others.
    fn remove(&mut self, id: PeerId) -> Vec<(PeerId, String)> {
        if self.members.remove(&id).is_none() {
            return Vec::new();
        }
        if self.host == Host::Present(id) {
            self.host = Host::Left;
        }
        let Some(left) = text(&JsonPeerEvent::PeerLeft(id)) else {
            return Vec::new();
        };
        self.members
            .keys()
            .map(|&other| (other, left.clone()))
            .collect()
    }
}

/// An event as matchbox_socket reads it, or `None` if it cannot be written, which a value
/// read from JSON never is.
fn text(event: &JsonPeerEvent) -> Option<String> {
    serde_json::to_string(event).ok()
}

/// Every room of the listener and the refusals of each address.
#[derive(Debug)]
pub(crate) struct Rooms {
    rooms: BTreeMap<RoomCode, Room>,
    refusals: Refusals,
    limits: Limits,
}

impl Rooms {
    pub(crate) fn new(limits: Limits) -> Rooms {
        Rooms {
            rooms: BTreeMap::new(),
            refusals: Refusals::default(),
            limits,
        }
    }

    /// Reserves a place in room `code` for a client at `address`: a new room of `size`
    /// places when `size` is given (a host), else a place in an existing room.
    fn admit(
        &mut self,
        code: &RoomCode,
        size: Option<usize>,
        address: IpAddr,
        now: Instant,
    ) -> Result<(), Refusal> {
        let address = client_key(address);
        if self.refusals.count(address, now) >= self.limits.refused_joins_per_minute {
            return Err(Refusal::TooManyRefused);
        }
        let result = self.reserve(code, size, address);
        if let Err(refusal) = &result
            && refusal.counts()
        {
            self.refusals.add(address, now);
        }
        result
    }

    fn reserve(
        &mut self,
        code: &RoomCode,
        size: Option<usize>,
        address: IpAddr,
    ) -> Result<(), Refusal> {
        match size {
            Some(size) => {
                if self.rooms.contains_key(code) {
                    return Err(Refusal::InUse(code.clone()));
                }
                if self.rooms.len() >= self.limits.max_rooms {
                    return Err(Refusal::MaxRooms(self.limits.max_rooms));
                }
                let hosted = self
                    .rooms
                    .values()
                    .filter(|room| room.host_address == address)
                    .count();
                if hosted >= self.limits.rooms_per_address {
                    return Err(Refusal::Hosting(hosted));
                }
                self.rooms.insert(
                    code.clone(),
                    Room {
                        size,
                        host: Host::Arriving,
                        host_address: address,
                        reserved: 1,
                        members: BTreeMap::new(),
                    },
                );
            }
            None => {
                let room = self
                    .rooms
                    .get_mut(code)
                    .ok_or_else(|| Refusal::Missing(code.clone()))?;
                if room.host == Host::Left {
                    return Err(Refusal::HostLeft(code.clone()));
                }
                if room.occupants() >= room.size {
                    return Err(Refusal::Full(code.clone()));
                }
                room.reserved += 1;
            }
        }
        Ok(())
    }

    /// Turns a reserved place of room `code` into a member: it is sent its id first, then
    /// every other member hears of it. `None` if the room is gone, which a reservation
    /// prevents.
    fn arrive(&mut self, code: &RoomCode, host: bool) -> Option<(PeerId, mpsc::Receiver<String>)> {
        let queue = self.limits.queue.max(1);
        let room = self.rooms.get_mut(code)?;
        room.reserved = room.reserved.saturating_sub(1);
        let id = PeerId(uuid::Uuid::new_v4());
        let (sender, receiver) = mpsc::channel(queue);
        let others: Vec<PeerId> = room.members.keys().copied().collect();
        room.members.insert(id, sender);
        if host {
            room.host = Host::Present(id);
        }
        let assigned = text(&JsonPeerEvent::IdAssigned(id)).map(|text| (id, text));
        let announced = text(&JsonPeerEvent::NewPeer(id))
            .into_iter()
            .flat_map(|text| others.iter().map(move |&other| (other, text.clone())));
        room.deliver(assigned.into_iter().chain(announced));
        self.forget_if_empty(code);
        Some((id, receiver))
    }

    /// Releases a reserved place of room `code` whose WebSocket never opened.
    fn cancel(&mut self, code: &RoomCode, host: bool) {
        if let Some(room) = self.rooms.get_mut(code) {
            room.reserved = room.reserved.saturating_sub(1);
            if host {
                room.host = Host::Left;
            }
        }
        self.forget_if_empty(code);
    }

    /// Relays `data` from member `from` to member `to` of room `code`; dropped unless both
    /// are members of it.
    fn relay(&mut self, code: &RoomCode, from: PeerId, to: PeerId, data: serde_json::Value) {
        let Some(room) = self.rooms.get_mut(code) else {
            return;
        };
        if !room.members.contains_key(&from) || !room.members.contains_key(&to) {
            return;
        }
        if let Some(text) = text(&JsonPeerEvent::Signal { sender: from, data }) {
            room.deliver([(to, text)]);
        }
        self.forget_if_empty(code);
    }

    /// Member `id` of room `code` left, if it was still there: the others hear of it.
    fn leave(&mut self, code: &RoomCode, id: PeerId) {
        if let Some(room) = self.rooms.get_mut(code) {
            let announcements = room.remove(id);
            room.deliver(announcements);
        }
        self.forget_if_empty(code);
    }

    fn forget_if_empty(&mut self, code: &RoomCode) {
        if self
            .rooms
            .get(code)
            .is_some_and(|room| room.occupants() == 0)
        {
            self.rooms.remove(code);
        }
    }
}

/// The rooms, shared by every connection.
pub(crate) type Shared = Arc<Mutex<Rooms>>;

pub(crate) fn lock(rooms: &Shared) -> MutexGuard<'_, Rooms> {
    // Nothing under the lock panics; a poisoned lock still holds consistent rooms.
    rooms
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A place reserved in a room for a client whose WebSocket is not open yet. Dropped before
/// [`Reservation::arrive`], it releases the place, so an upgrade that never completes takes
/// nothing.
pub(crate) struct Reservation {
    rooms: Shared,
    code: RoomCode,
    host: bool,
    held: bool,
}

impl Reservation {
    /// Reserves a place in room `code` for a client at `address`; a new room of `size` places
    /// when `size` is given.
    pub(crate) fn new(
        rooms: &Shared,
        code: RoomCode,
        size: Option<usize>,
        address: IpAddr,
        now: Instant,
    ) -> Result<Reservation, Refusal> {
        lock(rooms).admit(&code, size, address, now)?;
        Ok(Reservation {
            rooms: Arc::clone(rooms),
            code,
            host: size.is_some(),
            held: true,
        })
    }

    /// The client's WebSocket is open: it becomes a member.
    pub(crate) fn arrive(mut self) -> Option<Membership> {
        self.held = false;
        let (id, events) = lock(&self.rooms).arrive(&self.code, self.host)?;
        Some(Membership {
            rooms: Arc::clone(&self.rooms),
            code: self.code.clone(),
            id,
            events,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.held {
            lock(&self.rooms).cancel(&self.code, self.host);
        }
    }
}

/// A member of a room, until it is dropped, which announces its leaving.
pub(crate) struct Membership {
    rooms: Shared,
    code: RoomCode,
    id: PeerId,
    /// The events queued for this member. It ends when the member was removed for falling
    /// behind.
    pub(crate) events: mpsc::Receiver<String>,
}

impl Membership {
    pub(crate) fn relay(&self, to: PeerId, data: serde_json::Value) {
        lock(&self.rooms).relay(&self.code, self.id, to, data);
    }
}

impl Drop for Membership {
    fn drop(&mut self) {
        lock(&self.rooms).leave(&self.code, self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Shared {
        Arc::new(Mutex::new(Rooms::new(Limits::default())))
    }

    fn reserve(rooms: &Shared, size: Option<usize>) -> Result<Reservation, Refusal> {
        let code = RoomCode::parse("ABCDEF").unwrap();
        #[expect(clippy::disallowed_methods, reason = "a test of network pacing")]
        let now = Instant::now();
        Reservation::new(rooms, code, size, IpAddr::from([127, 0, 0, 1]), now)
    }

    #[test]
    fn a_reserved_place_counts_until_it_is_released() {
        let rooms = shared();
        let _host = reserve(&rooms, Some(2)).unwrap();
        let joiner = reserve(&rooms, None).unwrap();
        // Neither WebSocket is open, yet the room is full.
        assert!(matches!(reserve(&rooms, None), Err(Refusal::Full(_))));
        drop(joiner);
        let _joiner = reserve(&rooms, None).unwrap();
    }

    #[test]
    fn a_host_that_never_arrives_leaves_its_room() {
        let rooms = shared();
        let host = reserve(&rooms, Some(3)).unwrap();
        let joiner = reserve(&rooms, None).unwrap().arrive().unwrap();
        drop(host);
        assert!(matches!(reserve(&rooms, None), Err(Refusal::HostLeft(_))));
        drop(joiner);
        // Empty, the room is forgotten.
        assert!(matches!(reserve(&rooms, None), Err(Refusal::Missing(_))));
    }
}
