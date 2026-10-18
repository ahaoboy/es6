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
`Runtime::run_jobs`.

## Garbage collection

Cycle collection mirrors QuickJS `JS_RunGC` (quickjs.c:7483): its
`gc_decref`/`gc_scan`/`gc_free_cycles` phases reappear as an explicit
internal-in-degree count (QuickJS mutates refcounts; `Rc` counts are
immutable, so the same trial deletion is computed without touching them).
Collections trigger automatically at safe points after evaluations and
between jobs, and `Runtime::run_gc` exposes `JS_RunGC` directly. Every
object, environment, native data cell and function bytecode registers with
the collector, like QuickJS' `rt->gc_obj_list`; promise machinery carries
its state in GC-visible data cells, matching QuickJS C functions with data
(`js_c_function_data`, quickjs.c:6595), and suspended `async` frames live in
GC objects exactly like QuickJS' `JSAsyncFunction` stack
(`JSStackFrame.cur_gc_obj`, quickjs.c:442).

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
  gc.rs       trial-deletion cycle collector
  promise.rs  promise reactions, combinator records, job queue
  runtime.rs  Realm, Runtime and Context
  builtins/   one module per constructor/prototype pair
  coerce.rs   ToPrimitive/ToNumber/ToString and UTF-16 helpers
  api.rs      rquickjs-shaped lifetime-bound handles
```

## Performance

The VM uses stack-resident locals for non-capturing functions, fused
two-operand opcodes (`AddVars`, `CompareVarConst`, `UpdateVarJump`), a frame
pool, monomorphic property caches guarded by shape ids and prototype
generations, and integer fast paths on arithmetic and element access.

## Limitations

- No JIT; no `Proxy`/`Reflect`, BigInt, or static module syntax.
- Async generators are not supported; `WeakMap`/`WeakSet` keys must be
  objects.
- Strings are UTF-8 backed; unpaired surrogates become U+FFFD, and the
  engine cannot represent lone surrogate code units (UTF-16 code-unit
  indexing and lengths are correct).
- Date handling is UTC (`getTimezoneOffset()` is always `0`).
