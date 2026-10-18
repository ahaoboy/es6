use super::*;
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicError / js_error_constructor (quickjs.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.error_proto.clone();
    install_constructor(realm, "Error", 1, error_constructor, &prototype);
    define_method(realm, &prototype, "toString", 0, to_string);
    define_value(&prototype, "name", JsValue::string("Error"));
    define_value(&prototype, "message", JsValue::string(""));

    for name in [
        "TypeError",
        "RangeError",
        "ReferenceError",
        "SyntaxError",
        "EvalError",
        "URIError",
    ] {
        let prototype = realm.error_prototype(name);
        // Per-type constructor: `TypeError("x")` builds a TypeError whether
        // called as a function or with `new` (ES2022 20.5.5.1).
        let function = realm.native_function(name, 1, move |realm, _this, arguments| {
            error_native(realm, name, arguments)
        });
        if let Some(object) = function.as_object() {
            object.set("prototype", JsValue::Object(prototype.clone()));
        }
        define_value(&prototype, "name", JsValue::string(name));
        define_value(&prototype, "message", JsValue::string(""));
        define_value(&realm.global, name, function);
    }
}

fn error_native(realm: &mut Realm, name: &str, arguments: &[JsValue]) -> JsResult<JsValue> {
    let message = if arg(arguments, 0).is_undefined() {
        String::new()
    } else {
        crate::vm::to_string(realm, arg(arguments, 0))?
    };
    Ok(realm.error_object(name, &message))
}

/// `Error(msg[, options])`: sets `message` and an optional `cause`
/// (ES2022 20.5.1.1).
fn error_constructor(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    if this.is_object() {
        let message = if arg(arguments, 0).is_undefined() {
            None
        } else {
            Some(crate::vm::to_string(realm, arg(arguments, 0))?)
        };
        if let JsValue::Object(object) = this {
            // `new Error()` builds an ordinary receiver; mark it so thrown
            // values are recognized and displayed like native errors.
            object.borrow_mut().kind = ObjectKind::Error;
            if let Some(message) = message {
                object.borrow_mut().define(
                    "message",
                    crate::object::Property::data(JsValue::string(message)),
                );
            }
            if let JsValue::Object(options) = arg(arguments, 1) {
                let cause = get(realm, &JsValue::Object(options.clone()), "cause")?;
                if !cause.is_undefined() {
                    object
                        .borrow_mut()
                        .define("cause", crate::object::Property::data(cause));
                }
            }
        }
        return Ok(this.clone());
    }
    error_native(realm, "Error", arguments)
}

fn to_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = this
        .as_object()
        .ok_or_else(|| realm.type_error("Not an Error"))?;
    let name = get(realm, this, "name")?;
    let message = get(realm, this, "message")?;
    let name = if name.is_undefined() {
        "Error".into()
    } else {
        crate::vm::to_string(realm, name)?
    };
    let message = if message.is_undefined() {
        String::new()
    } else {
        crate::vm::to_string(realm, message)?
    };
    let _ = object;
    Ok(JsValue::string(if message.is_empty() {
        name
    } else {
        format!("{name}: {message}")
    }))
}
