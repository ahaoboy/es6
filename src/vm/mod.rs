//! Stack VM: activation records, the dispatch loop and job execution.
//!
//! The pipeline mirrors QuickJS: bytecode from the compiler is interpreted by
//! a stack machine whose activation records correspond to `JSStackFrame`
//! (quickjs.c:427) and whose dispatch loop is the counterpart of
//! `JS_CallInternal` (quickjs.c:18051).

mod call;
mod error;
mod frame;
pub(crate) mod generator;
mod iter;
mod ops;
mod property;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use smallvec::SmallVec;

use crate::bytecode::{Constant, FunctionDef};
use crate::object::ObjRef;
use crate::runtime::Realm;
use crate::value::{EnvRef, JsResult, JsValue};

pub use call::{call_value, construct_value};
pub use error::{display_error, range_error, type_error};
pub(crate) use frame::Frame;
pub(crate) use frame::{RunOutcome, Step};
pub use iter::iterable_items;
pub use property::{get_value_property, set_value_property};

pub use crate::coerce::{
    add_values, format_number, loose_equals, same_value, same_value_zero, strict_equals,
    to_boolean, to_int32, to_number, to_numeric, to_primitive, to_property_key, to_string,
    to_uint32,
};

/// Operand stack. Inline capacity covers typical expression depths, so most
/// `push`/`pop` pairs never touch the heap (the QuickJS equivalent lives in
/// the `JS_CallInternal` alloca, quickjs.c:18051).
pub(crate) type ValueStack = SmallVec<[JsValue; 32]>;
pub(crate) type ArgumentStack = SmallVec<[JsValue; 8]>;

/// A queued job. QuickJS stores the same entries in `JSContext.job_list` and
/// dispatches them in `JS_ExecutePendingJob` (quickjs.c:2525).
pub(crate) enum Job {
    Reaction {
        reaction: crate::object::PromiseReaction,
        state: crate::object::PromiseState,
        value: JsValue,
    },
    Resume {
        frame: Box<Frame>,
        completion: ObjRef,
        result: JsResult<JsValue>,
    },
    /// PromiseResolveThenableJob (ES2022 27.2.1.3.2): adopt a foreign
    /// thenable by calling its `then` from the job queue.
    Thenable {
        promise: ObjRef,
        thenable: JsValue,
        then: JsValue,
    },
    /// `queueMicrotask` / `Promise`-style native callback job.
    #[cfg(feature = "timers")]
    Callback {
        function: JsValue,
        arguments: Vec<JsValue>,
    },
}

thread_local! {
    /// Byte offset of the innermost uncaught VM exception. The VM keeps this
    /// out of `JsValue` so thrown values remain ordinary JavaScript values.
    static ERROR_OFFSET: Cell<Option<usize>> = const { Cell::new(None) };
    pub(crate) static CALL_STACK: RefCell<Vec<Rc<str>>> = const { RefCell::new(Vec::new()) };
}

const MAX_POOLED_FRAMES: usize = 512;

fn take_frame(realm: &mut Realm) -> Option<Box<Frame>> {
    realm.frames.pop()
}

/// Return the frame to the pool, dropping every residual reference so pooled
/// frames never pin reachable values.
fn recycle_frame(realm: &mut Realm, mut frame: Box<Frame>) {
    if realm.frames.len() < MAX_POOLED_FRAMES {
        frame.clear();
        realm.frames.push(frame);
    }
}

pub fn take_error_offset() -> Option<usize> {
    ERROR_OFFSET.with(Cell::take)
}

pub fn clear_error_offset() {
    ERROR_OFFSET.with(|offset| offset.set(None));
}

/// Entry point for compiled top-level code, like the tail of
/// `JS_EvalFunctionInternal` (quickjs.c:38108).
pub fn execute(
    realm: &mut Realm,
    definition: Rc<FunctionDef>,
    env: EnvRef,
    this: JsValue,
) -> JsResult<JsValue> {
    ERROR_OFFSET.with(|offset| offset.set(None));
    match run_frame(
        realm,
        definition,
        Some(env),
        this,
        None,
        ArgumentStack::new(),
        None,
    )? {
        RunOutcome::Return(value) => Ok(value),
        RunOutcome::Suspended | RunOutcome::Yield { .. } => Ok(JsValue::Undefined),
    }
}

#[inline(never)]
pub(crate) fn run_frame(
    realm: &mut Realm,
    definition: Rc<FunctionDef>,
    env: Option<EnvRef>,
    this: JsValue,
    callee: Option<JsValue>,
    arguments: ArgumentStack,
    completion: Option<ObjRef>,
) -> JsResult<RunOutcome> {
    let use_env = definition.use_env;
    let event_loop = if use_env {
        frame::EventLoop::Env
    } else {
        frame::EventLoop::Locals
    };
    let mut frame = take_frame(realm).unwrap_or_else(Frame::new);
    frame.definition = definition;
    frame.env = if use_env { env } else { None };
    frame.event_loop = event_loop;
    frame.this = this;
    frame.callee = callee;
    frame.arguments = arguments;
    frame.pc = 0;
    frame.completion = completion;
    frame.begin(use_env);
    let (result, frame) = run_loop(realm, frame);
    if let Some(frame) = frame {
        recycle_frame(realm, frame);
    }
    // A suspended yield carries its own frame out to the generator.
    result
}

/// Interpreter loop: fetch, dispatch, handle suspension and exceptions. The
/// counterpart of the opcode switch in `JS_CallInternal` (quickjs.c:18051).
fn run_loop(
    realm: &mut Realm,
    mut frame: Box<Frame>,
) -> (JsResult<RunOutcome>, Option<Box<Frame>>) {
    // Keep the bytecode slice in a local: it is stable for the whole call and
    // avoids reloading `frame.definition` + the `Vec` pointer per opcode.
    let definition = frame.definition.clone();
    let code = definition.code.as_slice();
    // Program counter kept in a register; synchronized with `frame.pc` when
    // an opcode suspends, unwinds or returns.
    let mut pc = frame.pc;
    // Check the allocation threshold from the dispatch loop, like QuickJS
    // calling `js_trigger_gc` from its allocation helpers. The registry keeps
    // every entity alive, so without this a long-running script would only
    // collect once when `eval` returns.
    let mut gc_countdown = 4096u32;
    loop {
        gc_countdown -= 1;
        if gc_countdown == 0 {
            gc_countdown = 4096;
            crate::gc::maybe_collect();
        }
        if pc >= code.len() {
            frame.pc = pc;
            return (Ok(RunOutcome::Return(JsValue::Undefined)), Some(frame));
        }
        let op = code[pc];
        pc += 1;
        let step = ops::execute_op(realm, &mut frame, &mut pc, op);
        match step {
            Ok(Step::Continue) => {}
            Ok(Step::Return) => {
                frame.pc = pc;
                let value = frame.stack.pop().unwrap_or(JsValue::Undefined);
                return (Ok(RunOutcome::Return(value)), Some(frame));
            }
            Ok(Step::Await) => {
                frame.pc = pc;
                let value = frame.stack.pop().unwrap_or(JsValue::Undefined);
                let Some(completion) = frame.completion.clone() else {
                    match await_sync(realm, value) {
                        Ok(value) => {
                            frame.stack.push(value);
                            continue;
                        }
                        Err(error) => return (Err(error), Some(frame)),
                    }
                };
                // The frame moves into the async-frame object owned by the
                // promise reaction data, mirroring `JSStackFrame.cur_gc_obj`
                // (quickjs.c:442).
                if let Err(error) = crate::promise::await_value(realm, value, frame, completion) {
                    return (Err(error), None);
                }
                return (Ok(RunOutcome::Suspended), None);
            }
            Ok(Step::Yield) => {
                // Generator suspension: the whole frame moves into the
                // generator object with its resume point, like QuickJS
                // stashing `cur_sp`/`cur_pc` in the generator (quickjs.c:20871).
                frame.pc = pc;
                let value = frame.stack.pop().unwrap_or(JsValue::Undefined);
                let value = RunOutcome::Yield { frame, value };
                return (Ok(value), None);
            }
            Err(exception) => {
                frame.pc = pc;
                error::record_error_stack(&exception);
                if !unwind(&mut frame, exception.clone()) {
                    if let Some(offset) = frame.definition.positions.get(pc - 1) {
                        ERROR_OFFSET.with(|current| {
                            if current.get().is_none() {
                                current.set(Some(*offset as usize));
                            }
                        });
                    }
                    return (Err(exception), Some(frame));
                }
                pc = frame.pc;
            }
        }
    }
}

/// Restore the innermost handler, exactly like the exception path of
/// `JS_CallInternal` unwinding to its `catch_pc` (quickjs.c:18051).
fn unwind(frame: &mut Frame, exception: JsValue) -> bool {
    let Some(handler) = frame.handlers.pop() else {
        return false;
    };
    ERROR_OFFSET.with(|offset| offset.set(None));
    frame.handlers.truncate(handler.index);
    frame.stack.truncate(handler.stack_len);
    frame.env = handler.env;
    frame.restore_scope(handler.local_frames, handler.locals);
    frame.stack.push(exception);
    frame.pc = handler.catch_pc;
    true
}

/// `await` in a non-async context keeps the old synchronous behaviour.
fn await_sync(_realm: &mut Realm, value: JsValue) -> JsResult<JsValue> {
    let Some(promise) = crate::promise::promise_object(&value) else {
        return Ok(value);
    };
    let Some((state, value)) = crate::promise::state(&promise) else {
        return Ok(JsValue::Undefined);
    };
    match state {
        crate::object::PromiseState::Fulfilled => Ok(value),
        crate::object::PromiseState::Rejected => Err(value),
        crate::object::PromiseState::Pending => Ok(JsValue::Undefined),
    }
}

/// PromiseResolveThenableJob: call `then(thenable)` with resolve/reject
/// element functions that settle the promise exactly once
/// (ES2022 27.2.1.3.2). The shared once-only flag lives in a record object
/// held by the data cell, so it is mutable and GC-visible, exactly like
/// QuickJS' shared `JSFuncData` for thenable adoption.
fn run_thenable_job(realm: &mut Realm, promise: ObjRef, thenable: JsValue, then: JsValue) {
    use crate::object::NativeData;
    let record = realm.object();
    {
        let mut data = record.borrow_mut();
        data.define(
            "promise",
            crate::object::Property::data(JsValue::Object(promise.clone())),
        );
        data.define(
            "called",
            crate::object::Property::data(JsValue::Bool(false)),
        );
    }
    let data: NativeData = Rc::new(RefCell::new(vec![JsValue::Object(record)]));
    let resolve_function =
        realm.native_function_data("", 1, data.clone(), thenable_resolve_element);
    let reject_function = realm.native_function_data("", 1, data.clone(), thenable_reject_element);
    if let Err(error) =
        crate::vm::call_value(realm, then, thenable, &[resolve_function, reject_function])
    {
        // The `then` call threw before settling: reject the promise unless
        // one of the element functions already ran.
        let (promise, called) = thenable_record(realm, &data);
        if !called {
            let _ = crate::promise::reject(realm, &promise, error);
        }
    }
}

/// `(promise, called)` snapshot of a thenable adoption record.
fn thenable_record(realm: &mut Realm, data: &crate::object::NativeData) -> (ObjRef, bool) {
    let cell = data.borrow();
    let record = cell
        .first()
        .and_then(JsValue::as_object)
        .map(|object| object.into_ref());
    let Some(record) = record else {
        let promise = realm.promise();
        return (promise, true);
    };
    let promise = record
        .borrow()
        .get_own_value(&crate::object::PropKey::from("promise"))
        .and_then(|value| value.as_object())
        .map(|object| object.into_ref())
        .unwrap_or_else(|| realm.promise());
    let called = record
        .borrow()
        .get_own_value(&crate::object::PropKey::from("called"))
        .as_ref()
        .map(crate::vm::to_boolean)
        .unwrap_or(false);
    (promise, called)
}

/// Mark the record consumed; `Ok(Some(promise))` when this call settles.
fn thenable_claim(realm: &Realm, data: &[JsValue]) -> Option<ObjRef> {
    let record = data
        .first()
        .and_then(JsValue::as_object)
        .map(|object| object.into_ref())?;
    let promise = record
        .borrow()
        .get_own_value(&crate::object::PropKey::from("promise"))
        .and_then(|value| value.as_object())
        .map(|object| object.into_ref());
    let called = record
        .borrow()
        .get_own_value(&crate::object::PropKey::from("called"))
        .as_ref()
        .map(crate::vm::to_boolean)
        .unwrap_or(false);
    if called {
        return None;
    }
    record
        .borrow_mut()
        .define("called", crate::object::Property::data(JsValue::Bool(true)));
    let _ = realm;
    promise
}

fn thenable_resolve_element(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    if let Some(promise) = thenable_claim(realm, data) {
        crate::promise::resolve(realm, &promise, crate::builtins::arg(arguments, 0))?;
    }
    Ok(JsValue::Undefined)
}

fn thenable_reject_element(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    if let Some(promise) = thenable_claim(realm, data) {
        crate::promise::reject(realm, &promise, crate::builtins::arg(arguments, 0))?;
    }
    Ok(JsValue::Undefined)
}

/// Run one queued job, like `JS_ExecutePendingJob` (quickjs.c:2525).
pub(crate) fn run_job(realm: &mut Realm, job: Job) -> JsResult<()> {
    match job {
        Job::Reaction {
            reaction,
            state,
            value,
        } => {
            crate::promise::reaction(realm, reaction, state, value);
            Ok(())
        }
        Job::Thenable {
            promise,
            thenable,
            then,
        } => {
            run_thenable_job(realm, promise, thenable, then);
            Ok(())
        }
        #[cfg(feature = "timers")]
        Job::Callback {
            function,
            arguments,
        } => {
            crate::vm::call_value(realm, function, JsValue::Undefined, &arguments)?;
            Ok(())
        }
        Job::Resume {
            mut frame,
            completion,
            result,
        } => {
            frame.completion = Some(completion.clone());
            match result {
                Ok(value) => frame.stack.push(value),
                Err(exception) => {
                    if !unwind(&mut frame, exception.clone()) {
                        recycle_frame(realm, frame);
                        let _ = crate::promise::reject(realm, &completion, exception);
                        return Ok(());
                    }
                }
            }
            let (result, frame) = run_loop(realm, frame);
            if let Some(frame) = frame {
                recycle_frame(realm, frame);
            }
            match result {
                Ok(RunOutcome::Return(value)) => {
                    crate::promise::resolve(realm, &completion, value)?;
                }
                Ok(RunOutcome::Suspended | RunOutcome::Yield { .. }) => {}
                Err(error) => {
                    clear_error_offset();
                    let _ = crate::promise::reject(realm, &completion, error);
                }
            }
            Ok(())
        }
    }
}

/// Constant pool access shared by the opcode handlers. QuickJS indexes the
/// same data through `JSFunctionBytecode.cpool` (quickjs.c:855).
pub(crate) fn constant_string(definition: &FunctionDef, index: u32) -> crate::string::JsString {
    match definition.constants.get(index as usize) {
        Some(Constant::String(value)) => value.clone(),
        _ => crate::string::JsString::from(""),
    }
}

/// Constant pool access shared by the opcode handlers. QuickJS indexes the
/// same data through `JSFunctionBytecode.cpool` (quickjs.c:855).
pub(crate) fn constant_strings(
    definition: &FunctionDef,
    index: u32,
) -> Rc<[crate::string::JsString]> {
    match definition.constants.get(index as usize) {
        Some(Constant::Strings(value)) => value.clone(),
        _ => Rc::from([]),
    }
}
