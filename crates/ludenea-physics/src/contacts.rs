//! Contact state carried from one tick to the next.

use std::collections::BTreeMap;

use ludenea_ecs::schema::Schema;
use ludenea_ecs::{Entity, Resource};
use serde::{Deserialize, Serialize};

/// Contacts of the last step, keyed by `(lower, higher)` entity: one entry per pair whose
/// bounding boxes overlapped, with the accumulated impulses used to warm start.
#[derive(Resource, Default, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[resource(name = "ludenea::ContactCache")]
pub struct ContactCache {
    pub(crate) pairs: BTreeMap<(Entity, Entity), CachedContact>,
}

#[derive(Serialize, Deserialize, Schema, Clone, Debug, PartialEq)]
pub struct CachedContact {
    /// The manifold had at least one point.
    pub touching: bool,
    pub points: Vec<CachedPoint>,
}

#[derive(Serialize, Deserialize, Schema, Copy, Clone, Debug, PartialEq)]
pub struct CachedPoint {
    /// Manifold feature id.
    pub id: u16,
    pub normal_impulse: f32,
    pub tangent_impulse: f32,
}

impl CachedContact {
    pub(crate) fn point(&self, id: u16) -> Option<&CachedPoint> {
        self.points.iter().find(|point| point.id == id)
    }
}
