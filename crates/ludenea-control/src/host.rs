use ludenea_app::{App, AppError, InputLog, Simulation};
use ludenea_ecs::Snapshot;

use crate::protocol::{Bytes, Command, ErrorCode, Hash64, ProtocolError, Reply};

/// Owns a simulation and executes protocol commands against it.
pub struct Host {
    simulation: Simulation,
    inputs: Option<InputLog>,
}

impl Host {
    pub fn new(app: App) -> Result<Host, AppError> {
        Ok(Host {
            simulation: app.build()?,
            inputs: None,
        })
    }

    pub fn handle(&mut self, command: Command) -> Result<Reply, ProtocolError> {
        match command {
            Command::LoadScene { ron } => {
                self.simulation
                    .load_scene(&ron)
                    .map_err(|e| ProtocolError::new(ErrorCode::InvalidScene, e))?;
                Ok(Reply::Done)
            }
            Command::LoadInputs { log } => {
                let log = InputLog::decode(&log.0)
                    .map_err(|e| ProtocolError::new(ErrorCode::InvalidInputs, e))?;
                self.simulation.set_players(usize::from(log.players));
                self.inputs = Some(log);
                Ok(Reply::Done)
            }
            Command::SetThreads { threads } => {
                self.simulation.set_threads(threads as usize);
                Ok(Reply::Done)
            }
            Command::Step {
                ticks,
                record_hashes,
            } => self.step(ticks, record_hashes),
            Command::GetHash => Ok(Reply::Hash {
                tick: self.simulation.tick(),
                hash: Hash64(self.simulation.hash()),
            }),
            Command::GetHashTree => Ok(Reply::HashTree {
                tick: self.simulation.tick(),
                tree: self.simulation.hash_tree(),
            }),
            Command::TakeSnapshot => Ok(Reply::Snapshot {
                tick: self.simulation.tick(),
                snapshot: Bytes(self.simulation.snapshot().encode()),
            }),
            Command::Restore { snapshot } => {
                let snapshot = decode_snapshot(&snapshot)?;
                self.simulation
                    .restore(&snapshot)
                    .map_err(|e| ProtocolError::new(ErrorCode::InvalidSnapshot, e))?;
                Ok(Reply::Done)
            }
            Command::DiffSnapshots { a, b } => {
                let (a, b) = (decode_snapshot(&a)?, decode_snapshot(&b)?);
                let differences = self
                    .simulation
                    .world()
                    .diff_snapshots(&a, &b)
                    .map_err(|e| ProtocolError::new(ErrorCode::InvalidSnapshot, e))?;
                Ok(Reply::Differences {
                    lines: differences.iter().map(ToString::to_string).collect(),
                })
            }
            Command::ListEntities => Ok(Reply::Entities {
                entities: self.simulation.world().entities().collect(),
            }),
            Command::GetEntity { entity } => {
                let components = self
                    .simulation
                    .world()
                    .entity_components_ron(entity)
                    .ok_or_else(|| {
                        ProtocolError::new(
                            ErrorCode::NoSuchEntity,
                            format!("entity {entity} is not alive; list the entities first"),
                        )
                    })?;
                Ok(Reply::Entity { entity, components })
            }
        }
    }

    fn step(&mut self, ticks: u32, record_hashes: bool) -> Result<Reply, ProtocolError> {
        let mut hashes = Vec::new();
        for _ in 0..ticks {
            let tick = self.simulation.tick();
            let inputs = self.inputs.as_ref().and_then(|log| log.tick(tick));
            self.simulation.step_encoded(inputs).map_err(|e| {
                ProtocolError::new(
                    ErrorCode::InvalidInputs,
                    format!("tick {tick}: {e}; earlier ticks stay applied"),
                )
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
}

fn decode_snapshot(bytes: &Bytes) -> Result<Snapshot, ProtocolError> {
    Snapshot::decode(&bytes.0).map_err(|e| ProtocolError::new(ErrorCode::InvalidSnapshot, e))
}
