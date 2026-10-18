//! The flattened form of a prefab, loaded as an asset.

use std::collections::BTreeSet;

use ludenea_macros::Schema;
use serde::{Deserialize, Serialize};

use crate::Asset;

/// A prefab with its nested instances expanded and their overrides applied, in prefab
/// order. Every `Entity` value inside a component names the position of its target in
/// `entities` (generation 0); a reference to an entity the prefab does not have names
/// position `u32::MAX`.
#[derive(Asset, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[asset(name = "ludenea::Prefab")]
pub struct Prefab {
    pub entities: Vec<PrefabEntity>,
    /// The component types present whose schema contains `Entity`.
    pub entity_components: BTreeSet<String>,
}

/// One entity of a flattened prefab: its address inside the prefab (local identifiers,
/// deeper for nested instances) and its components' canonical bytes, by stable name.
#[derive(Schema, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrefabEntity {
    pub address: Vec<u32>,
    pub components: Vec<(String, Vec<u8>)>,
}
