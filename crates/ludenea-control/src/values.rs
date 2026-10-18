//! Component, resource, and asset values in their protocol form: JSON in wire form, or RON
//! for a value without an exact JSON form.

use std::collections::BTreeMap;

use ludenea_app::{EditError, Simulation, ValueKind};
use ludenea_ecs::schema::TypeSchema;
use ludenea_ecs::{AssetId, Entity};

use crate::json::to_wire;
use crate::protocol::ComponentValue;

/// The component values of `entity` keyed by stable name, if it is alive.
pub(crate) fn entity_values(
    sim: &Simulation,
    entity: Entity,
) -> Option<BTreeMap<String, ComponentValue>> {
    let world = sim.world();
    let json = world.entity_components_json(entity)?;
    let components = json
        .into_iter()
        .map(|(name, json)| {
            let value = match json {
                Ok(value) => json_value(sim, ValueKind::Component, &name, value),
                Err(_) => ComponentValue::Ron {
                    text: world
                        .component_ron(entity, &name)
                        .expect("the same component"),
                },
            };
            (name, value)
        })
        .collect();
    Some(components)
}

/// The `name` component of `entity`, if it has one.
pub(crate) fn component_value(
    sim: &Simulation,
    entity: Entity,
    name: &str,
) -> Option<ComponentValue> {
    let world = sim.world();
    Some(match world.component_json(entity, name)? {
        Ok(value) => json_value(sim, ValueKind::Component, name, value),
        Err(_) => ComponentValue::Ron {
            text: world
                .component_ron(entity, name)
                .expect("the same component"),
        },
    })
}

/// Every resource value keyed by stable name.
pub(crate) fn resource_values(sim: &Simulation) -> BTreeMap<String, ComponentValue> {
    sim.world()
        .resource_schemas()
        .into_iter()
        .map(|(name, _)| (name.to_string(), resource_value(sim, name)))
        .collect()
}

/// The value of the registered resource `name`.
///
/// # Panics
/// If `name` is not registered.
pub(crate) fn resource_value(sim: &Simulation, name: &str) -> ComponentValue {
    let world = sim.world();
    match world.resource_json(name).expect("a registered resource") {
        Ok(value) => json_value(sim, ValueKind::Resource, name, value),
        Err(_) => ComponentValue::Ron {
            text: world.resource_ron(name).expect("a registered resource"),
        },
    }
}

fn json_value(
    sim: &Simulation,
    kind: ValueKind,
    name: &str,
    value: serde_json::Value,
) -> ComponentValue {
    let schema = value_schema(sim, kind, name).expect("a value of the world is registered");
    ComponentValue::Json {
        value: to_wire(value, &schema),
    }
}

pub(crate) fn value_schema(
    sim: &Simulation,
    kind: ValueKind,
    name: &str,
) -> Result<TypeSchema, EditError> {
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

/// The type name, schema, and JSON value (or why it has none) of the loaded asset `id`.
pub(crate) fn asset_json(
    sim: &Simulation,
    id: AssetId,
) -> Option<(String, TypeSchema, Result<serde_json::Value, String>)> {
    let world = sim.world();
    let (asset, bytes) = world.asset_bytes(id)?;
    let codec = world.asset_codec(asset).expect("a loaded asset's type");
    let value = (codec.decode)(&bytes).expect("a loaded asset's bytes decode");
    Some((
        asset.to_string(),
        (codec.schema)(),
        (codec.to_json)(&*value),
    ))
}

/// The type name and value of the loaded asset `id`.
pub(crate) fn asset_value(sim: &Simulation, id: AssetId) -> Option<(String, ComponentValue)> {
    let (asset, schema, json) = asset_json(sim, id)?;
    let value = match json {
        Ok(value) => ComponentValue::Json {
            value: to_wire(value, &schema),
        },
        Err(_) => ComponentValue::Ron {
            text: sim.world().asset_ron(id).expect("the same asset"),
        },
    };
    Some((asset, value))
}
