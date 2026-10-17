//! Type-erased storage for the values of one component in one archetype.

use std::any::Any;

use crate::Component;

/// A `Vec<T>` of one component type, seen without its type.
pub trait Column: Any {
    /// Removes `row`, moving the last value into its place.
    fn swap_remove(&mut self, row: usize);

    /// Removes `row` like [`Column::swap_remove`] and pushes its value at
    /// the end of `destination`, a column of the same type.
    fn move_row(&mut self, row: usize, destination: &mut dyn Column);
}

impl<T: Component> Column for Vec<T> {
    fn swap_remove(&mut self, row: usize) {
        Vec::swap_remove(self, row);
    }

    fn move_row(&mut self, row: usize, destination: &mut dyn Column) {
        let value = Vec::swap_remove(self, row);
        downcast_mut::<T>(destination).push(value);
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

    #[test]
    fn move_row_pushes_the_value_at_the_end_of_the_destination() {
        let mut source: Box<dyn Column> = Box::new(vec![1u8, 2, 3]);
        let mut destination: Box<dyn Column> = Box::new(vec![9u8]);

        source.move_row(0, destination.as_mut());

        assert_eq!(downcast_ref::<u8>(source.as_ref()), &[3, 2]);
        assert_eq!(downcast_ref::<u8>(destination.as_ref()), &[9, 1]);
    }
}
