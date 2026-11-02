//! The target of the one-line change the spike makes in the core to measure
//! an incremental build after an edit. The spike rewrites the value below,
//! builds, and restores the file.

pub const EDIT_MARKER: u32 = 0;
