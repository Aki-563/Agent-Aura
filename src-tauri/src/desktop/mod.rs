//! Desktop hygiene: keeping Aura out of its own screenshots, tracking which window the
//! agent is operating, and making sure input only lands where it should.

pub mod affinity;
pub mod panel;
pub mod target;
pub mod layout;
pub mod obstruction;
pub mod registry;
pub mod windowops;
