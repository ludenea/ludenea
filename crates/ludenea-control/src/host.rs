use ludenea_app::{
    App, AppError, Edit, EditError, EditLog, InputLog, RawInput, SimError, Simulation, ValueKind,
};
use ludenea_ecs::schema::TypeSchema;
use ludenea_ecs::{Entity, Snapshot};
use serde_json::Value as JsonValue;

use crate::json::{from_wire, to_wire};
use crate::pacer::Pacer;
use crate::path::{Path, PathSegment, set_at_path};
use crate::protocol::{
    Bytes, Command, ComponentValue, ConsoleLevel, ErrorCode, Event, FieldTarget, GameInfo, Hash64,
    NamedSchema, PauseReason, ProtocolError, Reply,
};
use crate::session::{Resimulation, Session, SessionError, Timeline};

/// Most ticks one `advance` resimulates while the recorded future is stale.
pub const RESIMULATION_BUDGET: u32 = 240;

/// Owns a simulation and its editor session, executes protocol commands against them, and
/// runs play and resimulation as time passes.
pub struct Host {
    simulation: Simulation,
    session: Session,
    pacer: Pacer,
    playing: bool,
    /// The mapped live input, or `None` while every player is idle.
    live: Option<Vec<u8>>,
    /// The timeline last reported, to emit `TimelineChanged` only on change.
    timeline: Timeline,
    /// Events produced by commands, returned by the next `advance`.
    events: Vec<Event>,
}

impl Host {
    /// Starts a session on an empty scene: the built world, without entities.
    pub fn new(app: App) -> Result<Host, AppError> {
        let mut simulation = app.build()?;
        let scene = simulation.save_scene();
        let session =
            Session::start(&mut simulation, scene).expect("the built world's scene loads");
        Ok(Host {
            pacer: Pacer::new(simulation.tick_rate()),
            timeline: session.timeline(),
            simulation,
            session,
            playing: false,
            live: None,
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

    pub fn handle(&mut self, command: Command) -> Result<Reply, ProtocolError> {
        let result = self.execute(command);
        self.report_timeline();
        result
    }

    /// Runs the ticks due after `elapsed_seconds` while playing, and a resimulation budget
    /// while the recorded future is stale. Returns the events since the last call.
    pub fn advance(&mut self, elapsed_seconds: f64) -> Vec<Event> {
        if self.session.timeline().resimulating {
            self.resimulate();
        }
        if self.playing {
            for _ in 0..self.pacer.advance(elapsed_seconds) {
                match self
                    .session
                    .step(&mut self.simulation, self.live.as_deref())
                {
                    Ok(tick) => self.events.push(Event::Ticked {
                        tick,
                        hash: Hash64(self.simulation.hash()),
                        playing: self.playing,
                        effective_rate: self.pacer.effective_rate(),
                    }),
                    // Play waits for resimulation to catch up.
                    Err(SessionError::Resimulating { .. }) => break,
                    Err(error) => {
                        self.fail_unattended(error);
                        if self.playing {
                            self.pause(PauseReason::User);
                        }
                        break;
                    }
                }
            }
        }
        self.report_timeline();
        std::mem::take(&mut self.events)
    }

    fn execute(&mut self, command: Command) -> Result<Reply, ProtocolError> {
        let sim = &mut self.simulation;
        match command {
            Command::LoadScene { ron } => {
                let session = Session::start(sim, ron).map_err(|e| self.fail(e))?;
                self.load(session)
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
                let (a, b) = (decode_snapshot(&a)?, decode_snapshot(&b)?);
                let differences = sim
                    .world()
                    .diff_snapshots(&a, &b)
                    .map_err(|e| ProtocolError::new(ErrorCode::InvalidSnapshot, e))?;
                Ok(Reply::Differences {
                    lines: differences.iter().map(ToString::to_string).collect(),
                })
            }
            Command::ListEntities => Ok(Reply::Entities {
                entities: sim.world().entities().collect(),
            }),
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
                self.edit(Edit::Despawn { entity }, &format!("entity {entity}"))?;
                Ok(Reply::Done)
            }
            Command::InsertComponent {
                entity,
                component,
                value,
            } => {
                let context = format!("entity {entity}, component `{component}`");
                let value = encode_value(sim, ValueKind::Component, &component, value)
                    .map_err(|e| in_context(edit_error(&e), &context))?;
                let edit = Edit::Insert {
                    entity,
                    component,
                    value,
                };
                self.edit(edit, &context)?;
                Ok(Reply::Done)
            }
            Command::RemoveComponent { entity, component } => {
                let context = format!("entity {entity}, component `{component}`");
                self.edit(Edit::Remove { entity, component }, &context)?;
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
                    inputs: Bytes(export.inputs),
                    edits: Bytes(export.edits),
                    undo: Bytes(export.undo),
                    hashes: export.hashes.into_iter().map(Hash64).collect(),
                })
            }
            Command::ReplaceScene { ron } => {
                self.session
                    .replace_scene(sim, ron)
                    .map_err(|e| self.fail(e))?;
                Ok(Reply::Done)
            }
            Command::LoadSession {
                scene,
                inputs,
                edits,
                undo,
                expected,
            } => {
                let inputs = InputLog::decode(&inputs.0)
                    .map_err(|e| ProtocolError::new(ErrorCode::InvalidInputs, e))?;
                let edits = decode_edits(&edits)?;
                let expected = expected.into_iter().map(|hash| hash.0).collect();
                let session = Session::resume(sim, scene, inputs, edits, &undo.0, expected)
                    .map_err(|e| self.fail(e))?;
                self.load(session)
            }
        }
    }

    fn get_entity(&self, entity: Entity) -> Result<Reply, ProtocolError> {
        let world = self.simulation.world();
        let json = world.entity_components_json(entity).ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::NoSuchEntity,
                format!("entity {entity} is not alive; list the entities first"),
            )
        })?;
        let mut ron = world
            .entity_components_ron(entity)
            .expect("the entity is alive");
        let components = json
            .into_iter()
            .map(|(name, json)| {
                let value = match json {
                    Ok(value) => {
                        let schema = value_schema(&self.simulation, ValueKind::Component, &name)
                            .expect("a component of the world is registered");
                        ComponentValue::Json {
                            value: to_wire(value, &schema),
                        }
                    }
                    Err(_) => ComponentValue::Ron {
                        text: ron.remove(&name).expect("the same components"),
                    },
                };
                (name, value)
            })
            .collect();
        Ok(Reply::Entity { entity, components })
    }

    fn get_resources(&self) -> Reply {
        let world = self.simulation.world();
        let resources = world
            .resource_schemas()
            .into_iter()
            .map(|(name, schema)| {
                let value = match world.resource_json(name).expect("a registered resource") {
                    Ok(value) => ComponentValue::Json {
                        value: to_wire(value, &schema),
                    },
                    Err(_) => ComponentValue::Ron {
                        text: world.resource_ron(name).expect("a registered resource"),
                    },
                };
                (name.to_string(), value)
            })
            .collect();
        Reply::Resources { resources }
    }

    /// Reads the exact current value of `target`, replaces the value at `path`, and records
    /// the whole new value as an edit.
    fn set_field(
        &mut self,
        target: FieldTarget,
        path: &[PathSegment],
        value: JsonValue,
    ) -> Result<(), ProtocolError> {
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
                        ProtocolError::new(
                            ErrorCode::InvalidEdit,
                            format!("the entity has no `{component}`; insert it first"),
                        )
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
                entity,
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

    /// Applies `edit` through the session, naming `context` in its error.
    fn edit(&mut self, edit: Edit, context: &str) -> Result<Option<Entity>, ProtocolError> {
        self.session
            .edit(&mut self.simulation, edit)
            .map_err(|e| in_context(self.fail(e), context))
    }

    /// Takes over a new session, paused.
    fn load(&mut self, session: Session) -> Result<Reply, ProtocolError> {
        self.session = session;
        if self.playing {
            self.pause(PauseReason::User);
        }
        Ok(Reply::Done)
    }

    fn step(&mut self, ticks: u32, record_hashes: bool) -> Result<Reply, ProtocolError> {
        let mut hashes = Vec::new();
        for _ in 0..ticks {
            let tick = self.simulation.tick();
            self.session.step(&mut self.simulation, None).map_err(|e| {
                let mut error = self.fail(e);
                error.message =
                    format!("tick {tick}: {}; earlier ticks stay applied", error.message);
                error
            })?;
            if record_hashes {
                hashes.push(Hash64(self.simulation.hash()));
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
    fn fail_unattended(&mut self, error: SessionError) {
        let panicked = matches!(error, SessionError::Panicked { .. });
        let protocol = self.fail(error);
        if !panicked {
            self.console(ConsoleLevel::Error, protocol.message);
        }
    }

    fn pause(&mut self, reason: PauseReason) {
        self.playing = false;
        self.events.push(Event::Paused { reason });
    }

    fn console(&mut self, level: ConsoleLevel, text: String) {
        self.events.push(Event::Console { level, text });
    }

    fn report_timeline(&mut self) {
        let timeline = self.session.timeline();
        if timeline != self.timeline {
            self.timeline = timeline.clone();
            self.events.push(Event::TimelineChanged { timeline });
        }
    }
}

fn session_error(error: &SessionError) -> ProtocolError {
    let code = match error {
        SessionError::InvalidTick { .. } => ErrorCode::InvalidTick,
        SessionError::Resimulating { .. } => ErrorCode::Resimulating,
        SessionError::Panicked { .. } => ErrorCode::SystemPanicked,
        SessionError::NothingToUndo => ErrorCode::NothingToUndo,
        SessionError::NothingToRedo => ErrorCode::NothingToRedo,
        SessionError::UndoDecode(_) => ErrorCode::InvalidCommand,
        SessionError::Edit(error) => return edit_error(error),
        SessionError::MissingComponent { .. } => ErrorCode::InvalidEdit,
        SessionError::NotInScene(_) => ErrorCode::NoSuchEntity,
        SessionError::Sim(error) => return sim_error(error),
        SessionError::Scene(_) => ErrorCode::InvalidScene,
        SessionError::Snapshot(_) => ErrorCode::InvalidSnapshot,
    };
    ProtocolError::new(code, error)
}

fn edit_error(error: &EditError) -> ProtocolError {
    let code = match error {
        EditError::NoSuchEntity(_) => ErrorCode::NoSuchEntity,
        EditError::UnknownComponent(_) => ErrorCode::UnknownComponent,
        EditError::UnknownResource(_) => ErrorCode::UnknownResource,
        EditError::ReadOnly(_) | EditError::InvalidValue { .. } => ErrorCode::InvalidEdit,
    };
    ProtocolError::new(code, error)
}

fn in_context(mut error: ProtocolError, context: &str) -> ProtocolError {
    error.message = format!("{context}: {}", error.message);
    error
}

fn value_schema(sim: &Simulation, kind: ValueKind, name: &str) -> Result<TypeSchema, EditError> {
    let world = sim.world();
    match kind {
        ValueKind::Component => world
            .component_codec(name)
            .ok_or_else(|| EditError::UnknownComponent(name.to_string())),
        ValueKind::Resource => world
            .resource_codec(name)
            .ok_or_else(|| EditError::UnknownResource(name.to_string())),
    }
    .map(|codec| (codec.schema)())
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

fn decode_edits(bytes: &Bytes) -> Result<EditLog, ProtocolError> {
    EditLog::decode(&bytes.0).map_err(|e| ProtocolError::new(ErrorCode::InvalidCommand, e))
}
