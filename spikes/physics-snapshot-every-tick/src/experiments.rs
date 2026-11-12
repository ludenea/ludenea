//! The experiments of the Rapier half, each the counterpart of the Box2D
//! half's: a take after every tick, a correction inside one frame, the
//! continuation after a restore at every frame, the image after a churn of
//! bodies and of a sleeping world, and the correction by write-back.

use std::time::Instant;

use rapier3d::prelude::*;

use crate::scenes::{
    INPUT_SEED, LibraryScene, SCENE_SEED, SceneKind, build_library_scene, build_motion_walls, build_scene,
    ceil_cube_root, churn_tick,
};
use crate::sim::{FIXED_STEP_MS, Pool, Random, Ring, Sim, Take, milliseconds};

pub struct Config {
    pub quick: bool,
    pub ladder: Vec<usize>,
    pub claim_bodies: usize,
    pub warmup_ticks: usize,
    pub forward_ticks: usize,
    pub corrections: usize,
    pub settle_after: usize,
    pub continuation_frames: usize,
    pub churn_ticks: usize,
    pub asleep_ticks_at_most: usize,
    pub asleep_takes: usize,
    pub write_back_warmup: usize,
    pub write_back_later_ticks: usize,
}

impl Config {
    pub fn full() -> Config {
        Config {
            quick: false,
            ladder: vec![100, 300, 1_000, 3_000, 10_000],
            claim_bodies: 1_000,
            warmup_ticks: 60,
            forward_ticks: 600,
            corrections: 1_000,
            settle_after: 100,
            continuation_frames: 1_000,
            churn_ticks: 10_000,
            asleep_ticks_at_most: 3_000,
            asleep_takes: 200,
            write_back_warmup: 120,
            write_back_later_ticks: 600,
        }
    }

    pub fn quick() -> Config {
        Config {
            quick: true,
            ladder: vec![100, 300],
            claim_bodies: 300,
            warmup_ticks: 30,
            forward_ticks: 60,
            corrections: 60,
            settle_after: 30,
            continuation_frames: 40,
            churn_ticks: 300,
            asleep_ticks_at_most: 900,
            asleep_takes: 20,
            write_back_warmup: 60,
            write_back_later_ticks: 60,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Summary {
    pub median: f64,
    pub p99: f64,
    pub max: f64,
}

/// The median, the 99th percentile (the value that at most one sample in a
/// hundred exceeds) and the largest of the samples.
pub fn summarize(mut samples: Vec<f64>) -> Summary {
    if samples.is_empty() {
        return Summary::default();
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite samples"));
    let n = samples.len();
    let rank = (99 * n).div_ceil(100);
    Summary { median: samples[n / 2], p99: samples[rank - 1], max: samples[n - 1] }
}

// --- A take after every tick ---------------------------------------------------

pub struct ForwardResult {
    pub scene: SceneKind,
    pub bodies: usize,
    pub workers: usize,
    pub ticks: usize,
    pub step: Summary,
    pub sync: Summary,
    pub image: Summary,
    pub tables: Summary,
    pub canonical: Summary,
    pub take: Summary,
    pub tick_and_take: Summary,
    pub image_bytes: Summary,
    pub take_bytes: Summary,
    pub tables_bytes: usize,
    pub canonical_bytes: usize,
    pub ring_slots: usize,
    pub ring_bytes: usize,
    pub final_hash: u64,
}

pub fn run_forward(config: &Config, scene: SceneKind, bodies: usize, pool: &Pool) -> ForwardResult {
    let mut sim = build_scene(scene, bodies, pool, false, true);
    let mut ring = Ring::new(10, 1);
    for tick in 1..=config.warmup_ticks {
        sim.tick(tick, false);
        // The last ticks of the warm-up fill the ring, so that no timed take
        // pays for the first use of a slot's memory.
        if tick + ring.slot_count() > config.warmup_ticks {
            sim.take(ring.slot(tick), tick);
        }
    }
    let n = config.forward_ticks;
    let (mut step, mut sync, mut image, mut tables, mut canonical, mut take, mut both) =
        (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut image_bytes, mut take_bytes) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut tables_bytes, mut canonical_bytes) = (0, 0);
    for i in 1..=n {
        let tick = config.warmup_ticks + i;
        sim.apply_inputs(tick, false);
        let start = Instant::now();
        sim.step();
        let step_ms = milliseconds(start);
        let start = Instant::now();
        sim.sync();
        let sync_ms = milliseconds(start);
        let slot = ring.slot(tick);
        let times = sim.take(slot, tick);
        step.push(step_ms);
        sync.push(sync_ms);
        image.push(times.image_ms);
        tables.push(times.tables_ms);
        canonical.push(times.canonical_ms);
        take.push(times.total());
        both.push(step_ms + sync_ms + times.total());
        image_bytes.push(slot.image.len() as f64);
        take_bytes.push(slot.bytes() as f64);
        tables_bytes = slot.tables.len();
        canonical_bytes = slot.canonical.len();
    }
    ForwardResult {
        scene,
        bodies,
        workers: sim.workers,
        ticks: n,
        step: summarize(step),
        sync: summarize(sync),
        image: summarize(image),
        tables: summarize(tables),
        canonical: summarize(canonical),
        take: summarize(take),
        tick_and_take: summarize(both),
        image_bytes: summarize(image_bytes),
        take_bytes: summarize(take_bytes),
        tables_bytes,
        canonical_bytes,
        ring_slots: ring.slot_count(),
        ring_bytes: ring.bytes(),
        final_hash: sim.hash(),
    }
}

// --- A correction inside one frame ----------------------------------------------

pub struct CorrectionForm {
    pub name: &'static str,
    /// A take of the three parts every so many ticks.
    pub cadence: usize,
    /// The ticks a correction goes back.
    pub depth: usize,
    /// The ring holds the confirmed tick and no other.
    pub confirmed_alone: bool,
    pub check_continuation: bool,
}

pub const CORRECTION_FORMS: [CorrectionForm; 6] = [
    CorrectionForm { name: "every_tick", cadence: 1, depth: 2, confirmed_alone: false, check_continuation: false },
    CorrectionForm { name: "every_tick", cadence: 1, depth: 8, confirmed_alone: false, check_continuation: true },
    CorrectionForm { name: "every_tick", cadence: 1, depth: 24, confirmed_alone: false, check_continuation: false },
    CorrectionForm { name: "every_second_tick", cadence: 2, depth: 8, confirmed_alone: false, check_continuation: false },
    CorrectionForm { name: "every_fourth_tick", cadence: 4, depth: 8, confirmed_alone: false, check_continuation: false },
    CorrectionForm { name: "confirmed_tick_alone", cadence: 1, depth: 8, confirmed_alone: true, check_continuation: false },
];

pub struct CorrectionResult {
    pub scene: SceneKind,
    pub bodies: usize,
    pub workers: usize,
    pub form: &'static CorrectionForm,
    pub corrections_run: usize,
    pub settled_before_the_end: bool,
    pub exceeding: usize,
    pub ticks_again_at_most: usize,
    pub frame: Summary,
    pub restore: Summary,
    pub tick_again: Summary,
    /// The first tick applied after the restore, which is among the ticks
    /// applied again and is timed apart as well.
    pub first_tick_again: Summary,
    pub ring_slots: usize,
    pub ring_bytes: usize,
    pub final_hash: u64,
    /// None when not checked; otherwise whether the final hash equals the
    /// uninterrupted world's.
    pub continuation: Option<bool>,
}

pub fn run_corrections(
    config: &Config,
    scene: SceneKind,
    bodies: usize,
    pool: &Pool,
    form: &'static CorrectionForm,
) -> CorrectionResult {
    let (cadence, depth) = (form.cadence, form.depth);
    // Every tick a frame restores from or writes is held at once: the ticks
    // from the one a correction starts from to the frame's new tick.
    let slots = if form.confirmed_alone { 1 } else { (depth + cadence) / cadence + 1 };
    let taken = |tick: usize, single: usize| if form.confirmed_alone { tick == single } else { tick % cadence == 0 };

    let mut sim = build_scene(scene, bodies, pool, false, true);
    let mut ring = Ring::new(slots, cadence);
    let mut now = config.warmup_ticks;
    for tick in 1..=now {
        sim.tick(tick, false);
        if taken(tick, now - depth) {
            sim.take(ring.slot(tick), tick);
        }
    }

    let (mut frame, mut restore, mut again, mut first) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut exceeding, mut ticks_again_at_most, mut settled) = (0, 0, false);
    for n in 0..config.corrections {
        let frame_start = Instant::now();
        let start = if form.confirmed_alone { now - depth } else { ((now - depth) / cadence) * cadence };
        let clock = Instant::now();
        sim.restore(ring.held(start));
        restore.push(milliseconds(clock));
        ticks_again_at_most = ticks_again_at_most.max(now - start);
        for tick in start + 1..=now + 1 {
            let clock = Instant::now();
            sim.tick(tick, false);
            if taken(tick, start + 1) {
                sim.take(ring.slot(tick), tick);
            }
            if tick <= now {
                let tick_ms = milliseconds(clock);
                again.push(tick_ms);
                if tick == start + 1 {
                    first.push(tick_ms);
                }
            }
        }
        now += 1;
        let frame_ms = milliseconds(frame_start);
        frame.push(frame_ms);
        if frame_ms > FIXED_STEP_MS {
            exceeding += 1;
        }
        // Eleven frames over the fixed step among a thousand put the 99th
        // percentile over it whatever the others measure: the verdict is
        // settled and the run stops.
        if n + 1 >= config.settle_after && exceeding >= 11 && n + 1 < config.corrections {
            settled = true;
            break;
        }
    }
    let final_hash = sim.hash();
    let workers = sim.workers;
    let ring_bytes = ring.bytes();
    drop(ring);
    drop(sim);

    let continuation = form.check_continuation.then(|| {
        let mut uninterrupted = build_scene(scene, bodies, pool, false, true);
        for tick in 1..=now {
            uninterrupted.tick(tick, false);
        }
        uninterrupted.hash() == final_hash
    });
    CorrectionResult {
        scene,
        bodies,
        workers,
        form,
        corrections_run: frame.len(),
        settled_before_the_end: settled,
        exceeding,
        ticks_again_at_most,
        frame: summarize(frame),
        restore: summarize(restore),
        tick_again: summarize(again),
        first_tick_again: summarize(first),
        ring_slots: slots,
        ring_bytes,
        final_hash,
        continuation,
    }
}

// --- The continuation after a restore at every frame -------------------------------

pub struct ContinuationResult {
    pub scene: &'static str,
    pub bodies: usize,
    pub workers: usize,
    pub frames: usize,
    pub same_inputs_again: usize,
    pub amended: usize,
    pub mispredicted_ticks: usize,
    pub mispredicted_ticks_that_differed: usize,
    pub ticks_compared: usize,
    /// -1 when every confirmed tick hashes as the uninterrupted world's.
    pub first_differing_tick: i64,
    pub final_hash: u64,
    pub uninterrupted_final_hash: u64,
    pub image_bytes: usize,
    pub wall_seconds: f64,
}

pub fn run_continuation(config: &Config, scene: &LibraryScene, pool: &Pool) -> ContinuationResult {
    let wall = Instant::now();
    let warmup = 8;
    let last_tick = warmup + config.continuation_frames;

    // The world never restored, which receives the amended stream from the start.
    let mut uninterrupted = build_library_scene(scene, pool);
    let bodies = uninterrupted.count();
    let mut reference = vec![0u64; last_tick + 1];
    for tick in 1..=last_tick {
        uninterrupted.tick(tick, false);
        reference[tick] = uninterrupted.hash();
    }
    drop(uninterrupted);

    let mut sim = build_library_scene(scene, pool);
    let mut ring = Ring::new(10, 1);
    let mut random = Random::new(SCENE_SEED ^ 0x636f_6e74_696e_7565);
    let mut result = ContinuationResult {
        scene: scene.name,
        bodies,
        workers: sim.workers,
        frames: config.continuation_frames,
        same_inputs_again: 0,
        amended: 0,
        mispredicted_ticks: 0,
        mispredicted_ticks_that_differed: 0,
        ticks_compared: 0,
        first_differing_tick: -1,
        final_hash: 0,
        uninterrupted_final_hash: reference[last_tick],
        image_bytes: 0,
        wall_seconds: 0.0,
    };
    let confirm = |sim: &Sim, tick: usize, result: &mut ContinuationResult| {
        result.ticks_compared += 1;
        if sim.hash() != reference[tick] && result.first_differing_tick == -1 {
            result.first_differing_tick = tick as i64;
        }
    };
    let mut now = 0;
    for tick in 1..=warmup {
        sim.tick(tick, false);
        sim.take(ring.slot(tick), tick);
        confirm(&sim, tick, &mut result);
        now = tick;
    }
    let mut last_mispredicted = false;
    for frame in 0..=config.continuation_frames {
        // The last pass is a correction of one tick with no new tick after
        // it, so that the last tick is confirmed too.
        let closing = frame == config.continuation_frames;
        if closing && !last_mispredicted {
            break;
        }
        let depth = if closing { 1 } else { 1 + (random.next() % 8) as usize };
        sim.restore(ring.held(now - depth));
        for tick in now - depth + 1..=now {
            sim.tick(tick, false);
            sim.take(ring.slot(tick), tick);
            confirm(&sim, tick, &mut result);
        }
        if closing {
            break;
        }
        if last_mispredicted {
            result.amended += 1;
        } else {
            result.same_inputs_again += 1;
        }
        let mispredicted = random.next() & 1 != 0;
        now += 1;
        sim.tick(now, mispredicted);
        let slot = ring.slot(now);
        sim.take(slot, now);
        result.image_bytes = slot.image.len();
        if mispredicted {
            result.mispredicted_ticks += 1;
            if sim.hash() != reference[now] {
                result.mispredicted_ticks_that_differed += 1;
            }
        } else {
            confirm(&sim, now, &mut result);
        }
        last_mispredicted = mispredicted;
    }
    result.final_hash = sim.hash();
    result.wall_seconds = wall.elapsed().as_secs_f64();
    result
}

// --- The image after a churn of bodies --------------------------------------------

pub struct ChurnResult {
    pub bodies: usize,
    pub ticks: usize,
    pub changes_per_tick: usize,
    pub churned_image_bytes: usize,
    pub fresh_image_bytes: usize,
    pub churned_take_bytes: usize,
    pub fresh_take_bytes: usize,
    pub same_components: bool,
    /// The hash of the transforms and the motion after the churn, so that the
    /// cells can be compared on the scene they churned.
    pub hash: u64,
}

pub fn run_churn(config: &Config, pool: &Pool) -> ChurnResult {
    let side = ceil_cube_root(config.claim_bodies);
    let mut churned = build_scene(SceneKind::Motion, config.claim_bodies, pool, false, true);
    let mut random = Random::new(SCENE_SEED ^ 0x6368_7572_6e30_3031);
    for tick in 1..=config.churn_ticks {
        churn_tick(&mut churned, &mut random, side);
        churned.apply_inputs(tick, false);
        churned.step();
    }
    churned.sync();
    let mut churned_take = Take::default();
    churned.take(&mut churned_take, config.churn_ticks);

    // The fresh world: created from the components of the churned world's
    // last tick, so that it holds the same bodies with no history.
    let mut fresh = Sim::new(Vector::ZERO, pool, false, INPUT_SEED, SceneKind::Motion.impulse_speed());
    build_motion_walls(&mut fresh, side);
    for (entity, components) in churned.entities.iter().zip(&churned.components) {
        fresh.spawn(entity.definition, *components);
    }
    fresh.refresh_pushable();
    fresh.sync();
    let mut fresh_take = Take::default();
    fresh.take(&mut fresh_take, config.churn_ticks);

    ChurnResult {
        bodies: config.claim_bodies,
        ticks: config.churn_ticks,
        changes_per_tick: config.claim_bodies / 100,
        churned_image_bytes: churned_take.image.len(),
        fresh_image_bytes: fresh_take.image.len(),
        churned_take_bytes: churned_take.bytes(),
        fresh_take_bytes: fresh_take.bytes(),
        same_components: churned.hash() == fresh.hash(),
        hash: churned.hash(),
    }
}

// --- A sleeping world ---------------------------------------------------------------

pub struct AsleepResult {
    pub bodies: usize,
    pub ticks_until_asleep: usize,
    pub asleep_bodies: usize,
    pub awake_bodies_after_waking: usize,
    pub asleep_take: Summary,
    pub awake_take: Summary,
    pub asleep_image_bytes: usize,
    pub awake_image_bytes: usize,
    pub asleep_take_bytes: usize,
    pub awake_take_bytes: usize,
}

fn take_repeatedly(sim: &mut Sim, take: &mut Take, takes: usize) -> Summary {
    summarize((0..takes).map(|_| sim.take(take, 0).total()).collect())
}

pub fn run_asleep(config: &Config, pool: &Pool) -> AsleepResult {
    let mut sim = build_scene(SceneKind::Pile, config.claim_bodies, pool, true, false);
    let mut tick = 0;
    while tick < config.asleep_ticks_at_most && (tick < 10 || sim.awake_dynamic_bodies() > 0) {
        tick += 1;
        sim.step();
    }
    sim.sync();
    let asleep_bodies = sim.components.iter().filter(|c| !c.awake).count();
    let mut take = Take::default();
    take_repeatedly(&mut sim, &mut take, 5);
    let asleep_take = take_repeatedly(&mut sim, &mut take, config.asleep_takes);
    let (asleep_image_bytes, asleep_take_bytes) = (take.image.len(), take.bytes());

    // The same world awake: every body is woken, and one step gives it back
    // the contact constraints an awake world carries.
    sim.world.wake_up_all(true);
    sim.step();
    sim.sync();
    let awake_bodies_after_waking = sim.components.iter().filter(|c| c.awake).count();
    take_repeatedly(&mut sim, &mut take, 5);
    let awake_take = take_repeatedly(&mut sim, &mut take, config.asleep_takes);
    AsleepResult {
        bodies: config.claim_bodies,
        ticks_until_asleep: tick,
        asleep_bodies,
        awake_bodies_after_waking,
        asleep_take,
        awake_take,
        asleep_image_bytes,
        awake_image_bytes: take.image.len(),
        asleep_take_bytes,
        awake_take_bytes: take.bytes(),
    }
}

// --- The correction by write-back ----------------------------------------------------

pub struct WriteBackResult {
    pub scene: SceneKind,
    pub bodies: usize,
    pub workers: usize,
    pub identical_before_the_correction: bool,
    /// 1 to 8, or -1 when the eight ticks hash as the uninterrupted world's.
    pub first_differing_tick_of_the_eight: i64,
    pub largest_error_over_body_size: f64,
    pub largest_error_distance: f64,
    /// Counted from the correction, or -1.
    pub first_tick_the_hashes_part: i64,
    /// Counted from the correction, or -1.
    pub tick_they_meet_again: i64,
    pub equal_at_the_end: bool,
    pub highest_uninterrupted: f64,
    pub highest_corrected: f64,
}

fn highest(sim: &Sim) -> f64 {
    sim.components.iter().map(|c| f64::from(c.position[1])).fold(f64::MIN, f64::max)
}

pub fn run_write_back(config: &Config, scene: SceneKind, pool: &Pool) -> WriteBackResult {
    let depth = 8;
    let base = config.write_back_warmup;
    let corrected = base + depth;
    let last = corrected + config.write_back_later_ticks;

    let mut uninterrupted = build_scene(scene, config.claim_bodies, pool, false, true);
    let mut reference = vec![0u64; last + 1];
    let mut places = Vec::new();
    for tick in 1..=last {
        uninterrupted.tick(tick, false);
        reference[tick] = uninterrupted.hash();
        if tick == corrected {
            places = uninterrupted.components.iter().map(|c| c.position).collect();
        }
    }
    let highest_uninterrupted = highest(&uninterrupted);
    drop(uninterrupted);

    let mut sim = build_scene(scene, config.claim_bodies, pool, false, true);
    let mut take = Take::default();
    let mut identical = true;
    for tick in 1..=corrected {
        sim.tick(tick, false);
        identical &= sim.hash() == reference[tick];
        if tick == base {
            sim.take(&mut take, tick);
        }
    }

    // The correction: the components of eight ticks ago are restored and
    // written into the bodies of the world as it stands now, then the eight
    // ticks are applied again with the same inputs.
    sim.write_back(&take);
    let mut result = WriteBackResult {
        scene,
        bodies: config.claim_bodies,
        workers: sim.workers,
        identical_before_the_correction: identical,
        first_differing_tick_of_the_eight: -1,
        largest_error_over_body_size: 0.0,
        largest_error_distance: 0.0,
        first_tick_the_hashes_part: -1,
        tick_they_meet_again: -1,
        equal_at_the_end: false,
        highest_uninterrupted,
        highest_corrected: 0.0,
    };
    let mut parted = false;
    for tick in base + 1..=last {
        sim.tick(tick, false);
        let equal = sim.hash() == reference[tick];
        let since = (tick - base) as i64;
        if !equal && !parted {
            parted = true;
            result.first_tick_the_hashes_part = since;
            if since <= depth as i64 {
                result.first_differing_tick_of_the_eight = since;
            }
        }
        if equal && parted && result.tick_they_meet_again == -1 {
            result.tick_they_meet_again = since;
        }
        if tick == corrected {
            for ((entity, c), place) in sim.entities.iter().zip(&sim.components).zip(&places) {
                let squared: f64 =
                    (0..3).map(|axis| (f64::from(c.position[axis]) - f64::from(place[axis])).powi(2)).sum();
                let distance = squared.sqrt();
                let relative = distance / f64::from(entity.definition.size());
                if relative > result.largest_error_over_body_size {
                    result.largest_error_over_body_size = relative;
                    result.largest_error_distance = distance;
                }
            }
        }
        if tick == last {
            result.equal_at_the_end = equal;
        }
    }
    result.highest_corrected = highest(&sim);
    result
}
