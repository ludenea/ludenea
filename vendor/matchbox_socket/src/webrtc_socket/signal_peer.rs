use crate::webrtc_socket::{PeerId, PeerRequest, PeerSignal};
use futures_channel::mpsc::UnboundedSender;
use log::warn;

#[derive(Debug, Clone)]
pub struct SignalPeer {
    pub id: PeerId,
    pub sender: UnboundedSender<PeerRequest>,
}

impl SignalPeer {
    pub fn send(&self, signal: PeerSignal) {
        let req = PeerRequest::Signal {
            receiver: self.id,
            data: signal,
        };
        if self.sender.unbounded_send(req).is_err() {
            warn!(
                "dropping a signal for peer {}: the signaling loop ended",
                self.id
            );
        }
    }

    pub fn new(id: PeerId, sender: UnboundedSender<PeerRequest>) -> Self {
        Self { id, sender }
    }
}
