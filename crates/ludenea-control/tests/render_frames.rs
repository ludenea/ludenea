//! The render frames a host subscribed to `RenderFrames` pushes as play runs ticks.

use std::collections::BTreeSet;

use boxes::BoxesInput;
use ludenea::control::Event;
use ludenea::control::*;
use ludenea::draw::frame::RenderFrame;
use ludenea::prelude::*;

const LAYER: &str = "physics.colliders";

fn ok(host: &mut Host, command: Command) -> Reply {
    host.handle(command).unwrap()
}

/// A boxes host with a scene and 400 ticks of recorded inputs loaded.
fn boxes_host() -> Host {
    let mut host = Host::new(boxes::app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: boxes::generate_scene(30, 7),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    let mut log = InputLog::new(1);
    for _ in 0..400 {
        log.record(&[BoxesInput::default()]);
    }
    ok(
        &mut host,
        Command::LoadInputs {
            log: Bytes(log.encode()),
        },
    );
    host
}

fn subscribe(host: &mut Host) {
    ok(
        host,
        Command::Subscribe {
            topic: Topic::RenderFrames,
        },
    );
}

fn set_layers(host: &mut Host, layers: &[&str]) {
    let debug_layers = layers.iter().map(|layer| layer.to_string()).collect();
    assert_eq!(
        ok(host, Command::SetPushedLayers { debug_layers }),
        Reply::Done
    );
}

/// Plays, and drops the events playing caused.
fn play(host: &mut Host) {
    ok(host, Command::Play);
    host.advance(0.0);
}

/// Lets the time of `ticks` ticks pass, plus `extra` of a tick.
fn advance(host: &mut Host, ticks: u32, extra: f64) -> Vec<Event> {
    let rate = f64::from(host.game_info().tick_rate);
    host.advance((f64::from(ticks) + extra) / rate)
}

/// The pushed frames of `events` and their progress, one entry per `RenderFrames` event.
fn pushed(events: &[Event]) -> Vec<(Vec<RenderFrame>, f64)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::RenderFrames { frames, progress } => Some((frames.clone(), *progress)),
            _ => None,
        })
        .collect()
}

fn ticks_of(frames: &[RenderFrame]) -> Vec<u64> {
    frames.iter().map(|frame| frame.tick).collect()
}

#[test]
fn subscribed_advance_pushes_the_tick_it_ran() {
    let mut host = boxes_host();
    subscribe(&mut host);
    play(&mut host);
    let [(frames, _)] = pushed(&advance(&mut host, 1, 0.0)).try_into().unwrap();
    assert_eq!(ticks_of(&frames), [1]);
    let Reply::RenderFrame { frame } = ok(
        &mut host,
        Command::GetRenderFrame {
            debug_layers: Vec::new(),
        },
    ) else {
        panic!()
    };
    assert_eq!(frames[0], frame);
}

#[test]
fn advance_of_three_ticks_pushes_the_last_two_oldest_first() {
    let mut host = boxes_host();
    subscribe(&mut host);
    play(&mut host);
    let [(frames, _)] = pushed(&advance(&mut host, 3, 0.0)).try_into().unwrap();
    assert_eq!(ticks_of(&frames), [2, 3]);
    let [(frames, _)] = pushed(&advance(&mut host, 2, 0.0)).try_into().unwrap();
    assert_eq!(ticks_of(&frames), [4, 5]);
}

#[test]
fn frames_follow_the_ticked_events_of_the_same_advance() {
    let mut host = boxes_host();
    subscribe(&mut host);
    play(&mut host);
    let events = advance(&mut host, 3, 0.0);
    let position = |wanted: fn(&Event) -> bool| events.iter().rposition(wanted).unwrap();
    let last_ticked = position(|event| matches!(event, Event::Ticked { .. }));
    let frames = position(|event| matches!(event, Event::RenderFrames { .. }));
    assert!(last_ticked < frames, "{events:?}");
}

#[test]
fn no_tick_no_frames() {
    let mut host = boxes_host();
    subscribe(&mut host);
    assert!(pushed(&advance(&mut host, 2, 0.0)).is_empty(), "paused");
    play(&mut host);
    assert!(
        pushed(&advance(&mut host, 0, 0.5)).is_empty(),
        "half a tick"
    );
    assert_eq!(pushed(&advance(&mut host, 0, 0.5)).len(), 1);
}

#[test]
fn unsubscribed_host_pushes_nothing() {
    let mut host = boxes_host();
    play(&mut host);
    assert!(pushed(&advance(&mut host, 2, 0.0)).is_empty());
    subscribe(&mut host);
    assert_eq!(pushed(&advance(&mut host, 2, 0.0)).len(), 1);
    ok(
        &mut host,
        Command::Unsubscribe {
            topic: Topic::RenderFrames,
        },
    );
    assert!(pushed(&advance(&mut host, 2, 0.0)).is_empty());
}

#[test]
fn pushed_frames_hold_only_the_set_layers() {
    let mut host = boxes_host();
    subscribe(&mut host);
    play(&mut host);
    let [(frames, _)] = pushed(&advance(&mut host, 1, 0.0)).try_into().unwrap();
    assert!(frames[0].debug.layers.is_empty(), "none until set");

    set_layers(&mut host, &[LAYER]);
    let [(frames, _)] = pushed(&advance(&mut host, 2, 0.0)).try_into().unwrap();
    for frame in &frames {
        let layers: Vec<&str> = frame.debug.layers.keys().map(String::as_str).collect();
        assert_eq!(layers, [LAYER]);
    }
}

#[test]
fn progress_matches_tick_progress() {
    let mut host = boxes_host();
    subscribe(&mut host);
    play(&mut host);
    let [(_, progress)] = pushed(&advance(&mut host, 1, 0.25)).try_into().unwrap();
    assert_eq!(progress, host.tick_progress());
    assert!((progress - 0.25).abs() < 1e-6, "{progress}");
}

/// A game whose extractor panics on the frame of tick 5.
fn failing_app() -> App {
    let mut app = App::new();
    app.extractor(|_, frame| {
        if frame.tick == 5 {
            panic!("no frame at five");
        }
    });
    app
}

/// Plays 8 ticks one advance at a time: the console errors and the ticks of the frames.
fn play_eight(host: &mut Host) -> (Vec<String>, Vec<u64>) {
    let (mut errors, mut frame_ticks) = (Vec::new(), Vec::new());
    for _ in 0..8 {
        for event in advance(host, 1, 0.0) {
            match event {
                Event::Console {
                    level: ConsoleLevel::Error,
                    text,
                } => errors.push(text),
                Event::RenderFrames { frames, .. } => frame_ticks.extend(ticks_of(&frames)),
                _ => {}
            }
        }
    }
    (errors, frame_ticks)
}

#[test]
fn a_failing_extraction_logs_once_and_play_continues() {
    let mut host = Host::new(failing_app).unwrap();
    subscribe(&mut host);
    play(&mut host);
    let (errors, frame_ticks) = play_eight(&mut host);
    assert_eq!(frame_ticks, [1, 2, 3, 4, 6, 7, 8]);
    assert_eq!(host.tick(), 8, "play continues");
    let [error] = errors.as_slice() else {
        panic!("{errors:?}")
    };
    assert!(
        error.starts_with("render frame of tick 5: ") && error.contains("no frame at five"),
        "{error}"
    );

    // The same failure again is not reported again.
    ok(&mut host, Command::Seek { tick: 0 });
    play(&mut host);
    let (errors, frame_ticks) = play_eight(&mut host);
    assert_eq!(frame_ticks, [1, 2, 3, 4, 6, 7, 8]);
    assert!(errors.is_empty(), "{errors:?}");
}

/// The hash after playing to tick 300, subscribed to `RenderFrames` or not.
fn hash_at_300(subscribed: bool) -> Hash64 {
    let mut host = boxes_host();
    if subscribed {
        subscribe(&mut host);
        set_layers(&mut host, &[LAYER]);
    }
    play(&mut host);
    while host.tick() < 300 {
        advance(&mut host, 1, 0.0);
    }
    assert_eq!(host.tick(), 300);
    match ok(&mut host, Command::GetHash) {
        Reply::Hash { hash, .. } => hash,
        other => panic!("{other:?}"),
    }
}

#[test]
fn pushed_frames_never_change_the_hash() {
    assert_eq!(hash_at_300(true), hash_at_300(false));
}

#[test]
fn worker_frames_decode_to_the_same_frames() {
    let mut host = boxes_host();
    subscribe(&mut host);
    let every_layer: BTreeSet<String> = host.game_info().debug_layers.into_iter().collect();
    let layers: Vec<&str> = every_layer.iter().map(String::as_str).collect();
    set_layers(&mut host, &layers);
    play(&mut host);
    let [(frames, _)] = pushed(&advance(&mut host, 2, 0.0)).try_into().unwrap();
    assert!(!frames[1].debug.layers.is_empty());
    for frame in &frames {
        let bytes = protocol::encode_frame(frame);
        let decoded: RenderFrame = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(&decoded, frame);
    }
}
