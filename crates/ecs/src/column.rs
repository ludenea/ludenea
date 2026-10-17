//! Type-erased storage for the values of one component in one archetype, or
//! for the value of one resource.

use std::any::Any;

/// A `Vec<T>` of one component or resource type, seen without its type.
pub trait Column: Any {
    /// Removes `row`, moving the last value into its place.
    fn swap_remove(&mut self, row: usize);

    /// Removes `row` like [`Column::swap_remove`] and pushes its value at
    /// the end of `destination`, a column of the same type.
    fn move_row(&mut self, row: usize, destination: &mut dyn Column);

    fn boxed_clone(&self) -> Box<dyn Column>;

    /// Copies `self` into `destination`, reusing its allocation. Returns
    /// `false`, leaving `destination` untouched, when the types differ.
    fn copy_into(&self, destination: &mut dyn Column) -> bool;
}

impl<T: Copy + 'static> Column for Vec<T> {
    fn swap_remove(&mut self, row: usize) {
        Vec::swap_remove(self, row);
    }

    fn move_row(&mut self, row: usize, destination: &mut dyn Column) {
        let value = Vec::swap_remove(self, row);
        downcast_mut::<T>(destination).push(value);
    }

    fn boxed_clone(&self) -> Box<dyn Column> {
        Box::new(self.clone())
    }

    fn copy_into(&self, destination: &mut dyn Column) -> bool {
        let destination: &mut dyn Any = destination;
        match destination.downcast_mut::<Self>() {
            Some(destination) => {
                destination.clone_from(self);
                true
            }
            None => false,
        }
    }
}

impl Clone for Box<dyn Column> {
    fn clone(&self) -> Self {
        self.boxed_clone()
    }

    fn clone_from(&mut self, source: &Self) {
        if !source.copy_into(self.as_mut()) {
            *self = source.boxed_clone();
        }
    }
}

/// # Panics
///
/// Panics when `column` does not hold `T` values, which the archetype key
/// rules out.
pub(crate) fn downcast_ref<T: Copy + 'static>(column: &dyn Column) -> &Vec<T> {
    let column: &dyn Any = column;
    column
        .downcast_ref()
        .expect("a column holds the component type its identifier names")
}

/// # Panics
///
/// Panics when `column` does not hold `T` values, which the archetype key
/// rules out.
pub(crate) fn downcast_mut<T: Copy + 'static>(column: &mut dyn Column) -> &mut Vec<T> {
    let column: &mut dyn Any = column;
    column
        .downcast_mut()
        .expect("a column holds the component type its identifier names")
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn clone_from_reuses_the_allocation_of_the_same_type() {
        let source: Box<dyn Column> = Box::new(vec![1u8, 2]);
        let mut destination: Box<dyn Column> = Box::new(Vec::<u8>::with_capacity(8));
        let allocation = downcast_ref::<u8>(destination.as_ref()).as_ptr();

        destination.clone_from(&source);

        let destination = downcast_ref::<u8>(destination.as_ref());
        assert_eq!(destination, &[1, 2]);
        assert_eq!(destination.as_ptr(), allocation);
    }

    #[test]
    fn clone_from_replaces_a_column_of_another_type() {
        let source: Box<dyn Column> = Box::new(vec![7u16]);
        let mut destination: Box<dyn Column> = Box::new(vec![1u8]);

        destination.clone_from(&source);

        assert_eq!(downcast_ref::<u16>(destination.as_ref()), &[7]);
    }
}
