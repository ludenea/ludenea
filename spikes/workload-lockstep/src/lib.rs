//! The lockstep spike as a library, so that a later spike can stand on its
//! world and on its protocol's values: the world of the reference workload
//! with its input fixed point and its snapshot, and the two peers, their link
//! and their messages. Throwaway, never product code.

pub mod lockstep;
pub mod world;
