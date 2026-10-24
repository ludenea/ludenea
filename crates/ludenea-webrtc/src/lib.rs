//! [`WebRtcLink`]: a [`Link`](ludenea_net::Link) over WebRTC data channels, through
//! matchbox_socket, natively (webrtc-rs on a thread of its own) and in the browser.
//!
//! A link joins the room of a signaling server ([`LinkSettings`]) and connects to every
//! other member: channel 0, unreliable and unordered, carries
//! [`Channel::Match`](ludenea_net::Channel::Match); channel 1, reliable and ordered, carries
//! [`Channel::Room`](ludenea_net::Channel::Room). What the server and the other members
//! send is untrusted: a handshake matchbox cannot use is dropped, never a panic.
//!
//! The link needs the `link` feature. Without it, and always on WebAssembly outside the
//! browser (`wasm32-wasip1`), the crate builds its settings alone
//! and links no WebRTC.

mod settings;

pub use settings::{DEFAULT_STUN, LinkSettings, LinkSettingsError};

#[cfg(all(feature = "link", not(target_os = "wasi")))]
pub use link::WebRtcLink;

#[cfg(all(feature = "link", not(target_arch = "wasm32")))]
mod native;
#[cfg(all(feature = "link", target_arch = "wasm32", not(target_os = "wasi")))]
mod web;

#[cfg(all(feature = "link", not(target_os = "wasi")))]
mod link {
    use std::collections::{BTreeSet, VecDeque};
    use std::sync::mpsc::{Receiver, TryRecvError};
    use std::time::Duration;

    use ludenea_net::{
        Channel, Link, LinkError, LinkEvent, LinkId, MAX_PACKET_BYTES, MAX_ROOM_MESSAGE_BYTES,
    };
    use matchbox_socket::{
        ChannelConfig, PeerId, PeerState, RtcIceServerConfig, WebRtcSocket, WebRtcSocketBuilder,
    };

    #[cfg(not(target_arch = "wasm32"))]
    use crate::native as platform;
    use crate::settings::LinkSettings;
    #[cfg(target_arch = "wasm32")]
    use crate::web as platform;

    /// How long the signaling server has to assign this link its id.
    const ASSIGN_TIMEOUT: Duration = Duration::from_secs(10);

    /// The socket's channel of each [`Channel`].
    fn index(channel: Channel) -> usize {
        match channel {
            Channel::Match => 0,
            Channel::Room => 1,
        }
    }

    /// The longest message each [`Channel`] carries.
    fn limit(channel: Channel) -> usize {
        match channel {
            Channel::Match => MAX_PACKET_BYTES,
            Channel::Room => MAX_ROOM_MESSAGE_BYTES,
        }
    }

    fn link_id(peer: PeerId) -> LinkId {
        LinkId(peer.0.into_bytes())
    }

    fn peer_id(link: LinkId) -> PeerId {
        PeerId(uuid::Uuid::from_bytes(link.0))
    }

    /// A [`Link`] to the members of one room over WebRTC.
    ///
    /// Its message loop runs on its own: natively on a thread named `ludenea-webrtc`, in the
    /// browser as a task of the page. Dropping the link closes the socket, which ends the loop.
    pub struct WebRtcLink {
        socket: WebRtcSocket,
        /// The loop's end: `Ok` when it ended without an error, else why it ended.
        ended: Receiver<Result<(), String>>,
        settings: LinkSettings,
        opened: platform::Clock,
        assigned: bool,
        /// The peers reported `Connected` and not since `Disconnected`.
        connected: BTreeSet<LinkId>,
        closed: bool,
        /// What arrived on each socket channel and was not read yet.
        inboxes: [VecDeque<(LinkId, Vec<u8>)>; 2],
        /// Messages dropped on arrival for exceeding their channel's limit.
        oversized: u64,
    }

    impl WebRtcLink {
        /// Joins the room of `settings` (or creates it, as its host) and starts the message
        /// loop. Failures arrive through [`Link::poll`] as [`LinkEvent::Closed`]: a refused
        /// room is final, the link makes one connection attempt.
        pub fn open(settings: LinkSettings) -> WebRtcLink {
            let (socket, future) = WebRtcSocketBuilder::new(settings.room_url())
                .ice_server(RtcIceServerConfig {
                    urls: settings.stun().to_vec(),
                    username: None,
                    credential: None,
                })
                .reconnect_attempts(Some(1))
                .add_channel(ChannelConfig::unreliable())
                .add_channel(ChannelConfig::reliable())
                .build();
            let ended = platform::run(future, settings.clone());
            WebRtcLink {
                socket,
                ended,
                settings,
                opened: platform::Clock::start(),
                assigned: false,
                connected: BTreeSet::new(),
                closed: false,
                inboxes: [VecDeque::new(), VecDeque::new()],
                oversized: 0,
            }
        }

        /// Why the link is gone, if it is: the loop ended, or no id came in time.
        fn end(&mut self) -> Option<String> {
            let signaling = self.settings.signaling();
            match self.ended.try_recv() {
                Ok(Ok(())) => Some(format!("lost the signaling server at {signaling}")),
                Ok(Err(reason)) => Some(reason),
                Err(TryRecvError::Disconnected) => Some(format!(
                    "the WebRTC link to {signaling} stopped without saying why"
                )),
                Err(TryRecvError::Empty)
                    if !self.assigned && self.opened.elapsed() >= ASSIGN_TIMEOUT =>
                {
                    Some(format!(
                        "no answer from the signaling server at {signaling} within 10 s"
                    ))
                }
                Err(TryRecvError::Empty) => None,
            }
        }
    }

    impl Link for WebRtcLink {
        fn poll(&mut self) -> Vec<LinkEvent> {
            if self.closed {
                return Vec::new();
            }
            let mut events = Vec::new();
            if !self.assigned
                && let Some(id) = self.socket.id()
            {
                self.assigned = true;
                events.push(LinkEvent::Assigned(link_id(id)));
            }
            // Read before the peers, so every change the loop made before it ended is seen.
            let end = self.end();
            // An error means the loop ended; the end above or on the next poll reports it.
            if let Ok(changes) = self.socket.try_update_peers() {
                for (peer, state) in changes {
                    let link = link_id(peer);
                    match state {
                        PeerState::Connected => {
                            if self.connected.insert(link) {
                                events.push(LinkEvent::Connected(link));
                            }
                        }
                        PeerState::Disconnected => {
                            if self.connected.remove(&link) {
                                events.push(LinkEvent::Disconnected(link));
                            }
                        }
                    }
                }
            }
            if let Some(reason) = end {
                events.extend(
                    std::mem::take(&mut self.connected)
                        .into_iter()
                        .map(LinkEvent::Disconnected),
                );
                events.push(LinkEvent::Closed(reason));
                self.closed = true;
                self.socket.close();
            }
            events
        }

        fn send(&mut self, channel: Channel, to: LinkId, bytes: Vec<u8>) -> Result<(), LinkError> {
            if self.closed {
                return Err(LinkError::Closed);
            }
            if !self.connected.contains(&to) {
                return Err(LinkError::UnknownLink { link: to });
            }
            let max = limit(channel);
            if bytes.len() > max {
                return Err(LinkError::TooLarge {
                    len: bytes.len(),
                    max,
                });
            }
            self.socket
                .get_channel_mut(index(channel))
                .map_err(|_| LinkError::Closed)?
                .try_send(bytes.into_boxed_slice(), peer_id(to))
                .map_err(|_| LinkError::Closed)
        }

        fn receive(&mut self, channel: Channel) -> Option<(LinkId, Vec<u8>)> {
            let index = index(channel);
            let max = limit(channel);
            let inbox = &mut self.inboxes[index];
            if inbox.is_empty()
                && let Ok(arrived) = self.socket.get_channel_mut(index)
            {
                for (peer, packet) in arrived.receive() {
                    if packet.len() > max {
                        self.oversized += 1;
                    } else {
                        inbox.push_back((link_id(peer), packet.into_vec()));
                    }
                }
            }
            inbox.pop_front()
        }

        /// The messages that arrived longer than their channel's limit
        /// ([`MAX_PACKET_BYTES`] for [`Channel::Match`], [`MAX_ROOM_MESSAGE_BYTES`] for
        /// [`Channel::Room`]), which [`Link::receive`] dropped.
        fn oversized(&self) -> u64 {
            self.oversized
        }
    }
}
