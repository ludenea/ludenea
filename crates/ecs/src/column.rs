//! Type-erased storage for the values of one component in one archetype.

use std::any::Any;

use crate::Component;

/// A `Vec<T>` of one component type, seen without its type.
pub trait Column: Any {
    /// Removes `row`, moving the last value into its place.
    fn swap_remove(&mut self, row: usize);
}

impl<T: Component> Column for Vec<T> {
    fn swap_remove(&mut self, row: usize) {
        Vec::swap_remove(self, row);
    }
}

/// # Panics
///
/// Panics when `column` does not hold `T` values, which the archetype key
/// rules out.
pub(crate) fn downcast_ref<T: Component>(column: &dyn Column) -> &Vec<T> {
    let column: &dyn Any = column;
    column
        .downcast_ref()
        .expect("a column holds the component type its identifier names")
}

/// # Panics
///
/// Panics when `column` does not hold `T` values, which the archetype key
/// rules out.
pub(crate) fn downcast_mut<T: Component>(column: &mut dyn Column) -> &mut Vec<T> {
    let column: &mut dyn Any = column;
    column
        .downcast_mut()
        .expect("a column holds the component type its identifier names")
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Component for u8 {}

    #[test]
    fn swap_remove_moves_the_last_value_into_the_hole() {
        let mut column: Box<dyn Column> = Box::new(vec![1u8, 2, 3]);

        column.swap_remove(0);

        assert_eq!(downcast_ref::<u8>(column.as_ref()), &[3, 2]);
    }
}
