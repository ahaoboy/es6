# es6

A small ECMAScript engine written in safe Rust, inspired by QuickJS and
rquickjs.

The engine follows the same pipeline as QuickJS:

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
following the style of rquickjs. Evaluation, building native values, and
driving the promise job queue are all available through the public API.

## Performance

The VM uses stack-resident locals, fused bytecode, property caches, and
integer/index fast paths. On a local V7 benchmark run it reaches roughly
QuickJS parity overall, without a JIT.

## Limitations

- No JIT.
- No `Proxy`/`Reflect`, `WeakMap`/`WeakSet`, generators, or static modules.
- No garbage collector; object reference cycles are not reclaimed.
- Timers are not part of the core; promises run through the job queue.
