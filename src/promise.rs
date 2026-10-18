//! Promise reactions and the microtask queue.
//!
//! Promise state, reactions and the job list mirror `JSPromiseData` /
//! `JSContext.job_list` in QuickJS; jobs are drained by
//! `JS_ExecutePendingJob` (quickjs.c:2525). Native handlers carry their
//! captured state in GC-visible data cells, the equivalent of QuickJS C
//! functions with data (`js_c_function_data`, quickjs.c:6595) whose payloads
//! `mark_children` walks (quickjs.c:7255).

use std::cell::RefCell;
use std::rc::Rc;

use crate::object::{
    NativeData, ObjRef, ObjectKind, PromiseReaction, PromiseState, PropKey,
};
use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};
use crate::vm::Job;

pub fn is_promise_object(object: &ObjRef) -> bool {
    matches!(object.borrow().kind, ObjectKind::Promise(_))
}

pub fn promise_object(value: &JsValue) -> Option<ObjRef> {
    let object = value.as_object()?;
    is_promise_object(&object.to_ref()).then(|| object.to_ref())
}

pub fn state(object: &ObjRef) -> Option<(PromiseState, JsValue)> {
    match &object.borrow().kind {
        ObjectKind::Promise(promise) => Some((promise.state, promise.value.clone())),
        _ => None,
    }
}

pub fn fulfill(realm: &mut Realm, promise: &ObjRef, value: JsValue) -> JsResult<()> {
    settle(realm, promise, PromiseState::Fulfilled, value)
}

pub fn reject(realm: &mut Realm, promise: &ObjRef, value: JsValue) -> JsResult<()> {
    settle(realm, promise, PromiseState::Rejected, value)
}

fn settle(
    realm: &mut Realm,
    promise: &ObjRef,
    state: PromiseState,
    value: JsValue,
) -> JsResult<()> {
    let reactions = {
        let mut object = promise.borrow_mut();
        let ObjectKind::Promise(data) = &mut object.kind else {
            return Err(realm.type_error("Invalid promise"));
        };
        if data.state != PromiseState::Pending {
            return Ok(());
        }
        data.state = state;
        data.value = value.clone();
        std::mem::take(&mut data.reactions)
    };
    for reaction in reactions {
        realm.enqueue_job(Job::Reaction {
            reaction,
            state,
            value: value.clone(),
        });
    }
    Ok(())
}

/// `PromiseResolve`: unwrap our own promises, adopt foreign thenables.
pub fn resolve_value(realm: &mut Realm, value: JsValue) -> JsResult<ObjRef> {
    if let Some(promise) = promise_object(&value) {
        return Ok(promise);
    }
    let promise = realm.promise();
    resolve(realm, &promise, value)?;
    Ok(promise)
}

/// `js_promise_resolve_function`: a self-resolving promise rejects.
pub fn resolve(realm: &mut Realm, promise: &ObjRef, value: JsValue) -> JsResult<()> {
    if let JsValue::Object(object) = &value
        && Rc::ptr_eq(object, promise)
    {
        return reject(
            realm,
            promise,
            realm.type_error("Chaining cycle detected for promise"),
        );
    }
    if let Some(target) = promise_object(&value) {
        return attach(realm, &target, None, None, Some(promise.clone()));
    }
    if let JsValue::Object(object) = &value {
        let then = match crate::vm::get_value_property(
            realm,
            &JsValue::Object(object.clone()),
            &PropKey::from("then"),
        ) {
            Ok(then) => then,
            Err(error) => return reject(realm, promise, error),
        };
        if then.is_function() {
            // PromiseResolveThenableJob (ES2022 27.2.1.3.2): the `then` call
            // runs as a queued job, never synchronously.
            realm.enqueue_job(Job::Thenable {
                promise: promise.clone(),
                thenable: value.clone(),
                then,
            });
            return Ok(());
        }
    }
    fulfill(realm, promise, value)
}

pub fn then(
    realm: &mut Realm,
    this: &JsValue,
    on_fulfilled: JsValue,
    on_rejected: JsValue,
) -> JsResult<JsValue> {
    let object = promise_object(this).ok_or_else(|| realm.type_error("Invalid promise"))?;
    let result = realm.promise();
    attach(
        realm,
        &object,
        on_fulfilled.is_function().then_some(on_fulfilled),
        on_rejected.is_function().then_some(on_rejected),
        Some(result.clone()),
    )?;
    Ok(JsValue::Object(result))
}

/// Record a reaction. Settled promises enqueue it immediately so callbacks
/// always run from the job queue, like QuickJS' promise jobs.
pub fn attach(
    realm: &mut Realm,
    promise: &ObjRef,
    on_fulfilled: Option<JsValue>,
    on_rejected: Option<JsValue>,
    result: Option<ObjRef>,
) -> JsResult<()> {
    let reaction = PromiseReaction {
        on_fulfilled,
        on_rejected,
        promise: result,
    };
    let settled = {
        let mut object = promise.borrow_mut();
        let ObjectKind::Promise(data) = &mut object.kind else {
            return Err(realm.type_error("Invalid promise"));
        };
        match data.state {
            PromiseState::Pending => {
                data.reactions.push(reaction);
                return Ok(());
            }
            state => Some((state, data.value.clone())),
        }
    };
    let Some((state, value)) = settled else {
        return Ok(());
    };
    realm.enqueue_job(Job::Reaction {
        reaction,
        state,
        value,
    });
    Ok(())
}

/// Attach the await continuation. The suspended VM frame lives inside an
/// async-frame object, mirroring the QuickJS `JSAsyncFunction` GC object
/// that owns the suspended stack (`JSStackFrame.cur_gc_obj`,
/// quickjs.c:442), so the whole frame stays visible to the GC.
pub fn await_value(
    realm: &mut Realm,
    value: JsValue,
    frame: Box<crate::vm::Frame>,
    completion: ObjRef,
) -> JsResult<()> {
    let promise = resolve_value(realm, value)?;
    let frame_object = realm.async_frame(frame);
    let data: NativeData = Rc::new(RefCell::new(vec![
        JsValue::Object(frame_object),
        JsValue::Object(completion),
    ]));
    let on_fulfilled =
        realm.native_function_data("", 1, data.clone(), await_resumed::<true>);
    let on_rejected = realm.native_function_data("", 1, data, await_resumed::<false>);
    attach(realm, &promise, Some(on_fulfilled), Some(on_rejected), None)
}

/// `js_async_function_resolve_create`/`js_async_function_reject_create`:
/// resume the suspended frame through a queued job.
fn await_resumed<const FULFILLED: bool>(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    let result = if FULFILLED {
        Ok(crate::builtins::arg(arguments, 0))
    } else {
        Err(crate::builtins::arg(arguments, 0))
    };
    let (frame_object, completion) = match (&data.first(), &data.get(1)) {
        (Some(JsValue::Object(frame)), Some(JsValue::Object(completion))) => {
            (frame.clone(), completion.clone())
        }
        _ => return Ok(JsValue::Undefined),
    };
    let frame = frame_object.borrow_mut().kind.take_async_frame();
    if let Some(frame) = frame {
        realm.enqueue_job(Job::Resume {
            frame,
            completion,
            result,
        });
    }
    Ok(JsValue::Undefined)
}

pub fn reaction(realm: &mut Realm, reaction: PromiseReaction, state: PromiseState, value: JsValue) {
    let handler = match state {
        PromiseState::Fulfilled => reaction.on_fulfilled,
        PromiseState::Rejected => reaction.on_rejected,
        PromiseState::Pending => None,
    };
    let Some(result) = reaction.promise else {
        if let Some(handler) = handler {
            let _ = crate::vm::call_value(realm, handler, JsValue::Undefined, &[value]);
        }
        return;
    };
    let Some(handler) = handler else {
        if matches!(state, PromiseState::Fulfilled) {
            let _ = fulfill(realm, &result, value);
        } else {
            let _ = reject(realm, &result, value);
        }
        return;
    };
    match crate::vm::call_value(realm, handler, JsValue::Undefined, &[value]) {
        Ok(value) => {
            let _ = resolve(realm, &result, value);
        }
        Err(error) => {
            crate::vm::clear_error_offset();
            let _ = reject(realm, &result, error);
        }
    }
}

/// `Promise.prototype.finally` (ES2022 27.7.4.5): the callback's returned
/// thenable is awaited before the chain settles, exactly like the spec's
/// `then` composition.
pub fn finally(realm: &mut Realm, this: &JsValue, callback: JsValue) -> JsResult<JsValue> {
    if !callback.is_function() {
        return then(realm, this, callback.clone(), callback);
    }
    let promise = promise_object(this).ok_or_else(|| realm.type_error("Invalid promise"))?;
    let result = realm.promise();
    let data: NativeData = Rc::new(RefCell::new(vec![
        JsValue::Object(result.clone()),
        callback,
    ]));
    let on_fulfilled = realm.native_function_data("", 1, data.clone(), finally_then);
    let on_rejected = realm.native_function_data("", 1, data, finally_catch);
    // The pass-through result is settled by the chained data functions, not
    // by these handlers' return values, exactly like the spec's `then`
    // composition of `finally`.
    attach(realm, &promise, Some(on_fulfilled), Some(on_rejected), None)?;
    Ok(JsValue::Object(result))
}

/// Called with the settled value: run the callback, await its result, then
/// fulfill with the original value.
fn finally_then(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    let result = data[0].clone();
    let callback = data[1].clone();
    let value = crate::builtins::arg(arguments, 0);
    let callback_result = run_finally_callback(realm, &callback)?;
    let chained = resolve_value(realm, callback_result)?;
    let data: NativeData = Rc::new(RefCell::new(vec![result, value]));
    let settle = realm.native_function_data("", 1, data.clone(), pass_fulfilled);
    let propagate = realm.native_function_data("", 1, data, propagate_rejection);
    attach(realm, &chained, Some(settle), Some(propagate), None)?;
    Ok(JsValue::Undefined)
}
/// Called with the settled rejection: run the callback, await its result,
/// then reject with the original error (or the callback's own rejection).
fn finally_catch(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    let result = data[0].clone();
    let callback = data[1].clone();
    let error = crate::builtins::arg(arguments, 0);
    let callback_result = run_finally_callback(realm, &callback)?;
    let chained = resolve_value(realm, callback_result)?;
    let data: NativeData = Rc::new(RefCell::new(vec![result, error]));
    let settle = realm.native_function_data("", 1, data.clone(), reject_with_captured);
    let propagate = realm.native_function_data("", 1, data, propagate_rejection);
    attach(realm, &chained, Some(settle), Some(propagate), None)?;
    Ok(JsValue::Undefined)
}

/// The callback's synchronous exception propagates immediately.
fn run_finally_callback(realm: &mut Realm, callback: &JsValue) -> JsResult<JsValue> {
    crate::vm::call_value(realm, callback.clone(), JsValue::Undefined, &[])
}

/// Fulfill the target promise with the captured original value.
fn pass_fulfilled(
    realm: &mut Realm,
    _this: &JsValue,
    _arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    if let (Some(result), Some(value)) = (
        data.first().and_then(|value| value.as_object()),
        data.get(1),
    ) {
        fulfill(realm, &result.into_ref(), value.clone())?;
    }
    Ok(JsValue::Undefined)
}

/// Reject the target promise with the captured original error.
fn reject_with_captured(
    realm: &mut Realm,
    _this: &JsValue,
    _arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    if let (Some(result), Some(error)) = (
        data.first().and_then(|value| value.as_object()),
        data.get(1),
    ) {
        reject(realm, &result.into_ref(), error.clone())?;
    }
    Ok(JsValue::Undefined)
}

/// Reject the target promise with the propagated rejection.
fn propagate_rejection(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    if let Some(result) = data.first().and_then(|value| value.as_object()) {
        reject(realm, &result.into_ref(), crate::builtins::arg(arguments, 0))?;
    }
    Ok(JsValue::Undefined)
}

/// `Promise.all` (ES2022 27.2.4.1). Shared mutable state lives in a
/// combinator record object held by the element function data cells, the
/// same shape QuickJS uses for its promise combinators.
pub fn all(realm: &mut Realm, iterable: JsValue) -> JsResult<JsValue> {
    let items = match crate::vm::iterable_items(realm, iterable) {
        Ok(items) => items,
        Err(error) => return rejected(realm, error),
    };
    let result = realm.promise();
    if items.is_empty() {
        let empty = realm.array(Vec::new());
        fulfill(realm, &result, JsValue::Object(empty))?;
        return Ok(JsValue::Object(result));
    }
    let record = combinator_record(realm, &result, items.len());
    for (index, item) in items.into_iter().enumerate() {
        let promise = resolve_value(realm, item)?;
        let element: NativeData = Rc::new(RefCell::new(vec![
            JsValue::Object(record.clone()),
            JsValue::number(index as f64),
        ]));
        let on_fulfilled =
            realm.native_function_data("", 1, element.clone(), all_resolve_element);
        let on_rejected = realm.native_function_data("", 1, element, all_reject_element);
        attach(realm, &promise, Some(on_fulfilled), Some(on_rejected), None)?;
    }
    Ok(JsValue::Object(result))
}

/// `Promise.race` (ES2022 27.2.4.4): the first settlement wins.
pub fn race(realm: &mut Realm, iterable: JsValue) -> JsResult<JsValue> {
    let items = match crate::vm::iterable_items(realm, iterable) {
        Ok(items) => items,
        Err(error) => return rejected(realm, error),
    };
    let result = realm.promise();
    let record = combinator_record(realm, &result, 0);
    for item in items {
        let promise = resolve_value(realm, item)?;
        let element: NativeData = Rc::new(RefCell::new(vec![JsValue::Object(record.clone())]));
        let on_fulfilled =
            realm.native_function_data("", 1, element.clone(), race_resolve_element);
        let on_rejected = realm.native_function_data("", 1, element, race_reject_element);
        attach(realm, &promise, Some(on_fulfilled), Some(on_rejected), None)?;
    }
    Ok(JsValue::Object(result))
}

/// Internal combinator record: `result`, `remaining`, `done` and `values`,
/// mirroring QuickJS' promise combinator records. Plain own properties of
/// an ordinary object, so the GC walks them through the data cells.
fn combinator_record(realm: &Realm, result: &ObjRef, count: usize) -> ObjRef {
    let record = realm.object();
    let mut data = record.borrow_mut();
    data.define(
        "result",
        crate::object::Property::data(JsValue::Object(result.clone())),
    );
    data.define(
        "remaining",
        crate::object::Property::data(JsValue::number(count as f64)),
    );
    data.define(
        "done",
        crate::object::Property::data(JsValue::Bool(false)),
    );
    let values = realm.array(Vec::new());
    data.define(
        "values",
        crate::object::Property::data(JsValue::Object(values)),
    );
    drop(data);
    record
}

fn record_field(record: &ObjRef, name: &str) -> Option<JsValue> {
    record.borrow().get_own_value(&PropKey::from(name))
}

fn record_number(record: &ObjRef, name: &str) -> f64 {
    record_field(record, name)
        .and_then(|value| value.as_number())
        .unwrap_or(f64::NAN)
}

fn record_bool(record: &ObjRef, name: &str) -> bool {
    record_field(record, name)
        .as_ref()
        .map(|value| crate::vm::to_boolean(value))
        .unwrap_or(false)
}

fn set_record_field(record: &ObjRef, name: &str, value: JsValue) {
    record
        .borrow_mut()
        .define(name, crate::object::Property::data(value));
}

/// The `Promise.all` element resolve function: store the element value and
/// fulfill with the values array once every element has arrived
/// (ES2022 27.2.4.1.1).
fn all_resolve_element(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    let record = data[0]
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| realm.type_error("Invalid combinator record"))?;
    let index = match data.get(1).and_then(JsValue::as_number) {
        Some(number) if number.fract() == 0.0 && number >= 0.0 => number as usize,
        _ => return Ok(JsValue::Undefined),
    };
    if record_bool(&record, "done") {
        return Ok(JsValue::Undefined);
    }
    let values = record_field(&record, "values")
        .and_then(|value| value.as_object())
        .ok_or_else(|| realm.type_error("Invalid combinator record"))?;
    let _ = values.set_index(index, crate::builtins::arg(arguments, 0));
    let remaining = record_number(&record, "remaining") - 1.0;
    set_record_field(&record, "remaining", JsValue::number(remaining));
    if remaining <= 0.0 {
        set_record_field(&record, "done", JsValue::Bool(true));
        let result = record_field(&record, "result")
            .and_then(|value| value.as_object())
            .map(|object| object.into_ref())
            .ok_or_else(|| realm.type_error("Invalid combinator record"))?;
        let values = record_field(&record, "values").unwrap_or(JsValue::Undefined);
        fulfill(realm, &result, values)?;
    }
    Ok(JsValue::Undefined)
}

/// The `Promise.all` element reject function: first rejection rejects the
/// result promise.
fn all_reject_element(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    let record = data[0]
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| realm.type_error("Invalid combinator record"))?;
    if !record_bool(&record, "done") {
        set_record_field(&record, "done", JsValue::Bool(true));
        let result = record_field(&record, "result")
            .and_then(|value| value.as_object())
            .map(|object| object.into_ref())
            .ok_or_else(|| realm.type_error("Invalid combinator record"))?;
        reject(realm, &result, crate::builtins::arg(arguments, 0))?;
    }
    Ok(JsValue::Undefined)
}

fn race_resolve_element(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    let record = data[0]
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| realm.type_error("Invalid combinator record"))?;
    if !record_bool(&record, "done") {
        set_record_field(&record, "done", JsValue::Bool(true));
        let result = record_field(&record, "result")
            .and_then(|value| value.as_object())
            .map(|object| object.into_ref())
            .ok_or_else(|| realm.type_error("Invalid combinator record"))?;
        fulfill(realm, &result, crate::builtins::arg(arguments, 0))?;
    }
    Ok(JsValue::Undefined)
}

fn race_reject_element(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
    data: &[JsValue],
) -> JsResult<JsValue> {
    let record = data[0]
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| realm.type_error("Invalid combinator record"))?;
    if !record_bool(&record, "done") {
        set_record_field(&record, "done", JsValue::Bool(true));
        let result = record_field(&record, "result")
            .and_then(|value| value.as_object())
            .map(|object| object.into_ref())
            .ok_or_else(|| realm.type_error("Invalid combinator record"))?;
        reject(realm, &result, crate::builtins::arg(arguments, 0))?;
    }
    Ok(JsValue::Undefined)
}

fn rejected(realm: &mut Realm, error: JsValue) -> JsResult<JsValue> {
    crate::vm::clear_error_offset();
    let promise = realm.promise();
    reject(realm, &promise, error)?;
    Ok(JsValue::Object(promise))
}
