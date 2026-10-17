//! Component and resource values in their protocol form: JSON in wire form, or RON for a
//! value without an exact JSON form.

use std::collections::BTreeMap;

use ludenea_app::{EditError, Simulation, ValueKind};
use ludenea_ecs::Entity;
use ludenea_ecs::schema::TypeSchema;

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
