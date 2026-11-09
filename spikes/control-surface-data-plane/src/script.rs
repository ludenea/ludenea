//! The transactions the scripted agent sends. Throwaway, never product code.
//!
//! The agent builds every transaction from what it read of the state (the
//! canonical bytes of a full-state read) and from what it remembers of the
//! labels it added. Five kinds, each at 1, 10 and 100 elements: field sets,
//! adds and removes of the label component, despawns, spawns, and a mix of
//! all of them with kicks. A transaction that must be rejected is a valid one
//! with elements replaced by ones the fixed point refuses.

use std::collections::HashMap;

use crate::commands::{Body, Element, Field, Handle, SlotView};

/// The entities the script watches and kicks; no transaction targets them.
pub const WATCHED: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixKind {
    FieldSets,
    Labels,
    Despawns,
    Spawns,
    Mixed,
}

impl MixKind {
    pub const ALL: [MixKind; 5] = [MixKind::FieldSets, MixKind::Labels, MixKind::Despawns, MixKind::Spawns, MixKind::Mixed];

    pub fn name(self) -> &'static str {
        match self {
            MixKind::FieldSets => "field sets",
            MixKind::Labels => "component adds and removes",
            MixKind::Despawns => "despawns",
            MixKind::Spawns => "spawns",
            MixKind::Mixed => "mixed",
        }
    }
}

pub const SIZES: [usize; 3] = [1, 10, 100];

/// The labels the agent added and has not removed, by slot.
#[derive(Clone, Debug, Default)]
pub struct LabelBook {
    known: HashMap<u32, (u32, u32)>,
}

impl LabelBook {
    fn labeled(&self, view: &[SlotView], index: usize) -> bool {
        self.known.get(&(index as u32)).is_some_and(|(generation, _)| view[index].alive && view[index].generation == *generation)
    }

    /// The labeled entities still alive in `view`, in slot order.
    fn alive(&self, view: &[SlotView]) -> Vec<Handle> {
        let mut handles: Vec<Handle> = (0..view.len()).filter(|&index| self.labeled(view, index)).map(|index| view[index].handle(index)).collect();
        handles.sort_unstable_by_key(|handle| handle.index);
        handles
    }

    /// The book of `take` of the labeled entities alive in `view`, after the first `skip`.
    pub fn part(&self, view: &[SlotView], skip: usize, take: usize) -> LabelBook {
        let known = self.alive(view).into_iter().skip(skip).take(take).map(|handle| (handle.index, self.known[&handle.index])).collect();
        LabelBook { known }
    }

    /// Follows a transaction that was applied and kept.
    pub fn follow(&mut self, elements: &[Element]) {
        for element in elements {
            match element {
                Element::AddLabel { entity, value } => {
                    self.known.insert(entity.index, (entity.generation, *value));
                }
                Element::RemoveLabel { entity } | Element::Despawn { entity } => {
                    self.known.remove(&entity.index);
                }
                _ => {}
            }
        }
    }
}

/// Hands out distinct alive entities of a view, from a starting slot onward.
struct Targets<'a> {
    view: &'a [SlotView],
    next: usize,
    left: usize,
}

impl<'a> Targets<'a> {
    fn new(view: &'a [SlotView], start: usize) -> Targets<'a> {
        Targets { view, next: start % view.len(), left: view.len() }
    }

    fn take(&mut self, mut wanted: impl FnMut(usize) -> bool) -> Handle {
        while self.left > 0 {
            let index = self.next;
            self.next = (self.next + 1) % self.view.len();
            self.left -= 1;
            if index >= WATCHED && self.view[index].alive && wanted(index) {
                return self.view[index].handle(index);
            }
        }
        panic!("the view has too few alive entities for the script");
    }

    fn any(&mut self) -> Handle {
        self.take(|_| true)
    }
}

fn spawned_body(i: usize) -> Body {
    Body {
        x: 100.0 + ((i * 37) % 800) as f32,
        y: 50.0 + ((i * 53) % 900) as f32,
        vx: (i % 7) as f32 - 3.0,
        vy: (i % 5) as f32 - 2.0,
        mass: [1.0, 2.0, 4.0][i % 3],
    }
}

fn field_set(entity: Handle, i: usize) -> Element {
    let field = Field::ALL[i % 5];
    let value = match field {
        Field::X | Field::Y => 37.0 + ((i * 19) % 950) as f32 + 0.5,
        Field::Vx | Field::Vy => ((i % 9) as f32 - 4.0) * 3.0,
        Field::Mass => [4.0, 1.0, 2.0][i % 3],
    };
    Element::SetField { entity, field, value }
}

/// A valid transaction of `size` elements of one kind against `view`.
pub fn build(kind: MixKind, size: usize, view: &[SlotView], book: &LabelBook, start: usize) -> Vec<Element> {
    let mut targets = Targets::new(view, start);
    let mut elements = Vec::with_capacity(size);
    let label_adds = |targets: &mut Targets, elements: &mut Vec<Element>, count: usize| {
        for i in 0..count {
            let entity = targets.take(|index| !book.labeled(view, index));
            elements.push(Element::AddLabel { entity, value: 1_000 + (start + i) as u32 });
        }
    };
    match kind {
        MixKind::FieldSets => elements.extend((0..size).map(|i| field_set(targets.any(), i))),
        MixKind::Labels => {
            let labeled = book.alive(view);
            let removes = (size / 2).min(labeled.len());
            label_adds(&mut targets, &mut elements, size - removes);
            elements.extend(labeled[..removes].iter().map(|entity| Element::RemoveLabel { entity: *entity }));
        }
        MixKind::Despawns => elements.extend((0..size).map(|_| Element::Despawn { entity: targets.any() })),
        MixKind::Spawns => elements.extend((0..size).map(|i| Element::Spawn { body: spawned_body(start + i) })),
        MixKind::Mixed if size < 10 => {
            elements.extend((0..size).map(|i| Element::Kick { entity: targets.any(), delta_vx: 2.5 + i as f32, delta_vy: -1.5 }));
        }
        MixKind::Mixed => {
            let labeled = book.alive(view);
            let structural = (size * 15 / 100).max(1);
            let (kicks, adds, removes) = (size / 10, size / 5, (size / 10).min(labeled.len()));
            let sets = size - kicks - adds - removes - 2 * structural;
            elements.extend((0..sets).map(|i| field_set(targets.any(), i)));
            elements.extend((0..kicks).map(|i| Element::Kick { entity: targets.any(), delta_vx: 80.0, delta_vy: -(i as f32) }));
            label_adds(&mut targets, &mut elements, adds);
            // The newest labels: the ones another kind's removes leave alone.
            elements.extend(labeled[labeled.len() - removes..].iter().map(|entity| Element::RemoveLabel { entity: *entity }));
            elements.extend((0..structural).map(|_| Element::Despawn { entity: targets.any() }));
            elements.extend((0..structural).map(|i| Element::Spawn { body: spawned_body(start + i) }));
        }
    }
    elements
}

/// The despawns that free the slots a transaction of spawns needs.
pub fn slots_for_spawns(size: usize, view: &[SlotView], start: usize) -> Vec<Element> {
    let mut targets = Targets::new(view, start);
    (0..size).map(|_| Element::Despawn { entity: targets.any() }).collect()
}

/// Labels on `count` entities from a starting slot: what the script adds
/// before anything else, so that removes have something to remove.
pub fn first_labels(view: &[SlotView], start: usize, count: usize) -> Vec<Element> {
    let mut targets = Targets::new(view, start);
    (0..count).map(|i| Element::AddLabel { entity: targets.any(), value: i as u32 }).collect()
}

/// One element the fixed point refuses, of three sorts in turn: a stale
/// handle, an entity outside the table, a value outside its range.
fn refused(view: &[SlotView], sort: usize) -> Element {
    let mut alive = Targets::new(view, WATCHED);
    let entity = alive.any();
    match sort % 3 {
        0 => Element::SetField { entity: Handle { generation: entity.generation + 1, ..entity }, field: Field::Vx, value: 1.0 },
        1 => Element::Despawn { entity: Handle { index: view.len() as u32 + 5, generation: 0 } },
        _ => Element::SetField { entity, field: Field::X, value: -1.0 },
    }
}

/// The transaction with its middle element replaced by a refused one;
/// returns the position.
pub fn with_one_refused(elements: &[Element], view: &[SlotView]) -> (Vec<Element>, usize) {
    let mut planted = elements.to_vec();
    let position = planted.len() / 2;
    planted[position] = refused(view, 0);
    (planted, position)
}

/// The transaction with its first, middle and last elements replaced by
/// refused ones; returns how many were planted.
pub fn with_several_refused(elements: &[Element], view: &[SlotView]) -> (Vec<Element>, usize) {
    let mut planted = elements.to_vec();
    let mut positions = vec![0, planted.len() / 2, planted.len() - 1];
    positions.dedup();
    for (sort, position) in positions.iter().enumerate() {
        planted[*position] = refused(view, sort);
    }
    (planted, positions.len())
}

/// How many restores of an inverse land on a slot a spawn of the original
/// reused: there the generation is rolled back to the despawned handle's.
pub fn restores_over_a_reused_slot(inverse: &[Element]) -> usize {
    inverse
        .iter()
        .filter(|element| {
            let Element::Restore { entity, .. } = element else { return false };
            inverse.iter().any(|other| matches!(other, Element::Despawn { entity: spawned } if spawned.index == entity.index && spawned.generation > entity.generation))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{Oracle, Transaction, Verdict, read_canonical, write_canonical};
    use reference_workload::Size;

    fn view(oracle: &mut Oracle) -> Vec<SlotView> {
        let mut bytes = Vec::new();
        write_canonical(oracle.state(), &mut bytes);
        read_canonical(&bytes).unwrap()
    }

    #[test]
    fn every_kind_and_size_is_accepted_whole_and_its_planted_variants_are_refused_as_planted() {
        let mut oracle = Oracle::new(Size::Small);
        oracle.step_to(20);
        let mut book = LabelBook::default();
        let labels = first_labels(&view(&mut oracle), 333, 40);
        assert!(oracle.transact(&Transaction::of(labels.clone())).applied);
        book.follow(&labels);
        for (round, (size, kind)) in SIZES.into_iter().flat_map(|size| MixKind::ALL.into_iter().map(move |kind| (size, kind))).enumerate() {
            oracle.step_to(20 + round as u32 * 3);
            let mut seen = view(&mut oracle);
            let start = 100 + round * 57;
            if kind == MixKind::Spawns {
                assert!(oracle.transact(&Transaction::of(slots_for_spawns(size, &seen, start + 300))).applied);
                seen = view(&mut oracle);
            }
            let elements = build(kind, size, &seen, &book, start);
            assert_eq!(elements.len(), size, "{} of {size}", kind.name());
            let before = oracle.digests();

            let (one, position) = with_one_refused(&elements, &seen);
            let outcome = oracle.transact(&Transaction::of(one));
            assert_eq!((outcome.applied, outcome.accepted(), outcome.rejected(), outcome.not_evaluated()), (false, position, 1, size - position - 1), "{} of {size}", kind.name());

            let (several, planted) = with_several_refused(&elements, &seen);
            let outcome = oracle.transact(&Transaction { full_evaluation: true, want_inverse: false, elements: several });
            assert_eq!((outcome.applied, outcome.rejected(), outcome.not_evaluated()), (false, planted, 0), "{} of {size}", kind.name());
            assert_eq!(oracle.digests(), before);

            let outcome = oracle.transact(&Transaction { full_evaluation: false, want_inverse: true, elements: elements.clone() });
            assert!(outcome.applied && outcome.verdicts.iter().all(|verdict| *verdict == Verdict::Accepted), "{} of {size}: {:?}", kind.name(), outcome.verdicts);
            assert_ne!(oracle.digests(), before, "{} of {size} changes the state", kind.name());
            let inverse = outcome.inverse.unwrap();
            let spawns = elements.iter().filter(|element| matches!(element, Element::Spawn { .. })).count();
            let rolled_back = restores_over_a_reused_slot(&inverse);
            assert!(rolled_back == 0 || kind == MixKind::Mixed);
            assert!(oracle.transact(&Transaction::of(inverse)).applied);
            let after = oracle.digests();
            assert_eq!((after.components, after.labels), (before.components, before.labels), "{} of {size}", kind.name());
            assert_eq!(after.full == before.full, spawns == rolled_back, "a spawn leaves its generation behind unless a restore lands on its slot");
            assert!(oracle.transact(&Transaction::of(elements.clone())).applied, "the original applies again");
            book.follow(&elements);
        }
    }
}
