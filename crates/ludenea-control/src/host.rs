use std::panic::{AssertUnwindSafe, catch_unwind};

use ludenea_app::prefab;
use ludenea_app::replay::Replay;
use ludenea_app::trace::{Trace, compare_traces};
use ludenea_app::{
    App, AppError, AssetTexts, Edit, EditError, EditLog, InputLog, RawInput, SceneError, SimError,
    Simulation, ValueKind,
};

use ludenea_debug_draw::DebugDrawFrame;
use ludenea_ecs::{AssetId, Entity, EntityFilter, FilterError, Snapshot};
use serde_json::Value as JsonValue;

use crate::compare::{COMPARE_BUDGET_PAUSED, COMPARE_BUDGET_PLAYING, CompareRun, caught_draw};
use crate::divergence::DivergenceRun;
use crate::json::{from_wire, to_wire};
use crate::pacer::Pacer;
use crate::path::{Path, PathSegment, set_at_path};
use crate::protocol::{
    Bytes, Command, ComponentValue, ConsoleLevel, EntityRow, ErrorCode, Event, FieldTarget,
    GameInfo, Hash64, NamedSchema, PauseReason, ProtocolError, Reply, SyncPoint, SystemRow, Topic,
};
use crate::session::{
    Document, Resimulation, Session, SessionError, SessionExport, Timeline, panic_message,
};
use crate::subscriptions::{INTROSPECTION_TICKS, Subscriptions, TickRecord};
use crate::values::{asset_json, asset_value, entity_values, resource_values, value_schema};

/// Most ticks one `advance` resimulates while the recorded future is stale.
pub const RESIMULATION_BUDGET: u32 = 240;

/// Owns a simulation and its editor session, executes protocol commands against them, and
/// runs play, resimulation, and the compare simulation as time passes.
pub struct Host {
    build: fn() -> App,
    simulation: Simulation,
    session: Session,
    pacer: Pacer,
    playing: bool,
    /// The mapped live input, or `None` while every player is idle.
    live: Option<Vec<u8>>,
    /// The timeline last reported, to emit `TimelineChanged` only on change.
    timeline: Timeline,
    subscriptions: Subscriptions,
    /// Events produced by commands, returned by the next `advance`.
    events: Vec<Event>,
    /// The second simulation, built the first time the ghost is needed.
    compare: Option<CompareRun>,
    /// Whether `Topic::Ghost` is subscribed: the compare simulation follows only then.
    ghost: bool,
    /// What `GhostAdvanced` last reported: the ghost's tick, or `None` outside the compare
    /// branch's run.
    ghost_reported: Option<Option<u64>>,
    /// The divergence report's simulations, built the first time a report is needed.
    divergence: Option<DivergenceRun>,
    /// Whether `Topic::Divergence` is subscribed.
    divergence_subscribed: bool,
    /// Whether a requested report is not complete yet: it runs until it is.
    divergence_asked: bool,
}

impl Host {
    /// Starts a session on an empty scene: the built world, without entities. `build` makes
    /// the game again for the compare simulation.
    pub fn new(build: fn() -> App) -> Result<Host, AppError> {
        let mut simulation = build().build()?;
        let scene = simulation.save_scene();
        let session = Session::start(&mut simulation, scene, AssetTexts::new(), Document::Scene)
            .expect("the built world's scene loads");
        Ok(Host {
            build,
            compare: None,
            ghost: false,
            ghost_reported: None,
            divergence: None,
            divergence_subscribed: false,
            divergence_asked: false,
            pacer: Pacer::new(simulation.tick_rate()),
            timeline: session.timeline(),
            simulation,
            session,
            playing: false,
            live: None,
            subscriptions: Subscriptions::default(),
            events: Vec::new(),
        })
    }

    pub fn game_info(&self) -> GameInfo {
        let world = self.simulation.world();
        let named = |schemas: Vec<(&str, _)>| {
            schemas
                .into_iter()
                .map(|(name, schema)| NamedSchema {
                    name: name.to_string(),
                    schema,
                })
                .collect()
        };
        GameInfo {
            components: named(world.component_schemas()),
            resources: named(world.resource_schemas()),
            assets: named(world.asset_schemas()),
            events: named(world.event_schemas()),
            fingerprint: Hash64(self.simulation.schema_fingerprint()),
            input: self.simulation.input_schema(),
            tick_rate: self.simulation.tick_rate(),
            debug_layers: self
                .simulation
                .debug_layers()
                .into_iter()
                .map(String::from)
                .collect(),
        }
    }

    /// The tick of the current state.
    pub fn tick(&self) -> u64 {
        self.simulation.tick()
    }

    pub fn handle(&mut self, command: Command) -> Result<Reply, ProtocolError> {
        let before = changes_state(&command).then(|| self.state());
        let result = self.execute(command);
        if let Some(before) = before {
            if let Some(compare) = &mut self.compare {
                compare.invalidate();
            }
            if let Some(divergence) = &mut self.divergence {
                divergence.invalidate();
            }
            self.report_state(before);
            self.report_changes();
        }
        self.report_timeline();
        result
    }

    /// Runs the ticks due after `elapsed_seconds` while playing, and a resimulation budget
    /// while the recorded future is stale. Returns the events since the last call.
    pub fn advance(&mut self, elapsed_seconds: f64) -> Vec<Event> {
        let resimulating = self.session.timeline().resimulating;
        if resimulating {
            // Resimulation changes the state shown only by moving to another tick (back
            // to the tick of a scene change), so the tick alone says whether to announce it.
            let before = self.simulation.tick();
            self.resimulate();
            if self.simulation.tick() != before {
                self.ticked();
            }
        }
        if self.playing {
            self.subscriptions.played(elapsed_seconds);
            let due = self.pacer.advance(elapsed_seconds);
            if due > 0
                && let Some(divergence) = &mut self.divergence
            {
                divergence.played();
            }
            for ran in 0..due {
                match self
                    .session
                    .step(&mut self.simulation, self.live.as_deref())
                {
                    Ok(_) => self.ticked(),
                    Err(error) => {
                        // Ticks that did not run stay owed: play waits for resimulation to
                        // catch up, or stops on a failure.
                        self.pacer.not_run(due - ran);
                        if !matches!(error, SessionError::Resimulating { .. }) {
                            let tick = self.simulation.tick();
                            let message = self.fail_unattended(error);
                            if self.playing {
                                self.pause(PauseReason::Failed { tick, message });
                            }
                        }
                        break;
                    }
                }
            }
        }
        // Play never changes the compare branch's run: it is never the current branch.
        if resimulating && let Some(compare) = &mut self.compare {
            compare.invalidate();
        }
        if resimulating && let Some(divergence) = &mut self.divergence {
            divergence.invalidate();
        }
        self.follow_compare();
        self.follow_divergence();
        self.report_changes();
        self.report_timeline();
        std::mem::take(&mut self.events)
    }

    /// Runs the compare simulation toward the current tick while the ghost is subscribed;
    /// a failure stops the ghost, the main session untouched.
    fn follow_compare(&mut self) {
        if !self.ghost || self.session.compare_branch().is_none() {
            return;
        }
        let budget = if self.playing {
            COMPARE_BUDGET_PLAYING
        } else {
            COMPARE_BUDGET_PAUSED
        };
        let tick = self.simulation.tick();
        let followed = self.build_compare().and_then(|()| {
            let compare = self.compare.as_mut().expect("built");
            compare.follow(&self.session, tick, budget)
        });
        match followed {
            Ok(state) => {
                let reported = (!state.outside).then_some(state.tick);
                if self.ghost_reported != Some(reported) {
                    self.ghost_reported = Some(reported);
                    self.events.push(Event::GhostAdvanced { tick: state.tick });
                }
            }
            Err(message) => self.stop_ghost(&message),
        }
    }

    /// Computes the divergence report a budget further while it is asked for or subscribed;
    /// a failure stops it, the main session untouched.
    fn follow_divergence(&mut self) {
        if !(self.divergence_subscribed || self.divergence_asked)
            || self.session.compare_branch().is_none()
        {
            return;
        }
        let budget = if self.playing {
            COMPARE_BUDGET_PLAYING
        } else {
            COMPARE_BUDGET_PAUSED
        };
        let advanced = self.build_divergence().and_then(|()| {
            let divergence = self.divergence.as_mut().expect("built");
            divergence.advance(&self.session, budget)
        });
        if let Some(divergence) = &mut self.divergence {
            for warning in divergence.take_warnings() {
                self.console(
                    ConsoleLevel::Warning,
                    format!("divergence report: {warning}"),
                );
            }
        }
        match advanced {
            Ok((moved, complete)) => {
                if let Some(up_to) = moved {
                    self.events.push(Event::DivergenceProgress { up_to });
                }
                if complete {
                    self.divergence_asked = false;
                }
            }
            Err(message) => {
                self.divergence = None;
                self.divergence_asked = false;
                let text = format!("the divergence report stopped: {message}");
                self.console(ConsoleLevel::Error, text);
            }
        }
    }

    /// The divergence report as far as it is computed; `advance` computes it until it is
    /// complete.
    fn get_divergence(&mut self, up_to: Option<u64>) -> Result<Reply, ProtocolError> {
        let invalid = |message| ProtocolError::new(ErrorCode::InvalidCommand, message);
        if self.session.compare_branch().is_none() {
            return Err(invalid(
                "there is no compare branch to compare the current branch with".to_string(),
            ));
        }
        self.build_divergence().map_err(invalid)?;
        let divergence = self.divergence.as_mut().expect("built");
        divergence.request(up_to);
        let report = divergence.report(&self.session).map_err(invalid)?;
        self.divergence_asked = !report.complete;
        Ok(Reply::Divergence(report))
    }

    /// Builds the divergence report's simulations on first use.
    fn build_divergence(&mut self) -> Result<(), String> {
        if self.divergence.is_none() {
            let build = || (self.build)().build().map_err(|error| error.to_string());
            self.divergence = Some(DivergenceRun::new(build()?, build()?));
        }
        Ok(())
    }

    /// Builds the compare simulation on first use.
    fn build_compare(&mut self) -> Result<(), String> {
        if self.compare.is_none() {
            let sim = (self.build)().build().map_err(|error| error.to_string())?;
            self.compare = Some(CompareRun::new(sim));
        }
        Ok(())
    }

    /// Drops the compare simulation and the compare branch, and reports why.
    fn stop_ghost(&mut self, message: &str) {
        self.compare = None;
        self.ghost_reported = None;
        self.session
            .set_compare_branch(None)
            .expect("no branch always sets");
        self.console(ConsoleLevel::Error, format!("the ghost stopped: {message}"));
    }

    fn ghost_draw(&mut self, tick: u64, layers: Vec<String>) -> Result<Reply, ProtocolError> {
        let (frame, tick, lagging) = match self.session.compare_branch() {
            None => (DebugDrawFrame::default(), tick, false),
            Some(_) => {
                let layers = layers.into_iter().collect();
                self.build_compare()
                    .map_err(|message| ProtocolError::new(ErrorCode::InvalidCommand, message))?;
                let compare = self.compare.as_mut().expect("built");
                match compare.draw(&self.session, tick, &layers) {
                    Ok(drawn) => drawn,
                    Err(message) => {
                        self.stop_ghost(&message);
                        return Err(ProtocolError::new(ErrorCode::SystemPanicked, message));
                    }
                }
            }
        };
        Ok(Reply::GhostDraw {
            frame,
            tick,
            lagging,
        })
    }

    /// The events queued since the last `advance` or `drain_events`, for a client that
    /// never advances (the command line): they would otherwise pile up.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn execute(&mut self, command: Command) -> Result<Reply, ProtocolError> {
        let sim = &mut self.simulation;
        match command {
            Command::LoadScene {
                scene,
                assets,
                document,
            } => {
                let session = Session::start(sim, scene, assets.into_iter().collect(), document)
                    .map_err(|e| self.fail(e))?;
                self.load(session);
                Ok(self.loaded())
            }
            Command::LoadInputs { log } => {
                let log = InputLog::decode(&log.0)
                    .map_err(|e| ProtocolError::new(ErrorCode::InvalidInputs, e))?;
                self.session
                    .load_inputs(sim, log)
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::LoadEdits { log } => {
                let log = decode_edits(&log)?;
                self.session
                    .load_edits(sim, log)
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::SetThreads { threads } => {
                sim.set_threads(threads as usize);
                Ok(Reply::Done)
            }
            Command::Step {
                ticks,
                record_hashes,
            } => self.step(ticks, record_hashes),
            Command::GetHash => Ok(Reply::Hash {
                tick: sim.tick(),
                hash: Hash64(sim.hash()),
                pending: self.session.is_pending(),
            }),
            Command::GetHashTree => Ok(Reply::HashTree {
                tick: sim.tick(),
                tree: sim.hash_tree(),
            }),
            Command::TakeSnapshot => Ok(Reply::Snapshot {
                tick: sim.tick(),
                snapshot: Bytes(sim.snapshot().encode()),
            }),
            Command::Restore { snapshot } => {
                let snapshot = decode_snapshot(&snapshot)?;
                self.session
                    .restore(sim, &snapshot)
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::DiffSnapshots { a, b } => {
                let a = decode_snapshot(&a)?;
                let b = match b {
                    Some(b) => decode_snapshot(&b)?,
                    None => sim.snapshot(),
                };
                let differences = sim
                    .world()
                    .diff_snapshots(&a, &b)
                    .map_err(|e| ProtocolError::new(ErrorCode::InvalidSnapshot, e))?;
                Ok(Reply::Differences { differences })
            }
            Command::CompareTraces { a, b } => {
                let a = decode_trace(&a)?.hashes;
                let b = match b {
                    Some(b) => decode_trace(&b)?.hashes,
                    None => self
                        .session
                        .hashes()
                        .map(<[u64]>::to_vec)
                        .map_err(|e| self.fail(e))?,
                };
                Ok(Reply::TraceComparison(compare_traces(&a, &b)))
            }
            Command::QueryEntities {
                filter,
                offset,
                limit,
            } => {
                let world = sim.world();
                let filter = EntityFilter::parse(&filter, world).map_err(|e| filter_error(&e))?;
                let entities = world.filter_entities(&filter);
                let rows = entities
                    .iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .map(|&entity| EntityRow {
                        entity,
                        hash: Hash64(world.entity_hash(entity).expect("alive").hash),
                    })
                    .collect();
                Ok(Reply::Entities {
                    total: u32::try_from(entities.len()).expect("fewer than 2^32 entities"),
                    rows,
                })
            }
            Command::GetEntity { entity } => self.get_entity(entity),
            Command::GetResources => Ok(self.get_resources()),
            Command::Spawn { components } => {
                let components = components
                    .into_iter()
                    .map(|(name, value)| {
                        encode_value(sim, ValueKind::Component, &name, value)
                            .map(|bytes| (name.clone(), bytes))
                            .map_err(|e| in_context(edit_error(&e), &format!("component `{name}`")))
                    })
                    .collect::<Result<_, _>>()?;
                let spawned = self.edit(Edit::Spawn { components }, "spawn")?;
                Ok(Reply::Spawned {
                    entity: spawned.expect("a spawn returns its entity"),
                })
            }
            Command::Despawn { entity } => {
                let target = sim.entity_ref(entity);
                self.edit(
                    Edit::Despawn { entity: target },
                    &format!("entity {entity}"),
                )?;
                Ok(Reply::Done)
            }
            Command::InsertComponent {
                entity,
                component,
                value,
            } => {
                let context = format!("entity {entity}, component `{component}`");
                if sim.world().has_component_named(entity, &component) {
                    let error = ProtocolError::new(
                        ErrorCode::InvalidEdit,
                        "the entity already has this component; use SetField to change it",
                    );
                    return Err(in_context(error, &context));
                }
                let value = encode_value(sim, ValueKind::Component, &component, value)
                    .map_err(|e| in_context(edit_error(&e), &context))?;
                let edit = Edit::Insert {
                    entity: sim.entity_ref(entity),
                    component,
                    value,
                };
                self.edit(edit, &context)?;
                Ok(Reply::Done)
            }
            Command::RemoveComponent { entity, component } => {
                let context = format!("entity {entity}, component `{component}`");
                let target = sim.entity_ref(entity);
                let edit = Edit::Remove {
                    entity: target,
                    component,
                };
                self.edit(edit, &context)?;
                Ok(Reply::Done)
            }
            Command::SetField {
                target,
                path,
                value,
            } => {
                let context = match &target {
                    FieldTarget::Component { entity, component } => {
                        format!("entity {entity}, component `{component}`")
                    }
                    FieldTarget::Resource { resource } => format!("resource `{resource}`"),
                };
                let context = format!("{context}, path `{}`", Path(&path));
                self.set_field(target, &path, value)
                    .map_err(|error| in_context(error, &context))?;
                Ok(Reply::Done)
            }
            Command::SaveScene => {
                let ron = self.session.scene().to_string();
                self.session.mark_scene_saved();
                Ok(Reply::Scene { ron })
            }
            Command::ApplyToScene { entity, component } => {
                let context = format!("entity {entity}, component `{component}`");
                self.session
                    .apply_to_scene(sim, entity, &component)
                    .map_err(|e| in_context(self.fail(e), &context))?;
                Ok(Reply::Done)
            }
            Command::Play => {
                self.playing = true;
                Ok(Reply::Done)
            }
            Command::Pause => {
                self.pause(PauseReason::User);
                Ok(Reply::Done)
            }
            Command::Seek { tick } => {
                self.session.seek(sim, tick).map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::SetSpeed { speed } => {
                if !speed.is_finite() {
                    return Err(ProtocolError::new(
                        ErrorCode::InvalidCommand,
                        format!("speed {speed} is not a finite number"),
                    ));
                }
                self.pacer.set_speed(speed);
                Ok(Reply::Done)
            }
            Command::SetLiveInput { players } => {
                let mapped = sim.map_raw_input(&players).map_err(|e| sim_error(&e))?;
                self.live = (!players.iter().all(RawInput::is_idle)).then_some(mapped);
                Ok(Reply::Done)
            }
            Command::Undo => {
                self.session.undo(sim).map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::Redo => {
                self.session.redo(sim).map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::GetTimeline => Ok(Reply::Timeline(self.session.timeline())),
            Command::ExportSession => {
                let export = self.session.export().map_err(|e| self.fail(e))?;
                Ok(Reply::Session {
                    scene: export.scene,
                    saved_scene: export.saved_scene,
                    inputs: Bytes(export.inputs),
                    edits: Bytes(export.edits),
                    undo: Bytes(export.undo),
                    hashes: export.hashes.into_iter().map(Hash64).collect(),
                    assets: export.assets,
                    branches: Bytes(export.branches),
                    document: self.session.document(),
                    keeps_assets: export.keeps_assets,
                })
            }
            Command::ReplaceScene { scene, assets } => {
                self.session
                    .replace_scene(sim, scene, assets.into_iter().collect())
                    .map_err(|e| self.fail(e))?;
                Ok(self.loaded())
            }
            Command::LoadSession {
                scene,
                saved_scene,
                inputs,
                edits,
                undo,
                expected,
                assets,
                branches,
                document,
                keeps_assets,
            } => {
                // Empty logs load the scene alone; the logs are checked here for their
                // error codes.
                if !inputs.0.is_empty() {
                    InputLog::decode(&inputs.0)
                        .map_err(|e| ProtocolError::new(ErrorCode::InvalidInputs, e))?;
                }
                if !edits.0.is_empty() {
                    decode_edits(&edits)?;
                }
                let export = SessionExport {
                    scene,
                    saved_scene,
                    inputs: inputs.0,
                    edits: edits.0,
                    undo: undo.0,
                    hashes: expected.into_iter().map(|hash| hash.0).collect(),
                    branches: branches.0,
                    assets,
                    keeps_assets,
                };
                match Session::resume(sim, export, document) {
                    Ok(session) => self.load(session),
                    Err(error) => {
                        // The scene may have loaded before the error: the session takes its
                        // world back, as for `LoadReplay`.
                        if let Err(reload) = self.session.reload(sim) {
                            self.fail_unattended(reload);
                        }
                        return Err(self.fail(error));
                    }
                }
                Ok(self.loaded())
            }
            Command::GetAsset { id } => {
                let (asset, value) = asset_value(sim, id).ok_or_else(|| missing_asset(id))?;
                Ok(Reply::Asset { asset, value })
            }
            Command::SetAssetField { id, path, value } => {
                let context = format!("asset {id}, path `{}`", Path(&path));
                self.set_asset_field(id, &path, value)
                    .map_err(|error| in_context(error, &context))?;
                Ok(Reply::Done)
            }
            Command::SaveAsset { id } => Ok(Reply::Text {
                text: self
                    .session
                    .save_asset(id)
                    .ok_or_else(|| missing_asset(id))?,
            }),
            Command::ApplyToAsset { id } => {
                self.session
                    .apply_to_asset(sim, id)
                    .map_err(|e| in_context(self.fail(e), &format!("asset {id}")))?;
                Ok(Reply::Done)
            }
            Command::NewAssetValue { asset } => Ok(Reply::Text {
                text: sim
                    .world()
                    .default_asset_ron(&asset)
                    .map_err(|e| ProtocolError::new(ErrorCode::InvalidCommand, e))?,
            }),
            Command::SwitchBranch { branch } => {
                self.session
                    .switch_branch(sim, branch)
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::RenameBranch { branch, name } => {
                self.session
                    .rename_branch(branch, name)
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::DeleteBranch { branch } => {
                let descendants = self
                    .session
                    .delete_branch(sim, branch)
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Deleted {
                    branches: std::iter::once(branch).chain(descendants).collect(),
                })
            }
            Command::CreatePrefab { entities } => Ok(Reply::Text {
                text: prefab::create_prefab(sim, &entities).map_err(scene_error)?,
            }),
            Command::PlaceInstance { prefab } => {
                let instance = self
                    .session
                    .place_instance(sim, prefab)
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Placed { instance })
            }
            Command::ReplaceWithInstance {
                entities,
                prefab,
                prefab_text,
            } => {
                let instance = self
                    .session
                    .replace_with_instance(sim, &entities, prefab, prefab_text)
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Placed { instance })
            }
            Command::ApplyOverride {
                instance,
                entity,
                component,
                path,
                value,
            } => {
                let value = match value {
                    ComponentValue::Ron { text } => text,
                    ComponentValue::Json { value } => {
                        let address = [&instance[..], &entity[..]].concat();
                        self.override_ron(&address, &component, &path, Some(value))?
                    }
                };
                self.session
                    .set_override(
                        &mut self.simulation,
                        &instance,
                        &entity,
                        &component,
                        &path,
                        &value,
                    )
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::RevertOverride {
                instance,
                entity,
                component,
                path,
            } => {
                self.session
                    .revert_override(&mut self.simulation, &instance, &entity, &component, &path)
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::ApplyToPrefab {
                instance,
                entity,
                component,
                path,
            } => {
                let address = [&instance[..], &entity[..]].concat();
                let value = self.override_ron(&address, &component, &path, None)?;
                let id = self
                    .simulation
                    .instances()
                    .into_iter()
                    .find(|info| info.address == instance)
                    .map(|info| info.prefab)
                    .ok_or_else(|| {
                        ProtocolError::new(
                            ErrorCode::NoSuchEntity,
                            format!("there is no instance {instance:?}"),
                        )
                    })?;
                let assets = self.session.assets();
                let file = assets.get(&id).ok_or_else(|| missing_asset(id))?;
                let text = prefab::apply_to_prefab(file, &entity, &component, &path, &value)
                    .map_err(scene_error)?;
                prefab::save_prefab_checked(id, &text, assets).map_err(scene_error)?;
                Ok(Reply::Text { text })
            }
            Command::GetInstances => Ok(Reply::Instances {
                instances: sim.instances(),
                load_addresses: load_addresses(sim),
            }),
            Command::ExportReplay { branch } => {
                let (recorded, reload) = self.session.export_replay(sim, branch);
                if let Some(reload) = reload {
                    // A failed recording keeps the reply and the reload goes to the console,
                    // as for `LoadReplay`; otherwise the reload's error is the reply.
                    if recorded.is_ok() {
                        return Err(self.fail(reload));
                    }
                    self.fail_unattended(reload);
                }
                let replay = recorded.map_err(|e| self.fail(e))?;
                Ok(Reply::Replay {
                    replay: Bytes(replay.encode()),
                })
            }
            Command::LoadReplay { replay } => {
                let replay = Replay::decode(&replay.0).map_err(|e| session_error(&e.into()))?;
                let fingerprint_differs = replay.fingerprint != sim.schema_fingerprint();
                match Session::from_replay(sim, &replay) {
                    Ok(session) => self.load(session),
                    Err(error) => {
                        // The replay loaded in part: the session takes its world back. The
                        // reply names the replay's error; a failed reload goes to the console.
                        if let Err(reload) = self.session.reload(sim) {
                            self.fail_unattended(reload);
                        }
                        return Err(self.fail(error));
                    }
                }
                Ok(Reply::ReplayLoaded {
                    fingerprint_differs,
                })
            }
            Command::GetSchedule => {
                let systems = sim.systems();
                let micros = match &self.session.introspection {
                    Some(introspection) => introspection.mean_micros(systems.len()),
                    None => vec![None; systems.len()],
                };
                let accesses = |accesses: Vec<(&str, &str)>| {
                    accesses
                        .into_iter()
                        .map(|(kind, name)| format!("{kind} {name}"))
                        .collect()
                };
                let systems = systems
                    .into_iter()
                    .zip(micros)
                    .map(|(system, micros)| SystemRow {
                        name: system.name.to_string(),
                        reads: accesses(system.reads),
                        writes: accesses(system.writes),
                        micros,
                    })
                    .collect();
                Ok(Reply::Schedule { systems })
            }
            Command::GetTickEvents { tick } => Ok(Reply::TickEvents {
                events: self.tick_record(tick)?.events.clone(),
            }),
            Command::GetTickCommands { tick } => {
                let systems = sim.systems();
                let sync_points = self
                    .tick_record(tick)?
                    .commands
                    .iter()
                    .map(|(index, commands)| SyncPoint {
                        after_system: systems[*index].name.to_string(),
                        commands: commands.clone(),
                    })
                    .collect();
                Ok(Reply::TickCommands { sync_points })
            }
            Command::GetDebugDraw { layers } => Ok(Reply::DebugDraw {
                tick: sim.tick(),
                frame: caught_draw(sim, &layers.into_iter().collect())
                    .map_err(|message| ProtocolError::new(ErrorCode::SystemPanicked, message))?,
            }),
            Command::Pick { point } => Ok(Reply::Picked {
                entities: catch_unwind(AssertUnwindSafe(|| sim.pick(point))).map_err(
                    |payload| {
                        ProtocolError::new(
                            ErrorCode::SystemPanicked,
                            format!(
                                "the game panicked while picking: {}",
                                panic_message(&*payload)
                            ),
                        )
                    },
                )?,
            }),
            Command::SetCompareBranch { branch } => {
                self.session
                    .set_compare_branch(branch)
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::GetGhostDraw { tick, layers } => self.ghost_draw(tick, layers),
            Command::GetDivergence { up_to } => self.get_divergence(up_to),
            Command::Subscribe { topic } => {
                if let Topic::Resource { name } = &topic
                    && !sim
                        .world()
                        .resource_schemas()
                        .iter()
                        .any(|(known, _)| known == name)
                {
                    return Err(edit_error(&EditError::UnknownResource(name.clone())));
                }
                if topic == Topic::Ghost {
                    self.ghost = true;
                    self.ghost_reported = None;
                }
                if topic == Topic::Divergence {
                    self.divergence_subscribed = true;
                }
                if topic == Topic::Introspection {
                    self.session
                        .introspection
                        .get_or_insert_with(Default::default);
                }
                self.subscriptions
                    .subscribe(&topic, &self.simulation)
                    .map_err(|e| filter_error(&e))?;
                Ok(Reply::Done)
            }
            Command::Unsubscribe { topic } => {
                if topic == Topic::Ghost {
                    self.ghost = false;
                }
                if topic == Topic::Divergence {
                    // A report asked for while subscribed is dropped with the subscription:
                    // nothing follows it any more.
                    self.divergence_subscribed = false;
                    self.divergence_asked = false;
                    self.divergence = None;
                }
                if topic == Topic::Introspection {
                    self.session.introspection = None;
                }
                self.subscriptions.unsubscribe(&topic);
                Ok(Reply::Done)
            }
        }
    }

    fn get_entity(&self, entity: Entity) -> Result<Reply, ProtocolError> {
        let components = entity_values(&self.simulation, entity).ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::NoSuchEntity,
                format!("entity {entity} is not alive; query the entities first"),
            )
        })?;
        Ok(Reply::Entity { entity, components })
    }

    fn get_resources(&self) -> Reply {
        Reply::Resources {
            resources: resource_values(&self.simulation),
        }
    }

    /// The introspection record of the tick that ended at `tick`.
    fn tick_record(&self, tick: u64) -> Result<&TickRecord, ProtocolError> {
        self.session
            .introspection
            .as_ref()
            .and_then(|introspection| introspection.tick(tick, self.session.recorded_hash(tick)))
            .ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::InvalidTick,
                    format!(
                        "tick {tick} is not among the last {INTROSPECTION_TICKS} ticks run \
                         while introspection is subscribed; subscribe and run it again"
                    ),
                )
            })
    }

    /// The RON at `path` of `component` on the entity at load address `address`, as an
    /// override value of that entity: its current value, with `value` at `path` when given.
    fn override_ron(
        &self,
        address: &[u32],
        component: &str,
        path: &[PathSegment],
        value: Option<JsonValue>,
    ) -> Result<String, ProtocolError> {
        let sim = &self.simulation;
        let entity = sim
            .entity_at_address(address)
            .ok_or_else(|| edit_error(&EditError::NoSuchAddress(address.to_vec())))?;
        let missing = || {
            if sim.world().component_codec(component).is_none() {
                edit_error(&EditError::UnknownComponent(component.to_string()))
            } else {
                session_error(&SessionError::MissingComponent {
                    entity,
                    component: component.to_string(),
                })
            }
        };
        let bytes = match value {
            None => sim
                .world()
                .component_bytes(entity, component)
                .ok_or_else(missing)?,
            Some(value) => {
                let current = sim
                    .world()
                    .component_json(entity, component)
                    .ok_or_else(missing)?
                    .map_err(|reason| {
                        ProtocolError::new(
                            ErrorCode::InvalidEdit,
                            format!("`{component}` is read-only in the editor: {reason}"),
                        )
                    })?;
                let schema = value_schema(sim, ValueKind::Component, component)
                    .map_err(|e| edit_error(&e))?;
                let mut whole = to_wire(current, &schema);
                set_at_path(&mut whole, path, value)
                    .map_err(|reason| ProtocolError::new(ErrorCode::InvalidPath, reason))?;
                encode_value(sim, ValueKind::Component, component, whole)
                    .map_err(|e| edit_error(&e))?
            }
        };
        let whole = sim
            .override_value_ron(address, component, &bytes)
            .map_err(scene_error)?;
        prefab::ron_at_path(&whole, path).ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::InvalidPath,
                format!("`{component}` has no `{}`", Path(path)),
            )
        })
    }

    /// Reads the exact current value of `target`, replaces the value at `path`, and records
    /// the whole new value as an edit.
    fn set_field(
        &mut self,
        target: FieldTarget,
        path: &[PathSegment],
        value: JsonValue,
    ) -> Result<(), ProtocolError> {
        // At tick 0 a field of a prefab instance's entity is an override of that one field.
        if let FieldTarget::Component { entity, component } = &target
            && self.session.timeline().current == 0
            && let Some(address) = self.simulation.load_addresses().get(entity)
            && address.len() > 1
        {
            let address = address.clone();
            let ron = self.override_ron(&address, component, path, Some(value))?;
            return self
                .session
                .set_override(
                    &mut self.simulation,
                    &address[..1],
                    &address[1..],
                    component,
                    path,
                    &ron,
                )
                .map_err(|e| self.fail(e));
        }
        let world = self.simulation.world();
        let (kind, name, current) = match &target {
            FieldTarget::Component { entity, component } => {
                if !world.is_alive(*entity) {
                    return Err(edit_error(&EditError::NoSuchEntity(*entity)));
                }
                let current = world.component_json(*entity, component).ok_or_else(|| {
                    if world.component_codec(component).is_none() {
                        edit_error(&EditError::UnknownComponent(component.clone()))
                    } else {
                        session_error(&SessionError::MissingComponent {
                            entity: *entity,
                            component: component.clone(),
                        })
                    }
                })?;
                (ValueKind::Component, component, current)
            }
            FieldTarget::Resource { resource } => {
                let current = world
                    .resource_json(resource)
                    .ok_or_else(|| edit_error(&EditError::UnknownResource(resource.clone())))?;
                (ValueKind::Resource, resource, current)
            }
        };
        let current = current.map_err(|reason| {
            ProtocolError::new(
                ErrorCode::InvalidEdit,
                format!("`{name}` is read-only in the editor: {reason}"),
            )
        })?;
        let schema = value_schema(&self.simulation, kind, name).map_err(|e| edit_error(&e))?;
        let mut whole = to_wire(current, &schema);
        set_at_path(&mut whole, path, value)
            .map_err(|reason| ProtocolError::new(ErrorCode::InvalidPath, reason))?;
        let value =
            encode_value(&self.simulation, kind, name, whole).map_err(|e| edit_error(&e))?;
        let edit = match target {
            FieldTarget::Component { entity, component } => Edit::Insert {
                entity: self.simulation.entity_ref(entity),
                component,
                value,
            },
            FieldTarget::Resource { resource } => Edit::SetResource { resource, value },
        };
        self.session
            .edit(&mut self.simulation, edit)
            .map_err(|e| self.fail(e))?;
        Ok(())
    }

    /// Reads the value of the data asset `id`, replaces the value at `path`, and records the
    /// whole new value as an asset edit.
    fn set_asset_field(
        &mut self,
        id: AssetId,
        path: &[PathSegment],
        value: JsonValue,
    ) -> Result<(), ProtocolError> {
        let (asset, schema, current) = asset_json(&self.simulation, id)
            .ok_or_else(|| edit_error(&EditError::MissingAsset(id)))?;
        let current = current.map_err(|reason| {
            ProtocolError::new(
                ErrorCode::InvalidEdit,
                format!("`{asset}` is read-only in the editor: {reason}"),
            )
        })?;
        let mut whole = to_wire(current, &schema);
        set_at_path(&mut whole, path, value)
            .map_err(|reason| ProtocolError::new(ErrorCode::InvalidPath, reason))?;
        let invalid = |reason| {
            edit_error(&EditError::InvalidValue {
                name: asset.clone(),
                reason,
            })
        };
        let codec = self
            .simulation
            .world()
            .asset_codec(&asset)
            .expect("a loaded asset's type");
        let decoded =
            (codec.from_json)(from_wire(whole, &schema).map_err(invalid)?).map_err(invalid)?;
        let edit = Edit::SetAsset {
            id,
            value: (codec.encode)(&*decoded),
        };
        self.session
            .edit(&mut self.simulation, edit)
            .map_err(|e| self.fail(e))?;
        Ok(())
    }

    /// Applies `edit` through the session, naming `context` in its error.
    fn edit(&mut self, edit: Edit, context: &str) -> Result<Option<Entity>, ProtocolError> {
        self.session
            .edit(&mut self.simulation, edit)
            .map_err(|e| in_context(self.fail(e), context))
    }

    /// Takes over a new session, paused.
    fn load(&mut self, mut session: Session) {
        // Records of the old session never match the new one's hashes, unless it is the same.
        session.introspection = self.session.introspection.take();
        self.session = session;
        if self.playing {
            self.pause(PauseReason::User);
        }
    }

    /// What the last scene load of the session reports: `LoadScene`, `ReplaceScene`, and
    /// `LoadSession` answer it.
    fn loaded(&self) -> Reply {
        Reply::Loaded {
            warnings: self.session.warnings().to_vec(),
            instances: self.simulation.instances(),
            load_addresses: load_addresses(&self.simulation),
        }
    }

    fn step(&mut self, ticks: u32, record_hashes: bool) -> Result<Reply, ProtocolError> {
        let mut hashes = Vec::new();
        for _ in 0..ticks {
            let tick = self.simulation.tick();
            self.session.step(&mut self.simulation, None).map_err(|e| {
                // A panic message names its tick already.
                let panicked = matches!(e, SessionError::Panicked { .. });
                let mut error = self.fail(e);
                error.message = if panicked {
                    format!("{}; earlier ticks stay applied", error.message)
                } else {
                    format!("tick {tick}: {}; earlier ticks stay applied", error.message)
                };
                error
            })?;
            if record_hashes {
                // The hash after the tick, before the edits recorded at the new tick.
                let recorded = self.session.recorded_hash(self.simulation.tick());
                hashes.push(Hash64(recorded.unwrap_or_else(|| self.simulation.hash())));
            }
        }
        Ok(Reply::Stepped {
            tick: self.simulation.tick(),
            hashes,
        })
    }

    /// Replays up to `RESIMULATION_BUDGET` stale ticks and reports what it found.
    fn resimulate(&mut self) {
        let mut budget = RESIMULATION_BUDGET;
        loop {
            let report = self.session.resimulate(&mut self.simulation, budget);
            // Reports queued by that call come back without simulating more.
            budget = 0;
            match report {
                Ok(Resimulation::Idle) => return,
                Ok(Resimulation::Progress { up_to }) => {
                    self.events.push(Event::Resimulated { up_to });
                    return;
                }
                Ok(Resimulation::Done) => {
                    let up_to = self.session.timeline().simulated;
                    self.events.push(Event::Resimulated { up_to });
                    return;
                }
                Ok(Resimulation::Diverged {
                    tick,
                    expected,
                    got,
                }) => {
                    let text = format!(
                        "tick {tick} diverged from the recorded session: expected hash \
                         {expected:016x}, got {got:016x}; the game code changes the simulation"
                    );
                    self.console(ConsoleLevel::Warning, text);
                    self.events.push(Event::Diverged {
                        tick,
                        expected: Hash64(expected),
                        got: Hash64(got),
                    });
                }
                Ok(Resimulation::EditDropped { tick, reason }) => {
                    let text = format!("the edit recorded at tick {tick} was dropped: {reason}");
                    self.console(ConsoleLevel::Warning, text);
                    self.events.push(Event::EditDropped { tick, reason });
                }
                Err(error) => {
                    self.fail_unattended(error);
                    return;
                }
            }
        }
    }

    /// Turns a session error into a protocol error. A panic is written to the console and
    /// pauses with `Panicked`.
    fn fail(&mut self, error: SessionError) -> ProtocolError {
        let protocol = session_error(&error);
        if let SessionError::Panicked { tick, message } = error {
            self.console(ConsoleLevel::Error, protocol.message.clone());
            self.pause(PauseReason::Panicked { tick, message });
        }
        protocol
    }

    /// A failure with no command to reply to: every kind is written to the console.
    /// Returns its message.
    fn fail_unattended(&mut self, error: SessionError) -> String {
        let panicked = matches!(error, SessionError::Panicked { .. });
        let protocol = self.fail(error);
        if !panicked {
            self.console(ConsoleLevel::Error, protocol.message.clone());
        }
        protocol.message
    }

    fn pause(&mut self, reason: PauseReason) {
        self.playing = false;
        self.pacer.stop();
        self.events.push(Event::Paused { reason });
    }

    fn console(&mut self, level: ConsoleLevel, text: String) {
        self.events.push(Event::Console { level, text });
    }

    /// Reports the timeline when it changes beyond its current tick, which `Ticked`
    /// reports.
    fn report_timeline(&mut self) {
        let timeline = self.session.timeline();
        let moved_only = Timeline {
            current: timeline.current,
            ..self.timeline.clone()
        };
        if timeline != moved_only {
            self.timeline = timeline.clone();
            self.events.push(Event::TimelineChanged { timeline });
        }
    }

    /// The current tick and the world's hash, edits at that tick included.
    fn state(&self) -> (u64, u64) {
        (self.simulation.tick(), self.simulation.hash())
    }

    /// Announces the current state with `Ticked` when it differs from `before`, so clients
    /// refresh after a change that play did not report.
    fn report_state(&mut self, before: (u64, u64)) {
        if self.state() != before {
            self.ticked();
        }
    }

    /// Announces the current state with `Ticked`.
    fn ticked(&mut self) {
        self.events.push(Event::Ticked {
            tick: self.simulation.tick(),
            hash: Hash64(self.simulation.hash()),
            pending: self.session.is_pending(),
            playing: self.playing,
            effective_rate: self.pacer.effective_rate(),
        });
    }

    fn report_changes(&mut self) {
        let changes = self.subscriptions.deltas(&self.simulation, self.playing);
        self.events.extend(changes);
    }
}

/// Whether `command` may change the simulated state, so the state and the subscriptions are
/// checked after it.
fn changes_state(command: &Command) -> bool {
    !matches!(
        command,
        Command::SetThreads { .. }
            | Command::GetHash
            | Command::GetHashTree
            | Command::TakeSnapshot
            | Command::DiffSnapshots { .. }
            | Command::CompareTraces { .. }
            | Command::CreatePrefab { .. }
            | Command::ApplyToPrefab { .. }
            | Command::GetInstances
            | Command::QueryEntities { .. }
            | Command::GetEntity { .. }
            | Command::GetResources
            | Command::SaveScene
            | Command::Play
            | Command::Pause
            | Command::SetSpeed { .. }
            | Command::SetLiveInput { .. }
            | Command::GetTimeline
            | Command::ExportSession
            | Command::GetAsset { .. }
            | Command::SaveAsset { .. }
            | Command::NewAssetValue { .. }
            | Command::RenameBranch { .. }
            | Command::GetSchedule
            | Command::GetTickEvents { .. }
            | Command::GetTickCommands { .. }
            | Command::GetDebugDraw { .. }
            | Command::Pick { .. }
            | Command::SetCompareBranch { .. }
            | Command::GetGhostDraw { .. }
            | Command::GetDivergence { .. }
            | Command::Subscribe { .. }
            | Command::Unsubscribe { .. }
    )
}

fn filter_error(error: &FilterError) -> ProtocolError {
    ProtocolError::new(ErrorCode::InvalidFilter, error)
}

fn session_error(error: &SessionError) -> ProtocolError {
    let code = match error {
        SessionError::InvalidTick { .. } => ErrorCode::InvalidTick,
        SessionError::Resimulating { .. } => ErrorCode::Resimulating,
        SessionError::Panicked { .. } => ErrorCode::SystemPanicked,
        SessionError::NothingToUndo => ErrorCode::NothingToUndo,
        SessionError::NothingToRedo => ErrorCode::NothingToRedo,
        SessionError::Undo(_)
        | SessionError::BranchesDecode(_)
        | SessionError::NoSuchBranch(_)
        | SessionError::RootBranch
        | SessionError::NotOnBranch { .. } => ErrorCode::InvalidCommand,
        SessionError::Edit(error) => return edit_error(error),
        SessionError::MissingComponent { .. }
        | SessionError::PrefabHasNoResources
        | SessionError::NotDataAsset(_) => ErrorCode::InvalidEdit,
        SessionError::NotInScene(_) => ErrorCode::NoSuchEntity,
        SessionError::Sim(error) => return sim_error(error),
        SessionError::Scene(_) => ErrorCode::InvalidScene,
        SessionError::Snapshot(_) => ErrorCode::InvalidSnapshot,
        SessionError::Replay(_) => ErrorCode::InvalidReplay,
    };
    ProtocolError::new(code, error)
}

fn edit_error(error: &EditError) -> ProtocolError {
    let code = match error {
        EditError::NoSuchEntity(_) | EditError::NoSuchAddress(_) => ErrorCode::NoSuchEntity,
        EditError::UnknownComponent(_) => ErrorCode::UnknownComponent,
        EditError::UnknownResource(_) => ErrorCode::UnknownResource,
        EditError::ReadOnly(_) | EditError::InvalidValue { .. } | EditError::MissingAsset(_) => {
            ErrorCode::InvalidEdit
        }
    };
    ProtocolError::new(code, error)
}

fn scene_error(error: SceneError) -> ProtocolError {
    session_error(&error.into())
}

/// Where each entity of the last scene load came from.
fn load_addresses(sim: &Simulation) -> Vec<(Entity, Vec<u32>)> {
    sim.load_addresses()
        .iter()
        .map(|(&entity, address)| (entity, address.clone()))
        .collect()
}

/// An asset a query names that the session does not have: no edit is involved.
fn missing_asset(id: AssetId) -> ProtocolError {
    ProtocolError::new(ErrorCode::InvalidCommand, EditError::MissingAsset(id))
}

fn in_context(mut error: ProtocolError, context: &str) -> ProtocolError {
    error.message = format!("{context}: {}", error.message);
    error
}

/// Converts a wire value into the canonical bytes an edit carries.
fn encode_value(
    sim: &Simulation,
    kind: ValueKind,
    name: &str,
    value: JsonValue,
) -> Result<Vec<u8>, EditError> {
    let value = from_wire(value, &value_schema(sim, kind, name)?).map_err(|reason| {
        EditError::InvalidValue {
            name: name.to_string(),
            reason,
        }
    })?;
    sim.encode_json_value(kind, name, value)
}

fn sim_error(error: &SimError) -> ProtocolError {
    let code = match error {
        SimError::NoInputMapping => ErrorCode::NoInputMapping,
        _ => ErrorCode::InvalidInputs,
    };
    ProtocolError::new(code, error)
}

fn decode_snapshot(bytes: &Bytes) -> Result<Snapshot, ProtocolError> {
    Snapshot::decode(&bytes.0).map_err(|e| ProtocolError::new(ErrorCode::InvalidSnapshot, e))
}

fn decode_trace(bytes: &Bytes) -> Result<Trace, ProtocolError> {
    Trace::decode(&bytes.0).map_err(|e| ProtocolError::new(ErrorCode::InvalidCommand, e))
}

fn decode_edits(bytes: &Bytes) -> Result<EditLog, ProtocolError> {
    EditLog::decode(&bytes.0).map_err(|e| ProtocolError::new(ErrorCode::InvalidCommand, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_for_an_unknown_asset_is_an_invalid_command() {
        let mut host = Host::new(boxes::app).unwrap();
        let id = AssetId(7);
        // `ApplyToPrefab` names an instance, whose prefab the session always holds.
        for command in [Command::GetAsset { id }, Command::SaveAsset { id }] {
            let error = host.handle(command).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidCommand, "{error:?}");
        }
    }

    #[test]
    fn unsubscribing_drops_a_report_asked_for() {
        let mut host = Host::new(boxes::app).unwrap();
        host.handle(Command::LoadScene {
            scene: boxes::generate_scene(10, 3),
            assets: Vec::new(),
            document: Document::Scene,
        })
        .unwrap();
        let step = Command::Step {
            ticks: 40,
            record_hashes: false,
        };
        host.handle(step.clone()).unwrap();
        host.handle(Command::Seek { tick: 20 }).unwrap();
        host.handle(Command::Spawn {
            components: [("boxes::Crate".to_string(), JsonValue::Null)].into(),
        })
        .unwrap();
        host.handle(step).unwrap();
        let topic = Topic::Divergence;
        host.handle(Command::Subscribe {
            topic: topic.clone(),
        })
        .unwrap();
        host.handle(Command::GetDivergence { up_to: None }).unwrap();
        host.handle(Command::Unsubscribe { topic }).unwrap();
        assert!(host.divergence.is_none());
        let events = host.advance(0.0);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::DivergenceProgress { .. })),
            "{events:?}"
        );
    }

    #[test]
    fn playing_while_the_compare_branch_caps_the_report_composes_nothing() {
        let mut host = Host::new(boxes::app).unwrap();
        let scene = boxes::generate_scene(10, 3);
        let document = Document::Scene;
        host.handle(Command::LoadScene {
            scene,
            assets: Vec::new(),
            document,
        })
        .unwrap();
        let step = |host: &mut Host, ticks| {
            host.handle(Command::Step {
                ticks,
                record_hashes: false,
            })
            .unwrap();
        };
        step(&mut host, 40);
        host.handle(Command::Seek { tick: 20 }).unwrap();
        host.handle(Command::Spawn {
            components: [("boxes::Crate".to_string(), JsonValue::Null)].into(),
        })
        .unwrap();
        step(&mut host, 30);
        host.handle(Command::Subscribe {
            topic: Topic::Divergence,
        })
        .unwrap();
        // The default range ends past the root's end at tick 40, which caps it.
        let report = |host: &mut Host| match host.handle(Command::GetDivergence { up_to: None }) {
            Ok(Reply::Divergence(report)) => report,
            other => panic!("{other:?}"),
        };
        report(&mut host);
        host.advance(0.0);
        let complete = report(&mut host);
        assert!(complete.complete);
        assert_eq!(complete.up_to, 40);
        let composed = host.divergence.as_ref().unwrap().composed;

        host.handle(Command::Play).unwrap();
        for _ in 0..10 {
            host.advance(1.0 / 60.0);
        }
        assert!(
            host.tick() > 50,
            "play recorded the current branch's future"
        );
        assert_eq!(host.divergence.as_ref().unwrap().composed, composed);
    }

    #[test]
    fn playing_with_a_complete_report_composes_nothing() {
        let mut host = Host::new(boxes::app).unwrap();
        let scene = boxes::generate_scene(10, 3);
        let document = Document::Scene;
        host.handle(Command::LoadScene {
            scene,
            assets: Vec::new(),
            document,
        })
        .unwrap();
        let step = |host: &mut Host, ticks| {
            host.handle(Command::Step {
                ticks,
                record_hashes: false,
            })
            .unwrap();
        };
        step(&mut host, 40);
        host.handle(Command::Seek { tick: 20 }).unwrap();
        host.handle(Command::Spawn {
            components: [("boxes::Crate".to_string(), JsonValue::Null)].into(),
        })
        .unwrap();
        step(&mut host, 20);
        let report = |host: &mut Host, up_to| match host.handle(Command::GetDivergence { up_to }) {
            Ok(Reply::Divergence(report)) => report,
            other => panic!("{other:?}"),
        };
        report(&mut host, Some(30));
        host.advance(0.0);
        assert!(report(&mut host, Some(30)).complete);
        host.handle(Command::Subscribe {
            topic: Topic::Divergence,
        })
        .unwrap();
        let composed = host.divergence.as_ref().unwrap().composed;

        host.handle(Command::Play).unwrap();
        for _ in 0..10 {
            host.advance(1.0 / 60.0);
        }
        assert!(
            host.tick() > 40,
            "play recorded the current branch's future"
        );
        assert_eq!(host.divergence.as_ref().unwrap().composed, composed);
    }
}
