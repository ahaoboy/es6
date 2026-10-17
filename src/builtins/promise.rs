use super::*;
use crate::object::{ObjectKind, PromiseData, PromiseState};
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicPromise / js_promise_* (quickjs.c).
// This synchronous implementation preserves the observable API while a
// dedicated job queue is not required by the CLI.
pub fn install(realm: &mut Realm) {
    let prototype = realm.promise_proto.clone();
    let constructor = install_constructor(realm, "Promise", 1, constructor, &prototype);
    let target = constructor.as_object().unwrap().to_ref();
    define_method(realm, &target, "resolve", 1, resolve);
    define_method(realm, &target, "reject", 1, reject);
    define_method(realm, &target, "all", 1, all);
    define_method(realm, &target, "race", 1, race);
    define_method(realm, &prototype, "then", 2, then);
    define_method(realm, &prototype, "catch", 1, catch_method);
    define_method(realm, &prototype, "finally", 1, finally);
}

fn constructor(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let executor = arg(arguments, 0);
    if !executor.is_function() {
        return Err(realm.type_error("Promise resolver is not a function"));
    }
    let promise = JsValue::Object(realm.promise());
    let resolve = bound_function(
        realm,
        realm.native_function("resolve", 1, settle_fulfilled),
        promise.clone(),
        Vec::new(),
        "resolve",
        1,
    );
    let reject = bound_function(
        realm,
        realm.native_function("reject", 1, settle_rejected),
        promise.clone(),
        Vec::new(),
        "reject",
        1,
    );
    if let Err(error) = super::call(realm, executor, JsValue::Undefined, &[resolve, reject]) {
        crate::vm::clear_error_offset();
        set_state(realm, &promise, PromiseState::Rejected, error)?;
    }
    Ok(promise)
}

fn settle_fulfilled(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    set_state(realm, this, PromiseState::Fulfilled, arg(arguments, 0))?;
    Ok(JsValue::Undefined)
}

fn settle_rejected(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    set_state(realm, this, PromiseState::Rejected, arg(arguments, 0))?;
    Ok(JsValue::Undefined)
}

fn set_state(
    realm: &mut Realm,
    promise: &JsValue,
    state: PromiseState,
    value: JsValue,
) -> JsResult<()> {
    let object = promise
        .as_object()
        .ok_or_else(|| realm.type_error("Invalid promise"))?;
    let reference = object.to_ref();
    let mut data = reference.borrow_mut();
    if let ObjectKind::Promise(promise) = &mut data.kind {
        if promise.state == PromiseState::Pending {
            promise.state = state;
            promise.value = value;
        }
        Ok(())
    } else {
        Err(realm.type_error("Invalid promise"))
    }
}

fn state(realm: &Realm, value: &JsValue) -> JsResult<PromiseData> {
    let object = value
        .as_object()
        .ok_or_else(|| realm.type_error("Invalid promise"))?;
    let data = object.to_ref();
    match &data.borrow().kind {
        ObjectKind::Promise(promise) => Ok(promise.clone()),
        _ => Err(realm.type_error("Invalid promise")),
    }
}

fn resolve(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = arg(arguments, 0);
    if is_promise(&value) {
        return Ok(value);
    }
    let promise = realm.promise();
    if let ObjectKind::Promise(data) = &mut promise.borrow_mut().kind {
        data.state = PromiseState::Fulfilled;
        data.value = value;
    }
    Ok(JsValue::Object(promise))
}

fn reject(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let promise = realm.promise();
    if let ObjectKind::Promise(data) = &mut promise.borrow_mut().kind {
        data.state = PromiseState::Rejected;
        data.value = arg(arguments, 0);
    }
    Ok(JsValue::Object(promise))
}

fn then(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let data = state(realm, this)?;
    let on_fulfilled = arg(arguments, 0);
    let on_rejected = arg(arguments, 1);
    let result = realm.promise();
    match data.state {
        PromiseState::Fulfilled => {
            if on_fulfilled.is_function() {
                match super::call(realm, on_fulfilled, JsValue::Undefined, &[data.value]) {
                    Ok(value) => resolve_promise_value(realm, &result, value),
                    Err(error) => {
                        crate::vm::clear_error_offset();
                        reject_promise_value(realm, &result, error);
                    }
                }
            } else {
                resolve_promise_value(realm, &result, data.value);
            }
        }
        PromiseState::Rejected => {
            if on_rejected.is_function() {
                match super::call(realm, on_rejected, JsValue::Undefined, &[data.value]) {
                    Ok(value) => resolve_promise_value(realm, &result, value),
                    Err(error) => {
                        crate::vm::clear_error_offset();
                        reject_promise_value(realm, &result, error);
                    }
                }
            } else {
                reject_promise_value(realm, &result, data.value);
            }
        }
        PromiseState::Pending => {
            if let ObjectKind::Promise(result) = &mut result.borrow_mut().kind {
                result.state = PromiseState::Pending;
            }
        }
    }
    Ok(JsValue::Object(result))
}

fn catch_method(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    then(realm, this, &[JsValue::Undefined, arg(arguments, 0)])
}

fn finally(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let callback = arg(arguments, 0);
    let data = state(realm, this)?;
    if callback.is_function()
        && let Err(error) = super::call(realm, callback, JsValue::Undefined, &[])
    {
        crate::vm::clear_error_offset();
        return Err(error);
    }
    let result = realm.promise();
    match data.state {
        PromiseState::Fulfilled => resolve_promise_value(realm, &result, data.value),
        PromiseState::Rejected => reject_promise_value(realm, &result, data.value),
        PromiseState::Pending => {}
    }
    Ok(JsValue::Object(result))
}

fn all(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let items = crate::vm::iterable_items(realm, arg(arguments, 0))?;
    let result = realm.promise();
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        if is_promise(&item) {
            let data = state(realm, &item)?;
            match data.state {
                PromiseState::Rejected => {
                    reject_promise_value(realm, &result, data.value);
                    return Ok(JsValue::Object(result));
                }
                PromiseState::Fulfilled => values.push(data.value),
                PromiseState::Pending => values.push(JsValue::Undefined),
            }
        } else {
            values.push(item);
        }
    }
    resolve_promise_value(realm, &result, JsValue::Object(realm.array(values)));
    Ok(JsValue::Object(result))
}

fn race(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let items = crate::vm::iterable_items(realm, arg(arguments, 0))?;
    let result = realm.promise();
    if let Some(first) = items.into_iter().next() {
        if is_promise(&first) {
            let data = state(realm, &first)?;
            match data.state {
                PromiseState::Fulfilled => resolve_promise_value(realm, &result, data.value),
                PromiseState::Rejected => reject_promise_value(realm, &result, data.value),
                PromiseState::Pending => {}
            }
        } else {
            resolve_promise_value(realm, &result, first);
        }
    } else {
        if let ObjectKind::Promise(data) = &mut result.borrow_mut().kind {
            data.state = PromiseState::Pending;
        }
    }
    Ok(JsValue::Object(result))
}

fn is_promise(value: &JsValue) -> bool {
    matches!(
        value.as_object().map(|object| object.to_ref()),
        Some(object) if matches!(object.borrow().kind, ObjectKind::Promise(_))
    )
}

fn resolve_promise_value(realm: &mut Realm, promise: &ObjRef, value: JsValue) {
    if let ObjectKind::Promise(data) = &mut promise.borrow_mut().kind {
        if is_promise(&value)
            && let Ok(inner) = state(realm, &value)
        {
            data.state = inner.state;
            data.value = inner.value;
            return;
        }
        data.state = PromiseState::Fulfilled;
        data.value = value;
    }
}

fn reject_promise_value(_realm: &mut Realm, promise: &ObjRef, value: JsValue) {
    if let ObjectKind::Promise(data) = &mut promise.borrow_mut().kind {
        data.state = PromiseState::Rejected;
        data.value = value;
    }
}
