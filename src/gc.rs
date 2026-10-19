//! Garbage-collector integration.
//!
//! The engine uses Boa's `boa_gc` mark-sweep collector. `Gc<T>` handles keep
//! the collector's root accounting, cycles are collected by tracing, and
//! `WeakGc`/`Ephemeron`/`WeakMap` provide weak references. Allocation drives
//! collection automatically once the collector's byte threshold is reached;
//! explicit safe points can force a collection through [`run_gc_now`].

use boa_gc::{Gc, GcRefCell};

use crate::object::{ObjRef, ObjectData};

/// Allocate a traced object cell.
pub(crate) fn allocate_object(data: ObjectData) -> ObjRef {
    Gc::new(GcRefCell::new(data))
}

/// Force a full collection (`Context::run_gc` / `Runtime::run_gc`).
pub fn run_gc_now() {
    boa_gc::force_collect();
}
