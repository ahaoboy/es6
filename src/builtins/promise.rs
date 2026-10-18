use super::*;
use crate::object::PromiseState;
use crate::promise as core;
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicPromise / js_promise_* (quickjs.c). The state and
// job machinery lives in `crate::promise`, mirroring `JSPromiseData` and
// `JSContext.job_list`.
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
    let promise = realm.promise();
    let resolve = realm.native_function("", 1, {
        let promise = promise.clone();
        move |realm, _this, arguments| {
            if matches!(core::state(&promise), Some((PromiseState::Pending, _))) {
                core::resolve(realm, &promise, arg(arguments, 0))?;
            }
            Ok(JsValue::Undefined)
        }
    });
    let reject = realm.native_function("", 1, {
        let promise = promise.clone();
        move |realm, _this, arguments| {
            if matches!(core::state(&promise), Some((PromiseState::Pending, _))) {
                core::reject(realm, &promise, arg(arguments, 0))?;
            }
            Ok(JsValue::Undefined)
        }
    });
    if let Err(error) = super::call(realm, executor, JsValue::Undefined, &[resolve, reject]) {
        crate::vm::clear_error_offset();
        core::reject(realm, &promise, error)?;
    }
    Ok(JsValue::Object(promise))
}

fn resolve(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let promise = core::resolve_value(realm, arg(arguments, 0))?;
    Ok(JsValue::Object(promise))
}

fn reject(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let promise = realm.promise();
    core::reject(realm, &promise, arg(arguments, 0))?;
    Ok(JsValue::Object(promise))
}

fn then(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    core::then(realm, this, arg(arguments, 0), arg(arguments, 1))
}

fn catch_method(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    core::then(realm, this, JsValue::Undefined, arg(arguments, 0))
}

fn finally(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    core::finally(realm, this, arg(arguments, 0))
}

fn all(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    core::all(realm, arg(arguments, 0))
}

fn race(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    core::race(realm, arg(arguments, 0))
}
