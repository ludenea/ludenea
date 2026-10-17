//! Collision events: pairs that start or stop touching.

use std::collections::BTreeMap;

use ludenea_ecs::Entity;

use crate::contacts::CachedContact;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CollisionKind {
    Begin,
    End,
}

/// A pair of colliders that started or stopped touching. `a < b`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Collision {
    pub kind: CollisionKind,
    pub a: Entity,
    pub b: Entity,
}

type Pairs = BTreeMap<(Entity, Entity), CachedContact>;

fn touching(pairs: &Pairs, key: &(Entity, Entity)) -> bool {
    pairs.get(key).is_some_and(|contact| contact.touching)
}

/// The events turning the touching pairs of `previous` into those of `next`, sorted by
/// `(a, b)`. A pair missing from `next` (separated, or an entity was despawned) ends.
pub(crate) fn diff(previous: &Pairs, next: &Pairs) -> Vec<Collision> {
    let ended = previous
        .iter()
        .filter(|(key, contact)| contact.touching && !touching(next, key))
        .map(|(&(a, b), _)| Collision {
            kind: CollisionKind::End,
            a,
            b,
        });
    let began = next
        .iter()
        .filter(|(key, contact)| contact.touching && !touching(previous, key))
        .map(|(&(a, b), _)| Collision {
            kind: CollisionKind::Begin,
            a,
            b,
        });
    let mut events: Vec<Collision> = ended.chain(began).collect();
    // A pair is either ended or began, never both, so `(a, b)` has no equal elements.
    events.sort_unstable_by_key(|event| (event.a, event.b));
    events
}
