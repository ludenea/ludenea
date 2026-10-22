use ludenea_net::{
    MAX_PEERS, MAX_PLAYERS, MatchIdentity, MatchRules, PeerId, SessionSettings, SettingsError,
    Timeouts,
};

fn identity(owners: &[u8]) -> MatchIdentity {
    MatchIdentity {
        engine: "ludenea-test".to_string(),
        fingerprint: 7,
        scene_and_assets: 9,
        tick_rate: 60,
        players: owners.iter().copied().map(PeerId).collect(),
        rules: MatchRules::default(),
    }
}

fn settings(owners: &[u8]) -> SessionSettings {
    SessionSettings {
        local: PeerId(0),
        match_id: 1,
        identity: identity(owners),
        timeouts: Timeouts::default(),
    }
}

fn refusal(settings: &SessionSettings) -> SettingsError {
    settings
        .validate()
        .expect_err("the settings must be refused")
}

#[test]
fn defaults_are_the_spec_values() {
    let rules = MatchRules::default();
    assert_eq!(
        (
            rules.input_delay,
            rules.rollback_window,
            rules.hash_interval
        ),
        (2, 8, 30)
    );
    let timeouts = Timeouts::default();
    assert_eq!(timeouts.notice_after_micros, 750_000);
    assert_eq!(timeouts.disconnect_after_micros, 3_000_000);
    assert_eq!((MAX_PEERS, MAX_PLAYERS), (4, 4));
}

#[test]
fn default_rules_validate() {
    settings(&[0, 1])
        .validate()
        .expect("two peers, default rules");
    let mut four = settings(&[0, 1, 2, 3]);
    four.local = PeerId(3);
    four.validate().expect("four peers");
    settings(&[0, 1, 1, 0])
        .validate()
        .expect("several players per peer, any order");
}

#[test]
fn validate_refuses_no_players() {
    let error = refusal(&settings(&[]));
    assert!(matches!(error, SettingsError::PlayerCount { count: 0 }));
    assert!(error.to_string().contains("players") && error.to_string().contains('0'));
}

#[test]
fn validate_refuses_too_many_players() {
    let error = refusal(&settings(&[0, 1, 0, 1, 0]));
    assert!(matches!(error, SettingsError::PlayerCount { count: 5 }));
    assert!(error.to_string().contains('5'));
}

#[test]
fn validate_refuses_owners_that_are_not_contiguous_peers() {
    let error = refusal(&settings(&[0, 2]));
    assert!(matches!(
        error,
        SettingsError::PeerWithoutPlayer { peer: PeerId(1) }
    ));
    assert_eq!(
        error.to_string(),
        "peer 1 owns no player: the owners of players must be exactly the peers 0 to n"
    );
}

#[test]
fn validate_refuses_too_many_peers() {
    // Five players cannot fit anyway, so the peer bound is reached through a high owner.
    let error = refusal(&settings(&[0, 9]));
    assert!(matches!(
        error,
        SettingsError::PeerCount {
            highest_owner: PeerId(9)
        }
    ));
    assert_eq!(
        error.to_string(),
        "player owner peer 9 is out of range: a match has at most 4 peers (0 to 3)"
    );
}

#[test]
fn validate_refuses_a_single_peer() {
    for owners in [&[0][..], &[0, 0, 0, 0]] {
        let error = refusal(&settings(owners));
        assert_eq!(error, SettingsError::SinglePeer);
        assert_eq!(
            error.to_string(),
            "every player is owned by peer 0: a rollback session needs at least two peers, \
             a single-peer game steps its Simulation directly"
        );
    }
}

#[test]
fn validate_refuses_a_local_peer_outside_the_match() {
    let mut s = settings(&[0, 1]);
    s.local = PeerId(2);
    let error = refusal(&s);
    assert!(matches!(
        error,
        SettingsError::LocalOutOfRange {
            local: PeerId(2),
            peers: 2
        }
    ));
    assert_eq!(
        error.to_string(),
        "local peer 2 is not among the 2 peers of the match"
    );
}

#[test]
fn validate_refuses_a_zero_tick_rate() {
    let mut s = settings(&[0, 1]);
    s.identity.tick_rate = 0;
    let error = refusal(&s);
    assert!(matches!(error, SettingsError::TickRate));
    assert!(error.to_string().contains("tick_rate"));
}

#[test]
fn validate_refuses_an_empty_rollback_window() {
    let mut s = settings(&[0, 1]);
    s.identity.rules.rollback_window = 0;
    let error = refusal(&s);
    assert!(matches!(error, SettingsError::RollbackWindow { value: 0 }));
    assert!(error.to_string().contains("rollback_window"));
}

#[test]
fn validate_refuses_a_window_and_delay_beyond_one_message() {
    let mut s = settings(&[0, 1]);
    s.identity.rules.rollback_window = 30;
    s.identity.rules.input_delay = 1;
    s.validate()
        .expect("window + delay + 1 is exactly the limit");
    s.identity.rules.input_delay = 2;
    let error = refusal(&s);
    assert!(matches!(
        error,
        SettingsError::WindowAndDelay {
            rollback_window: 30,
            input_delay: 2
        }
    ));
    assert_eq!(
        error.to_string(),
        "rollback_window 30 plus input_delay 2 plus 1 exceeds 32 ticks"
    );
}

#[test]
fn validate_accepts_the_smallest_values() {
    let mut s = settings(&[0, 1]);
    s.identity.rules = MatchRules {
        input_delay: 0,
        rollback_window: 1,
        hash_interval: 1,
    };
    s.timeouts = Timeouts {
        notice_after_micros: 1,
        disconnect_after_micros: 2,
    };
    s.validate().expect("every field at its boundary");
}

#[test]
fn validate_reports_the_first_fault_in_order() {
    // Every rule is broken at once; fixing the reported one reveals the next in order.
    let mut s = settings(&[]);
    s.local = PeerId(9);
    s.identity.tick_rate = 0;
    s.identity.rules = MatchRules {
        input_delay: 40,
        rollback_window: 0,
        hash_interval: 0,
    };
    s.timeouts = Timeouts {
        notice_after_micros: 0,
        disconnect_after_micros: 0,
    };
    s.identity.engine = "e".repeat(65);
    assert!(matches!(
        refusal(&s),
        SettingsError::EngineTooLong { len: 65 }
    ));
    s.identity.engine = "ludenea-test".to_string();
    assert!(matches!(refusal(&s), SettingsError::PlayerCount { .. }));
    s.identity.players = vec![PeerId(0), PeerId(9)];
    assert!(matches!(refusal(&s), SettingsError::PeerCount { .. }));
    s.identity.players = vec![PeerId(0), PeerId(0)];
    assert_eq!(refusal(&s), SettingsError::SinglePeer);
    s.identity.players = vec![PeerId(0), PeerId(2)];
    assert!(matches!(
        refusal(&s),
        SettingsError::PeerWithoutPlayer { .. }
    ));
    s.identity.players = vec![PeerId(0), PeerId(1)];
    assert!(matches!(refusal(&s), SettingsError::LocalOutOfRange { .. }));
    s.local = PeerId(0);
    assert!(matches!(refusal(&s), SettingsError::TickRate));
    s.identity.tick_rate = 60;
    assert!(matches!(refusal(&s), SettingsError::RollbackWindow { .. }));
    s.identity.rules.rollback_window = 8;
    assert!(matches!(refusal(&s), SettingsError::WindowAndDelay { .. }));
    s.identity.rules.input_delay = 2;
    assert!(matches!(refusal(&s), SettingsError::HashInterval { .. }));
    s.identity.rules.hash_interval = 30;
    assert!(matches!(refusal(&s), SettingsError::Timeouts { .. }));
    s.timeouts = Timeouts::default();
    s.validate().expect("every fault fixed");
}

#[test]
fn validate_refuses_the_largest_input_delay_without_overflowing() {
    let mut s = settings(&[0, 1]);
    s.identity.rules.input_delay = u32::MAX;
    let error = refusal(&s);
    assert!(error.to_string().contains(&u32::MAX.to_string()));
    s.identity.rules.rollback_window = u32::MAX;
    s.identity.rules.input_delay = u32::MAX;
    refusal(&s);
}

#[test]
fn validate_refuses_a_zero_hash_interval() {
    let mut s = settings(&[0, 1]);
    s.identity.rules.hash_interval = 0;
    let error = refusal(&s);
    assert!(matches!(error, SettingsError::HashInterval { value: 0 }));
    assert!(error.to_string().contains("hash_interval"));
}

#[test]
fn validate_refuses_timeouts_out_of_order() {
    for (notice, disconnect) in [(0, 10), (10, 10), (20, 10)] {
        let mut s = settings(&[0, 1]);
        s.timeouts = Timeouts {
            notice_after_micros: notice,
            disconnect_after_micros: disconnect,
        };
        let error = refusal(&s);
        assert!(matches!(error, SettingsError::Timeouts { .. }));
        let text = error.to_string();
        assert!(text.contains(&notice.to_string()) && text.contains(&disconnect.to_string()));
    }
}
