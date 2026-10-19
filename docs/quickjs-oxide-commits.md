# quickjs-oxide 38e7ca3..main commit ledger

Performance-relevant commits and whether each can be applied independently
to `es6`. The short answer: the runtime speedup is one architectural unit
(the new core), not a set of cherry-pickable patches. The commits below are
ordered as they occurred.

## Runtime substrate (the ~4x)

| Commit | Change | Measured effect | es6 status |
|---|---|---|---|
| `77e4a2f7` | Phase 1: 8-byte NaN-boxed `Value`, typed arenas, value heap, mark-sweep, shapes | heap-only 4.0x/2.0x; closure allocation 25.8x; removes 675 instr/iter of refcount release | blocked: requires replacing the 16-byte `JsValue`/Boa `Gc` model |
| `74e0fe70` | Phase 2 M1: 8-byte register instruction set, code store, validator | loop foundation | blocked by the value/code model |
| `9327b365` | Phase 2 M2: lower stack code to register code using stack depth as register allocator | eliminates stack pushes/pops/shuffles | blocked; needs register ISA |
| `f067f6f2` | Phase 2 M3/M4: register machine, in-loop calls/returns/new, handler-table unwinding | 3.79x fewer instructions than old core; loop 12.7 KB vs 46 KB; numeric loop 6.2x, call0 3.4x | blocked; a stack-VM try measured slower and was reverted |
| `503bcd34` | Phase 2 M6: exit tables | part of the register loop | blocked; register-only |
| `8027454f` | Phase 3 M1: GetField cache | prop_read 1.35x, depth-3 prototype read 2.52x | partial: es6 has shape/holder property caches but not the 16-byte side-store/validity design |
| `b85da1e7` | Phase 3 M2: Set/Global/Construct caches, accessors, ic-stats | prop_write 1.30x, constructor 1.73x | partial: global and Set caches exist; constructor cache was tried and showed no measurable gain here |
| `2a086a34` | Phase 3 M5: profile-guided tuning (radix, doubles, length cache, mega table, receiver sizing) | Splay -24.4%, RayTrace -8.2%, Navier -5.9% | selectively applicable, but measured gains depend on the new core |

## Porting and language surface

| Commit | Change | es6 status |
|---|---|---|
| `6bd4fd8f` | arguments/rest/apply/call/spread/for-in/RegExp literals | some equivalents already exist; not the 4x source |
| `c47a2a46` | oxide-regexp crate | es6 uses `regress`; separate engine project |
| `bd090e45` | V8 v7 realm builtins | language/completeness, not the VM substrate |
| `8b40459b` | full builtins, strings, Test262 surface | language/completeness |
| `b527f955` | oxide-run driver | test harness, not runtime |
| `0b3722f2` | phase 6 baseline/API audit | measurement/docs |
| `64b3a689` | embedding API over the new core | architecture/API |
| `2cdd1052` | playground/adapters/Test262 runner | host/tooling |
| `7d86f66b` | cut over, delete old core, 0.3.0 | final cutover |
| `a99c68fc` | release 0.3.0 | release |
| `689296ae` | build probe without removed feature list | build tooling |

## Post-release, not runtime speed

| Commit | Change | es6 status |
|---|---|---|
| `ff19f353` | linear-time compilation: one-pass handler table, indexed names, one analysis, one-pass source positions, buffer reuse | independently applicable in principle; compile-time only |
| `87a34e30` | reclaim dynamic code, caches and atoms | memory/reclamation; not execution speed |

## Dependency conclusion

`77e4a2f7` → `74e0fe70` → `9327b365` → `f067f6f2` is a single dependency
chain:

- register instructions require the new value/code store;
- the register machine requires the lowered code;
- the caches and tuning require the register loop;
- the in-loop call optimization requires register windows and `Copy`
  operands.

Applying `8027454f`, `b85da1e7` or `2a086a34` alone to the current stack
VM gives only marginal gains because the dominant costs are value copies,
refcount traffic and stack dispatches.

The current `es6` changes are limited to mechanisms that do not require the
new core. The main independently applicable item is the lazy shape index
already applied in `a8420b5`.
