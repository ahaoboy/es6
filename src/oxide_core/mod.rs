//! Foundation for the staged quickjs-oxide-style core.
//!
//! This module is feature-gated while the new value/heap substrate is being
//! brought up beside the existing stack VM. It is not yet wired into the
//! interpreter; keeping it isolated lets the migration proceed in small,
//! testable steps without destabilizing the current engine.

pub mod arena;
pub mod value;

pub use arena::Arena;
pub use value::{BigIntId, ObjectId, StringId, SymbolId, Value, ValueKind};
