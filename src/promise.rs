//! Promise reactions and the microtask queue.
//!
//! QuickJS keeps promise state, reactions and the job list in
//! `JSPromiseData` / `JSContext.job_list`; the functions here are the safe
//! Rust equivalents of `js_promise_resolve`, `js_promise_then` and
//! `JS_ExecutePendingJob` (quickjs.c:2525).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::object::{ObjRef, ObjectKind, PromiseReaction, PromiseState, PropKey};
use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};
use crate::vm::{Job, SuspendedFrame};

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

/// `js_promise_resolve_function`.
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
            let called = Rc::new(Cell::new(false));
            let resolve_function = realm.native_function("", 1, {
                let promise = promise.clone();
                let called = called.clone();
                move |realm, _this, arguments| {
                    if !called.replace(true) {
                        resolve(realm, &promise, argument(arguments, 0))?;
                    }
                    Ok(JsValue::Undefined)
                }
            });
            let reject_function = realm.native_function("", 1, {
                let promise = promise.clone();
                let called = called.clone();
                move |realm, _this, arguments| {
                    if !called.replace(true) {
                        reject(realm, &promise, argument(arguments, 0))?;
                    }
                    Ok(JsValue::Undefined)
                }
            });
            if let Err(error) = crate::vm::call_value(
                realm,
                then,
                value.clone(),
                &[resolve_function, reject_function],
            ) && !called.get()
            {
                called.set(true);
                reject(realm, promise, error)?;
            }
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

/// Attach the await continuation. The suspended VM frame is resumed after the
/// promise settles.
pub fn await_value(
    realm: &mut Realm,
    value: JsValue,
    frame: SuspendedFrame,
    completion: ObjRef,
) -> JsResult<()> {
    let promise = resolve_value(realm, value)?;
    let fulfilled = realm.native_function("", 1, {
        let frame = frame.clone();
        let completion = completion.clone();
        move |realm, _this, arguments| {
            resume(realm, &frame, &completion, Ok(argument(arguments, 0)));
            Ok(JsValue::Undefined)
        }
    });
    let rejected = realm.native_function("", 1, {
        let completion = completion.clone();
        move |realm, _this, arguments| {
            resume(realm, &frame, &completion, Err(argument(arguments, 0)));
            Ok(JsValue::Undefined)
        }
    });
    attach(realm, &promise, Some(fulfilled), Some(rejected), None)
}

fn resume(
    realm: &mut Realm,
    frame: &SuspendedFrame,
    completion: &ObjRef,
    result: JsResult<JsValue>,
) {
    if let Some(frame) = frame.borrow_mut().take() {
        realm.enqueue_job(Job::Resume {
            frame,
            completion: completion.clone(),
            result,
        });
    }
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
        } else if matches!(state, PromiseState::Rejected) {
            crate::vm::clear_error_offset();
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

/// `Promise.prototype.finally` in terms of `then`.
pub fn finally(realm: &mut Realm, this: &JsValue, callback: JsValue) -> JsResult<JsValue> {
    if !callback.is_function() {
        return then(realm, this, callback.clone(), callback);
    }
    let on_fulfilled = realm.native_function("", 1, {
        let callback = callback.clone();
        move |realm, _this, arguments| {
            crate::vm::call_value(realm, callback.clone(), JsValue::Undefined, &[])?;
            Ok(argument(arguments, 0))
        }
    });
    let on_rejected = realm.native_function("", 1, {
        let callback = callback.clone();
        move |realm, _this, arguments| {
            crate::vm::call_value(realm, callback.clone(), JsValue::Undefined, &[])?;
            Err(argument(arguments, 0))
        }
    });
    then(realm, this, on_fulfilled, on_rejected)
}

/// `Promise.all`.
pub fn all(realm: &mut Realm, iterable: JsValue) -> JsResult<JsValue> {
    let items = match crate::vm::iterable_items(realm, iterable) {
        Ok(items) => items,
        Err(error) => return rejected(realm, error),
    };
    let result = realm.promise();
    if items.is_empty() {
        fulfill(realm, &result, JsValue::Object(realm.array(Vec::new())))?;
        return Ok(JsValue::Object(result));
    }
    let values = Rc::new(RefCell::new(vec![JsValue::Undefined; items.len()]));
    let remaining = Rc::new(Cell::new(items.len()));
    let done = Rc::new(Cell::new(false));
    for (index, item) in items.into_iter().enumerate() {
        let promise = resolve_value(realm, item)?;
        let on_fulfilled = realm.native_function("", 1, {
            let values = values.clone();
            let remaining = remaining.clone();
            let done = done.clone();
            let result = result.clone();
            move |realm, _this, arguments| {
                if !done.get() {
                    values.borrow_mut()[index] = argument(arguments, 0);
                    let left = remaining.get() - 1;
                    remaining.set(left);
                    if left == 0 {
                        done.set(true);
                        let items = values.borrow().clone();
                        fulfill(realm, &result, JsValue::Object(realm.array(items)))?;
                    }
                }
                Ok(JsValue::Undefined)
            }
        });
        let on_rejected = realm.native_function("", 1, {
            let done = done.clone();
            let result = result.clone();
            move |realm, _this, arguments| {
                if !done.replace(true) {
                    reject(realm, &result, argument(arguments, 0))?;
                }
                Ok(JsValue::Undefined)
            }
        });
        attach(realm, &promise, Some(on_fulfilled), Some(on_rejected), None)?;
    }
    Ok(JsValue::Object(result))
}

/// `Promise.race`.
pub fn race(realm: &mut Realm, iterable: JsValue) -> JsResult<JsValue> {
    let items = match crate::vm::iterable_items(realm, iterable) {
        Ok(items) => items,
        Err(error) => return rejected(realm, error),
    };
    let result = realm.promise();
    let done = Rc::new(Cell::new(false));
    for item in items {
        let promise = resolve_value(realm, item)?;
        let on_fulfilled = realm.native_function("", 1, {
            let done = done.clone();
            let result = result.clone();
            move |realm, _this, arguments| {
                if !done.replace(true) {
                    fulfill(realm, &result, argument(arguments, 0))?;
                }
                Ok(JsValue::Undefined)
            }
        });
        let on_rejected = realm.native_function("", 1, {
            let done = done.clone();
            let result = result.clone();
            move |realm, _this, arguments| {
                if !done.replace(true) {
                    reject(realm, &result, argument(arguments, 0))?;
                }
                Ok(JsValue::Undefined)
            }
        });
        attach(realm, &promise, Some(on_fulfilled), Some(on_rejected), None)?;
    }
    Ok(JsValue::Object(result))
}

fn rejected(realm: &mut Realm, error: JsValue) -> JsResult<JsValue> {
    crate::vm::clear_error_offset();
    let promise = realm.promise();
    reject(realm, &promise, error)?;
    Ok(JsValue::Object(promise))
}

fn argument(arguments: &[JsValue], index: usize) -> JsValue {
    arguments.get(index).cloned().unwrap_or(JsValue::Undefined)
}
