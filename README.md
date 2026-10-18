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
`Runtime::run_jobs`. ES modules are declared and evaluated through
`Context::declare_module` / `Context::eval_module`, which return the module's
exports namespace.

## Garbage collection

Objects are reference counted, and a trial-deletion cycle collector mirrors
QuickJS' `JS_RunGC` phases. The collector registry holds weak handles, so
acyclic garbage is freed immediately; only cycles survive to a cycle scan.
Collections run automatically as the heap grows during execution, keeping
long-running programs bounded, and `Runtime::run_gc` forces one.

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

Deep recursion is bounded by `Context::set_max_call_depth` (a `RangeError`
like QuickJS' stack limit); the CLI runs the engine on a 64 MB stack.

## Limitations

- No JIT; no `Proxy`/`Reflect`, BigInt, or dynamic `import()`.
- Async generators are not supported; `WeakMap`/`WeakSet` keys must be
  objects.
- Strings are UTF-8 backed; unpaired surrogates become U+FFFD, and the
  engine cannot represent lone surrogate code units (UTF-16 code-unit
  indexing and lengths are correct).
- Date handling is UTC (`getTimezoneOffset()` is always `0`).
