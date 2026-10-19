# es6

A small ECMAScript engine written in safe Rust, modeled on QuickJS
(quickjs-ng) and the `rquickjs` API style.

## Build and run

```sh
cargo build --release
cargo run --release -- script.js
cargo run --release -- -e 'console.log(1 + 2)'
```

The CLI is enabled by default. For library-only builds:

```sh
cargo build --release --no-default-features
```

Localized date formatting is optional because the ICU4X/CLDR data is large:

```sh
cargo build --release --features intl
```

The `intl` feature adds `Intl.DateTimeFormat` and
`Date.prototype.toLocaleString` / `toLocaleDateString` / `toLocaleTimeString`.
It uses the system time zone and DST rules; the regular Date arithmetic remains
the specification-defined UTC implementation.

CLI options:

```text
es6 [OPTIONS] [FILE]

  -e, --eval <CODE>  Evaluate a source string instead of a file
  -q, --quiet        Do not print the result of a top-level expression
  -h, --help         Print help
  -V, --version      Print version
```

## Library

The crate exposes a `Runtime` and `Context` API with lifetime-bound values,
following the style of `rquickjs`: `Context::with(|ctx| ...)` borrows a
`Ctx<'js>`, scripts are evaluated through `ctx.eval`, and `Value`, `Object`,
`Array` and `Function` carry the context lifetime. Promise reactions and
`async`/`await` continuations run through a QuickJS-style job queue that
`Context::eval` drains before returning; hosts can drive it themselves with
`Runtime::execute_pending_job`, `Runtime::is_job_pending` and
`Runtime::run_jobs`. ES modules are declared and evaluated through
`Context::declare_module` / `Context::eval_module`, which return the module's
exports namespace.

## Garbage collection

Objects, environments and native data cells are managed by the Boa-derived
mark-sweep collector in `src/gc`. Handles use `Gc<T>` root accounting; cycles
are collected by tracing, and `WeakMap`/`WeakSet` entries use ephemeron
semantics. Allocation triggers collection once the heap crosses an adaptive
byte threshold, and `Runtime::run_gc` forces a full collection.

## Module layout

```text
src/
  lexer/      token types + byte-oriented scanner (quickjs next_token)
  parser/     statement, expression, function and class parsing
  ast.rs      syntax tree
  compiler/   scope resolution + bytecode emission, split like the
              QuickJS passes (statements / expressions / classes / patterns)
  bytecode.rs opcode set, mirroring quickjs-opcode.h
  vm/         activation records, dispatch loop, call machinery, property
              paths with inline caches, iteration protocol, generators
  object/     object model, exotic payloads, public handles
  gc/         Boa-derived mark-sweep collector and engine integration
  promise.rs  promise reactions, combinator records, job queue
  runtime.rs  Realm, Runtime and Context
  builtins/   one module per constructor/prototype pair
              (`intl.rs` is compiled with the optional `intl` feature)
  coerce.rs   ToPrimitive/ToNumber/ToString and UTF-16 helpers
  api.rs      rquickjs-shaped lifetime-bound handles
  module.rs   module declaration, evaluation and exports
  string.rs   thin reference-counted JS strings
```

## Performance

The VM keeps locals on the frame stack for non-capturing functions, uses a
frame pool, and fuses the common hot paths: loop increments and tests,
comparisons with branches, integer arithmetic and assignments, and global
and property reads with inline caches. Strings are thin reference-counted
values so every JS value stays at 16 bytes, and the release profile enables
fat LTO with a single codegen unit.

See [`docs/quickjs-oxide-port.md`](docs/quickjs-oxide-port.md) for the
comparison with quickjs-oxide's register/8-byte-value core and the staged
porting plan, and [`docs/quickjs-oxide-commits.md`](docs/quickjs-oxide-commits.md)
for the commit-by-commit ledger over `38e7ca3..main`.

Additional hot-path work:

- One fused dispatch switch (the QuickJS `JS_CallInternal` shape) with the
  program counter and bytecode slice held in locals; splitting the dispatch
  into a "hot" pre-pass and a second full match measurably hurt and was
  removed.
- Superinstructions for the dominant patterns: fused comparison/branch and
  loop heads (`ForNext*`), fused `local.prop` reads and writes, fused
  `local.method(args)` / `this.method(args)` calls, fused parameter binding,
  and declared-function calls skip the per-call self-name binding.
- `for (let ...)` and `for (const ... of ...)` bindings are only materialized
  per iteration when a nested function can observe them, so the common loop
  compiles to the same fused shape as a `var` loop.
- Capture is tracked per binding, like QuickJS' `JSVarDef.is_captured`: a
  function that contains a closure keeps uncaptured locals in flat frame
  slots and stores only captured bindings in its environment. Bytecode slot
  operands carry an environment bit, so the existing fused instructions work
  for both storage classes without extra dispatch.
- Objects use a shared, interned shape (`Rc<Shape>` with a slot-aligned
  `Vec<Property>`), the counterpart of QuickJS' `JSShape` and its transition
  table. Creating a literal or a constructor-built object copies a shape
  pointer and a property vector instead of rebuilding a hash map, and
  property inline caches validate the shape id before reading the cached
  slot directly.
- Property inline caches carry the per-site canonical key and a prepared
  object-literal layout, so repeated literal evaluation does not re-hash or
  re-parse keys; `.prototype` is created lazily and function names reuse a
  per-bytecode atom. Each site keeps up to four recent shapes (own slots, or
  receiver shapes sharing the cached prototype holder), matching the
  polymorphism of real object graphs instead of thrashing a single entry.
  Constructor field writes reuse the interned shape transition and slot
  cached on first insertion.
- Strings are ASCII-fast: `length`, `charAt`, `charCodeAt`, `substring`,
  `slice`, `substr` and `String.fromCharCode` avoid the UTF-16
  encode/decode round trip for the overwhelmingly common ASCII case.
- The call path avoids redundant reference traffic: the common bytecode
  call does not clone the `JsFunction` handle, the callee's `this` is
  resolved lazily (sloppy calls that never read it skip the global object),
  the argument vector is inline for up to four arguments, and a call in
  statement position drops its result inside the call (`CallDiscard`)
  instead of pushing and popping it. `run_loop` recycles its frame directly
  on every non-suspending exit.
- `Map`/`Set` are insertion-ordered hash tables with SameValueZero keys
  (linear scanning was quadratic before), and primitive string receivers no
  longer allocate a temporary wrapper object per property access.

Measured with the micro-kernels in this repository against qjs-ng on the
same machine (pinned to one core): integer loops are ~1.3x QuickJS, own
property access ~1.65x, prototype property access ~1.75x, object allocation
~2x, a one-argument call ~2x, and `Map`/`Set` and exception handling are
faster than QuickJS; regexp execution is at parity. The V8 benchmark suite
runs at roughly 0.4 of qjs-ng's score. The remaining gap is dominated by
the opcode dispatch loop and the reference-counted `Rc<RefCell>` value
model rather than by frame allocation or property layout: complex
expressions cost 2.5-3.8x while a plain increment loop is only 1.3x, and
recursive call micro-benchmarks are ~6x because every opcode pays a `match`
dispatch and `Result` return.

Deep recursion is bounded by `Context::set_max_call_depth` (a `RangeError`
like QuickJS' stack limit); the CLI runs the engine on a 64 MB stack.

## Limitations

- No JIT; no `Proxy`/`Reflect`, BigInt, or dynamic `import()`.
- Async generators are not supported; `WeakMap`/`WeakSet` keys must be
  objects.
- Strings are UTF-8 backed; unpaired surrogates become U+FFFD, and the
  engine cannot represent lone surrogate code units (UTF-16 code-unit
  indexing and lengths are correct).
- Date local-time getters/setters and `getTimezoneOffset()` remain UTC. With
  the optional `intl` feature, `Date.prototype.toLocale*` and
  `Intl.DateTimeFormat` use the system time zone and DST rules; other Intl
  constructors and the full ECMA-402 option set are not implemented.
