//! Generator objects and their resumption protocol.
//!
//! QuickJS suspends a generator by stashing `JSStackFrame.cur_sp`/`cur_pc`
//! inside the generator object and resuming through `OP_generator_yield`
//! (`JS_CallInternal`, quickjs.c:18093/20871). The whole `Frame` moves in
//! and out of the `GeneratorData` payload here, which also keeps every
//! suspended reference visible to the GC.

use std::rc::Rc;

use super::frame::{Frame, RunOutcome};
use super::run_loop;
use crate::bytecode::FunctionDef;
use crate::object::{GeneratorState, JsFunction, ObjectKind, Property};
use crate::runtime::Realm;
use crate::value::{Env, JsResult, JsValue};

/// `Generator.prototype.next` (ES2022 27.5.2.2.2) and the iteration fast
/// path: start or resume the generator and return `{ value, done }`.
pub(crate) fn generator_next(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let sent = crate::builtins::arg(arguments, 0);
    let object = this
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| realm.type_error("Generator method called on incompatible receiver"))?;
    let (function, receiver, start_arguments, state) = {
        let mut data = object.borrow_mut();
        match &mut data.kind {
            ObjectKind::Generator(generator) => (
                generator.function.clone(),
                generator.this.clone(),
                std::mem::take(&mut generator.arguments),
                std::mem::replace(&mut generator.state, GeneratorState::Executing),
            ),
            _ => {
                return Err(
                    realm.type_error("Generator method called on incompatible receiver")
                )
            }
        }
    };
    let outcome = match state {
        GeneratorState::Completed => {
            return Ok(iter_result(realm, JsValue::Undefined, true));
        }
        GeneratorState::Executing => {
            return Err(realm.type_error("Generator is already running"));
        }
        GeneratorState::SuspendedStart => {
            let definition = function
                .def
                .clone()
                .ok_or_else(|| realm.type_error("Invalid generator"))?;
            let frame = start_frame(realm, definition, &function, receiver, start_arguments)?;
            run_loop(realm, frame)
        }
        GeneratorState::SuspendedYield(mut frame) => {
            // The sent value becomes the result of the suspended `yield`.
            frame.stack.push(sent);
            run_loop(realm, frame)
        }
    };
    finish_generator(realm, &object, outcome)
}

/// `Generator.prototype.return` (ES2022 27.5.2.2.4): completes the generator
/// and returns `{ value: argument, done: true }`.
pub(crate) fn generator_return(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let object = this
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| realm.type_error("Generator method called on incompatible receiver"))?;
    let mut completed = false;
    {
        let mut data = object.borrow_mut();
        if let ObjectKind::Generator(generator) = &mut data.kind
            && !matches!(generator.state, GeneratorState::Executing)
        {
            generator.state = GeneratorState::Completed;
            completed = true;
        }
    }
    let _ = completed;
    Ok(iter_result(realm, crate::builtins::arg(arguments, 0), true))
}

/// `Generator.prototype.throw` (ES2022 27.5.2.2.3): resume the generator
/// with an exception at the suspension point.
pub(crate) fn generator_throw(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let exception = crate::builtins::arg(arguments, 0);
    let object = this
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| realm.type_error("Generator method called on incompatible receiver"))?;
    let state = {
        let mut data = object.borrow_mut();
        match &mut data.kind {
            ObjectKind::Generator(generator) => {
                std::mem::replace(&mut generator.state, GeneratorState::Executing)
            }
            _ => {
                return Err(
                    realm.type_error("Generator method called on incompatible receiver")
                )
            }
        }
    };
    match state {
        GeneratorState::SuspendedYield(mut frame) => {
            if super::unwind(&mut frame, exception.clone()) {
                let outcome = run_loop(realm, frame);
                finish_generator(realm, &object, outcome)
            } else {
                set_generator_state(&object, GeneratorState::Completed);
                Err(exception)
            }
        }
        GeneratorState::SuspendedStart => {
            set_generator_state(&object, GeneratorState::Completed);
            Err(exception)
        }
        GeneratorState::Executing => {
            set_generator_state(&object, state);
            Err(realm.type_error("Generator is already running"))
        }
        GeneratorState::Completed => Ok(iter_result(realm, exception, true)),
    }
}

fn set_generator_state(object: &crate::object::ObjRef, state: GeneratorState) {
    let mut data = object.borrow_mut();
    if let ObjectKind::Generator(generator) = &mut data.kind {
        generator.state = state;
    }
}

/// Start a generator's first activation exactly like `call_value` starts a
/// normal call (ES2022 10.2.14): environment, receiver resolution and
/// argument binding.
fn start_frame(
    realm: &mut Realm,
    definition: Rc<FunctionDef>,
    function: &JsFunction,
    receiver: JsValue,
    arguments: Vec<JsValue>,
) -> JsResult<Box<Frame>> {
    let use_env = definition.use_env;
    let event_loop = if use_env {
        super::frame::EventLoop::Env
    } else {
        super::frame::EventLoop::Locals
    };
    let mut frame = super::take_frame(realm).unwrap_or_else(Frame::new);
    frame.definition = definition.clone();
    frame.env = if use_env {
        let parent = function.env.clone().unwrap_or_else(|| Env::new(0, None));
        Some(Env::new(
            definition.env_size as usize,
            Some(parent),
        ))
    } else {
        None
    };
    frame.event_loop = event_loop;
    // Generator functions use ordinary `this` semantics fixed at creation.
    frame.this = receiver;
    frame.callee = None;
    frame.arguments = arguments.into_iter().collect();
    frame.pc = 0;
    frame.completion = None;
    frame.begin(use_env);
    Ok(frame)
}

fn arguments_iter(arguments: Vec<JsValue>) -> std::vec::IntoIter<JsValue> {
    arguments.into_iter()
}

/// Convert a `run_loop` outcome into the iterator-result object, updating
/// the generator state.
fn finish_generator(
    realm: &mut Realm,
    object: &crate::object::ObjRef,
    outcome: (JsResult<RunOutcome>, Option<Box<Frame>>),
) -> JsResult<JsValue> {
    let (result, frame) = outcome;
    match result {
        Ok(RunOutcome::Return(value)) => {
            set_generator_state(object, GeneratorState::Completed);
            Ok(iter_result(realm, value, true))
        }
        Ok(RunOutcome::Suspended) => {
            set_generator_state(object, GeneratorState::Completed);
            Ok(iter_result(realm, JsValue::Undefined, true))
        }
        Ok(RunOutcome::Yield { frame, value }) => {
            set_generator_state(object, GeneratorState::SuspendedYield(frame));
            Ok(iter_result(realm, value, false))
        }
        Err(error) => {
            set_generator_state(object, GeneratorState::Completed);
            Err(error)
        }
    }
    .map(|value| {
        if let Some(frame) = frame {
            super::recycle_frame(realm, frame);
        }
        value
    })
}

/// Build the `{ value, done }` iterator result object.
pub(crate) fn iter_result(realm: &Realm, value: JsValue, done: bool) -> JsValue {
    let result = realm.object();
    let mut data = result.borrow_mut();
    data.define("value", Property::data(value));
    data.define("done", Property::data(JsValue::Bool(done)));
    drop(data);
    JsValue::Object(result)
}
