# GC module

This module contains a trimmed, in-tree copy of Boa's `boa_gc` crate
(version 0.22.0, <https://github.com/boa-dev/boa>), adapted to live inside the
engine as `crate::gc`.

Upstream license: `Unlicense OR MIT` (see `LICENSE-MIT` and `LICENSE-UNLICENSE`).

## Local changes

- The `boa_macros` dependency and derived `Trace`/`Finalize` implementations
  were removed; `WeakGc` and `WeakMap` have manual implementations. The module
  re-exports the `Trace`/`Finalize` traits and the `custom_trace!`/`empty_trace!`
  macros used by the engine.
- Test-only modules and helpers are not included.
- Optional trace integrations (`thin-vec`, `arrayvec`, `icu`, `boa_string`,
  `either`) were removed along with their optional dependencies.
- `hashbrown` remains a dependency for the weak-map table.
- The collection threshold starts at 8 MiB instead of 1 MiB.
- The second mark pass after finalization is skipped. Engine values have no
  resurrecting finalizers (all `Finalize` implementations are empty apart from
  the collector's own reference-count bookkeeping), so sweeping immediately
  after finalization is safe.
- `Gc::drop` decrements the reference count directly on the fast path, still
  guarded by the collector's drop flag.
- `Gc::clone`/`deref` and the reference-count helpers are marked inline.
