# Porting quickjs-oxide's execution design

This note records what changed in `quickjs-oxide` after
`38e7ca3ad4ddc0de234f850810d172532703f91a`, the measured source of its
speedup, and how the mechanisms map onto `es6`.

## What the four-fold number means

The baseline frozen by `38e7ca3` was the old stack/refcount core at
`8e16f672`: **4.49x slower than QuickJS in wall time and 5.19x more
instructions** over 37 fixed-work programs (V8 v7, Octane 2.0, AWFY).

The new core reaches, relative to that baseline:

| Suite | Old / new wall time | New / QuickJS |
|---|---:|---:|
| V8 v7 (8 programs) | **3.68x** | 0.91x |
| Octane 2.0 (15) | **3.84x** | 1.12x |
| AWFY (14) | **5.69x** | 0.97x |
| 37 programs (geometric mean) | **~4.3-4.4x** | ~1.0x |

The aggregate is more than four-fold, but individual results range from
about **1.2x** (compile-bound Code-Load) to **11x** (Bounce). RegExp
programs gain only about 1.5-1.8x because both cores share the regexp
engine. It is not a single optimization and not a uniform multiplier.

## The four mechanisms and their measured contribution

### 1. An 8-byte `Copy` value over arenas and a mark-sweep heap

The old `JsValue` was 16 bytes, non-`Copy`, and every stack/register
position owned a handle that had to be duplicated and released. Reference
count release alone cost **675 instructions per iteration** on the
constructor probe. The new `Value` is a `u64` NaN-boxed word with a 32-bit
arena index payload; heap records live in typed arenas and property values
in a shared `ValueHeap`.

Measured heap-only gains: **4.0x** on `{a:i,b:i}`, **2.0x** on `new P(i)`,
and **25.8x** on closure allocation. This is the prerequisite for
everything else: a register VM only pays once operands are plain `Copy`
words. The old core's dispatch stayed at **61-82 instructions per opcode**;
a register rewrite before the value change had stalled.

### 2. Register bytecode, stack-to-register lowering, and one loop

- `Insn` is 8 bytes: typed opcode + three `u8` registers + `u32` immediate.
- A frame owns a 256-register window; operand-stack depth becomes the
  register allocator during lowering.
- Calls, returns and `new` for bytecode functions stay in `run_at`; a
  callee's window overlaps the caller's argument registers, so arguments
  are not copied.
- The loop dispatches a typed enum, uses inline `Int32`/double paths, and
  moves rare opcodes behind out-of-line helpers.

Measured on 20 fixed probes: **3.79x fewer instructions** and about
**2.6x** less time than the old core. The loop shrank from 46 KB to
12.7 KB. `numeric_local_loop` fell from 1,007 to 160 instructions;
closures from 18,881 to 1,827.

This is the largest source of the end-to-end gain.

### 3. Inline caches guarded by shapes, validity cells and generations

A 16-byte side entry per named-property site:

| Kind | Guard | Hit |
|---|---|---|
| Get | receiver shape; holder shape + validity cell | slot load |
| Set | shape before/after; cell, room, extensible | slot store |
| Global | global generation | slot load/store |
| Construct | constructor shape; live prototype slot | receiver sizing |

Measured probe gains: `prop_read` 1.35x, `prop_write` 1.30x, depth-3
prototype read **2.52x**, constructor **1.73x**. On the V8 v7 set these
took the new core from 2.6x to **3.63x** versus the old core and 0.93x
QuickJS.

The critical correctness detail is invalidation: shape changes, per-chain
validity cells, a global generation, and weak cache pruning after each
collection.

### 4. Measured refinements

After the substrate and caches, profile-driven changes added the last
5-25%:

- exact radix-10 formatting without `Big` (Splay -24.4%),
- inline double `Mul`/`Div`, `length` cache, captured-cell fast path,
- 4,096-entry megamorphic table (RayTrace -4%),
- receiver pre-sizing and learned constructor sizes,
- loop-test inversion, in-loop `enter_frame`/return, array-write fusion,
- pristine-array iteration with no iterator or result allocation
  (`for_of_array` about 20x faster than the old core).

## How `es6` differs today

| Area | `es6` | quickjs-oxide |
|---|---|---|
| Value | 16-byte enum, `Gc`/`Rc` handles | 8-byte `Copy` NaN-boxed index |
| Heap | Boa `Gc` with per-handle refcounts | typed arenas + value heap, no refcounts |
| GC roots | refcount root inference | explicit root stack, register file, handles |
| Bytecode | stack machine with fused stack opcodes | register bytecode, 256-register window |
| Calls | Rust recursion through `call_value` | in-loop calls and returns |
| Property IC | shape/holder/prototype-generation, `RefCell` cache | 16-byte side cache, validity cells, global/construct caches |
| Shape | shared `Rc<Shape>` with key vector + hash index | parent-linked transitions, lazy tables, dictionary mode |

Property and global caches already exist in `es6`, so the cache gap is
smaller than in quickjs-oxide's baseline. The missing substrate, not the
cache, is the limiting factor.

## What was applied now

### Lazy shape index (`src/object/mod.rs`)

`Shape` used to copy the parent shape's entire `FxHashMap` for every
transition, and built a hash map even for two-property shapes.
`Shape.index` is now an `OnceCell`:

- shapes with at most eight properties use a linear scan;
- larger shapes build the hash index once, lazily;
- `add_transition` no longer clones the parent's index;
- `grow` updates the index only if it has been materialized;
- `shrink` rebuilds it only if materialized.

A dynamic-property microbenchmark (50,000 objects x 12 unique property
names) moved from 598 ms to 591 ms, and transitions no longer allocate a
full hash map per shape. The EJS regression still produces
`qmtUsIz04xxiNW`, and GC stress passes.

This is a safe, incremental application of the lazy-table idea, not a
source of four-fold speedup.

### Rejected: in-loop calls on the current stack VM

An experimental `Step::Call` path kept synchronous bytecode calls in
`run_loop` with an explicit caller-frame stack. It was reverted because it
was **slower** on this substrate (V8 suite roughly 566-591 vs 600, and the
call microbenchmark regressed). Without register windows and `Copy`
operands, the frame-stack bookkeeping, callee preparation and `Step`
traffic cost more than the removed Rust recursion. This matches
quickjs-oxide's own phase-1 conclusion: the register/call work must follow
the value/heap replacement.

## Recommended porting stages

### Phase 0: freeze the measurement gates

Use fixed-work programs and record output, wall time, and instruction
counts. Keep the EJS hash and the GC stress test as semantic gates. `es6`
already has the pinned V8 v7 suite and GC stress scripts.

### Phase 1: value and heap substrate

Replace `JsValue` with an 8-byte `Copy` word and remove per-move
refcounting. The recommended variant is quickjs-oxide's index arena:

- NaN-boxed `u64` (Int32, special, Object, String, Symbol, BigInt);
- typed arenas with free lists and side mark/live bitmaps;
- a shared `ValueHeap` for property slots, elements and environment slots;
- explicit roots: root stack, handles, register file, frames, jobs;
- non-moving mark-sweep at allocating safepoints;
- a GC-stress mode that collects before every allocation.

This conflicts with the current Boa `Gc` design and cannot be half-ported:
objects, environments and native data all move together. Expect about
**3-5 weeks** for the substrate alone. The alternative, 48-bit pointer NaN
boxes, needs collector support for raw pointers and is more error-prone.

### Phase 2: register bytecode and lowering

- Define an 8-byte instruction and a validator.
- Lower the existing stack bytecode with a symbolic stack:
  `this`, parameters, locals, then one temporary per stack depth.
- Materialize values before joins, calls and potentially throwing
  instructions.
- Add a 256-register window and a spill scope for larger functions.
- Enter/return calls inside the dispatch loop; keep natives and
  async/generator suspensions on the general path first.

Expect **4-6 weeks**. Do not start this before Phase 1 is stable.

### Phase 3: side-store caches

Move caches beside the register file. Add:
- monomorphic Get/Set/Global/Construct entries,
- per-prototype validity cells and dependents,
- global generation,
- polymorphic rows and a shared megamorphic table only when hit statistics
  justify them,
- weak pruning at collection.

Expect **1-2 weeks** once the substrate exists.

### Phase 4: refinements

Add loop inversion, receiver pre-sizing, length caches, double fast paths,
array-write fusion and pristine-array iteration based on profiles.
Expect another **1-3 weeks** for the execution core; builtin breadth is a
separate effort.

## Risks

- A missed root or a recycled arena index is silent memory corruption in
  release builds. GC stress and liveness checks are mandatory.
- Validity cells and constructor/prototype caches must be invalidated on
  every structural mutation; stale hits are user-visible bugs.
- Code layout is noisy: measure retired instructions, not one wall-time
  sample.
- The Boa collector cannot be retained under `Copy` values; the heap
  replacement is all-or-nothing.
- Four-fold is a suite aggregate. Expect less on regexp and compile-heavy
  code.

## References

`quickjs-oxide` (local checkout):

- `docs/performance/README.md`, `docs/performance/execution.md`, `docs/heap.md`
- `docs/architecture.md`
- `core/src/value.rs`, `core/src/bytecode/mod.rs`, `core/src/bytecode/builder.rs`
- `core/src/vm/interp.rs`, `core/src/vm/ic.rs`, `core/src/shape.rs`
- `core/src/heap/arena.rs`, `core/src/heap/values.rs`, `core/src/heap/gc.rs`
- `src/engine/compiler/register/mod.rs`

Historical measurement pages (`baseline.md`, `new-core-phase1-results.md`,
`new-core-phase2-results.md`, `new-core-phase3-results.md`,
`new-core-phase4-results.md`) are available at commit `0b3722f2^` and were
removed by `7d86f66b`.
