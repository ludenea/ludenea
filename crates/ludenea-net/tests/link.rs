use ludenea_net::{
    Channel, Link, LinkError, LinkEvent, LinkId, LinkTransport, MAX_PACKET_BYTES,
    MAX_ROOM_MESSAGE_BYTES, MemoryMesh, PeerId, Transport, TransportError,
};

fn drain(link: &mut impl Link, channel: Channel) -> Vec<(LinkId, Vec<u8>)> {
    std::iter::from_fn(|| link.receive(channel)).collect()
}

#[test]
fn mesh_assigns_then_connects_every_member() {
    let mesh = MemoryMesh::new();
    let mut a = mesh.join();
    let mut b = mesh.join();
    let mut c = mesh.join();
    let events_a = a.poll();
    let events_b = b.poll();
    let events_c = c.poll();
    let LinkEvent::Assigned(id_a) = events_a[0] else {
        panic!("first event is the assignment: {events_a:?}");
    };
    let LinkEvent::Assigned(id_b) = events_b[0] else {
        panic!("first event is the assignment: {events_b:?}");
    };
    let LinkEvent::Assigned(id_c) = events_c[0] else {
        panic!("first event is the assignment: {events_c:?}");
    };
    assert!(id_a != id_b && id_b != id_c && id_a != id_c);
    assert_eq!(
        events_a[1..],
        [LinkEvent::Connected(id_b), LinkEvent::Connected(id_c)]
    );
    assert_eq!(
        events_b[1..],
        [LinkEvent::Connected(id_a), LinkEvent::Connected(id_c)]
    );
    assert_eq!(
        events_c[1..],
        [LinkEvent::Connected(id_a), LinkEvent::Connected(id_b)]
    );
    assert!(a.poll().is_empty());
}

#[test]
fn room_channel_keeps_the_order() {
    let mesh = MemoryMesh::new();
    let mut a = mesh.join();
    let mut b = mesh.join();
    let LinkEvent::Assigned(id_a) = a.poll()[0] else {
        panic!("assigned first");
    };
    let LinkEvent::Assigned(id_b) = b.poll()[0] else {
        panic!("assigned first");
    };
    for i in 0..5u8 {
        a.send(Channel::Room, id_b, vec![i]).unwrap();
    }
    a.send(Channel::Match, id_b, vec![9]).unwrap();
    let expected: Vec<_> = (0..5u8).map(|i| (id_a, vec![i])).collect();
    assert_eq!(drain(&mut b, Channel::Room), expected);
    assert_eq!(drain(&mut b, Channel::Match), vec![(id_a, vec![9])]);
    assert_eq!(a.receive(Channel::Room), None);
}

#[test]
fn dropping_a_link_disconnects_it() {
    let mesh = MemoryMesh::new();
    let mut a = mesh.join();
    let mut b = mesh.join();
    let _ = a.poll();
    let LinkEvent::Assigned(id_b) = b.poll()[0] else {
        panic!("assigned first");
    };
    drop(b);
    assert_eq!(a.poll(), vec![LinkEvent::Disconnected(id_b)]);
    assert_eq!(
        a.send(Channel::Room, id_b, vec![1]),
        Err(LinkError::UnknownLink { link: id_b })
    );
}

#[test]
fn a_partial_mesh_connects_only_the_listed_members() {
    let mesh = MemoryMesh::new();
    let mut a = mesh.join();
    let mut b = mesh.join();
    let LinkEvent::Assigned(id_a) = a.poll()[0] else {
        panic!("assigned first");
    };
    let LinkEvent::Assigned(id_b) = b.poll()[0] else {
        panic!("assigned first");
    };
    let mut c = mesh.join_with(&[id_a]);
    let events = c.poll();
    let LinkEvent::Assigned(id_c) = events[0] else {
        panic!("assigned first");
    };
    assert_eq!(events[1..], [LinkEvent::Connected(id_a)]);
    assert_eq!(a.poll(), vec![LinkEvent::Connected(id_c)]);
    assert!(b.poll().is_empty());
    assert_eq!(
        c.send(Channel::Match, id_b, vec![1]),
        Err(LinkError::UnknownLink { link: id_b })
    );
    // An id nobody holds is ignored rather than invented.
    let mut d = mesh.join_with(&[LinkId([0xee; 16])]);
    assert_eq!(d.poll().len(), 1);
}

#[test]
fn disconnect_cuts_two_members_apart() {
    let mesh = MemoryMesh::new();
    let mut a = mesh.join();
    let mut b = mesh.join();
    let LinkEvent::Assigned(id_a) = a.poll()[0] else {
        panic!("assigned first");
    };
    let LinkEvent::Assigned(id_b) = b.poll()[0] else {
        panic!("assigned first");
    };
    let _ = b.poll();
    mesh.disconnect(id_a, id_b);
    assert_eq!(a.poll(), vec![LinkEvent::Disconnected(id_b)]);
    assert_eq!(b.poll(), vec![LinkEvent::Disconnected(id_a)]);
    mesh.disconnect(id_a, id_b);
    assert!(a.poll().is_empty());
    assert!(a.send(Channel::Room, id_b, vec![1]).is_err());
    mesh.connect(id_a, id_b);
    assert_eq!(a.poll(), vec![LinkEvent::Connected(id_b)]);
    assert_eq!(b.poll(), vec![LinkEvent::Connected(id_a)]);
    mesh.connect(id_a, id_b);
    assert!(a.poll().is_empty());
    a.send(Channel::Room, id_b, vec![1]).unwrap();
}

#[test]
fn sending_to_an_unconnected_link_is_refused() {
    let mesh = MemoryMesh::new();
    let mut a = mesh.join();
    let _ = a.poll();
    let stranger = LinkId([7; 16]);
    for channel in [Channel::Room, Channel::Match] {
        assert_eq!(
            a.send(channel, stranger, vec![1]),
            Err(LinkError::UnknownLink { link: stranger })
        );
    }
}

#[test]
fn oversized_messages_are_refused_per_channel() {
    let mesh = MemoryMesh::new();
    let mut a = mesh.join();
    let mut b = mesh.join();
    let _ = a.poll();
    let LinkEvent::Assigned(id_b) = b.poll()[0] else {
        panic!("assigned first");
    };
    a.send(Channel::Room, id_b, vec![0; MAX_ROOM_MESSAGE_BYTES])
        .unwrap();
    assert_eq!(
        a.send(Channel::Room, id_b, vec![0; MAX_ROOM_MESSAGE_BYTES + 1]),
        Err(LinkError::TooLarge {
            len: MAX_ROOM_MESSAGE_BYTES + 1,
            max: MAX_ROOM_MESSAGE_BYTES
        })
    );
    a.send(Channel::Match, id_b, vec![0; MAX_PACKET_BYTES])
        .unwrap();
    assert_eq!(
        a.send(Channel::Match, id_b, vec![0; MAX_PACKET_BYTES + 1]),
        Err(LinkError::TooLarge {
            len: MAX_PACKET_BYTES + 1,
            max: MAX_PACKET_BYTES
        })
    );
    assert_eq!(drain(&mut b, Channel::Room).len(), 1);
    assert_eq!(drain(&mut b, Channel::Match).len(), 1);
}

#[test]
fn link_id_displays_as_a_uuid_and_round_trips() {
    let id = LinkId([
        0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef,
    ]);
    assert_eq!(id.to_string(), "12345678-9abc-def0-0123-456789abcdef");
    let bytes = postcard::to_allocvec(&id).unwrap();
    assert_eq!(bytes, id.0);
    assert_eq!(postcard::from_bytes::<LinkId>(&bytes).unwrap(), id);
}

fn three_links() -> (
    ludenea_net::MemoryLink,
    ludenea_net::MemoryLink,
    ludenea_net::MemoryLink,
    [LinkId; 3],
) {
    let mesh = MemoryMesh::new();
    let mut links = [mesh.join(), mesh.join(), mesh.join()];
    let ids = links.each_mut().map(|link| {
        let LinkEvent::Assigned(id) = link.poll()[0] else {
            panic!("assigned first");
        };
        id
    });
    let [a, b, c] = links;
    (a, b, c, ids)
}

#[test]
fn link_transport_maps_peers_to_links() {
    let (a, b, _c, [id_a, id_b, _]) = three_links();
    let mut ta = LinkTransport::new(a, vec![id_a, id_b]);
    let mut tb = LinkTransport::new(b, vec![id_a, id_b]);
    ta.send(PeerId(1), vec![5], 0).unwrap();
    assert_eq!(tb.receive(0), Some((PeerId(0), vec![5])));
    assert_eq!(tb.receive(0), None);
    assert_eq!(
        ta.send(PeerId(2), vec![1], 0),
        Err(TransportError::UnknownPeer { peer: PeerId(2) })
    );
    assert_eq!(
        ta.send(PeerId(1), vec![0; MAX_PACKET_BYTES + 1], 0),
        Err(TransportError::TooLarge {
            len: MAX_PACKET_BYTES + 1
        })
    );
    assert!(ta.link_mut().poll().is_empty());
}

#[test]
fn link_transport_drops_and_counts_strangers() {
    let (mut a, b, mut c, [id_a, id_b, _]) = three_links();
    let mut tb = LinkTransport::new(b, vec![id_a, id_b]);
    a.send(Channel::Match, id_b, vec![1]).unwrap();
    c.send(Channel::Match, id_b, vec![2]).unwrap();
    c.send(Channel::Match, id_b, vec![3]).unwrap();
    a.send(Channel::Match, id_b, vec![4]).unwrap();
    assert_eq!(tb.receive(0), Some((PeerId(0), vec![1])));
    assert_eq!(tb.strays(), 0);
    assert_eq!(tb.receive(0), Some((PeerId(0), vec![4])));
    assert_eq!(tb.strays(), 2);
    assert_eq!(tb.receive(0), None);
}

#[test]
fn link_transport_reports_unreachable_peers() {
    let mesh = MemoryMesh::new();
    let mut a = mesh.join();
    let mut b = mesh.join();
    let LinkEvent::Assigned(id_a) = a.poll()[0] else {
        panic!("assigned first");
    };
    let LinkEvent::Assigned(id_b) = b.poll()[0] else {
        panic!("assigned first");
    };
    let mut ta = LinkTransport::new(a, vec![id_a, id_b]);
    mesh.disconnect(id_a, id_b);
    assert_eq!(
        ta.send(PeerId(1), vec![1], 0),
        Err(TransportError::Unreachable { peer: PeerId(1) })
    );
}

#[test]
fn link_transport_works_over_a_boxed_link() {
    let (a, b, _c, [id_a, id_b, _]) = three_links();
    let boxed_a: Box<dyn Link> = Box::new(a);
    let mut ta = LinkTransport::new(boxed_a, vec![id_a, id_b]);
    let mut tb = LinkTransport::new(Box::new(b), vec![id_a, id_b]);
    ta.send(PeerId(1), vec![8], 0).unwrap();
    assert_eq!(tb.receive(0), Some((PeerId(0), vec![8])));
    ta.link_mut().send(Channel::Room, id_b, vec![1]).unwrap();
    assert_eq!(tb.link_mut().receive(Channel::Room), Some((id_a, vec![1])));
}
