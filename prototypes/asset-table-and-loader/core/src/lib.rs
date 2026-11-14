//! The core of the prototype of the asset table and of the loader: the asset
//! identity core (`table`) inside a stand-in for the engine's world (`world`),
//! which is the reference workload's world of the asset table spike. Throwaway,
//! never product code. It reads no file and knows no store.

pub mod table;
pub mod world;
