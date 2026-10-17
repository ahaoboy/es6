use super::*;
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicError / js_error_constructor (quickjs.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.error_proto.clone();
    install_constructor(realm, "Error", 1, error, &prototype);
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
        let function = realm.native_function(name, 1, error);
        if let Some(object) = function.as_object() {
            object.set("prototype", JsValue::Object(prototype.clone()));
        }
        define_value(&prototype, "name", JsValue::string(name));
        define_value(&prototype, "message", JsValue::string(""));
        define_value(&realm.global, name, function);
    }
}

fn error(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let message = if arg(arguments, 0).is_undefined() {
        String::new()
    } else {
        crate::vm::to_string(realm, arg(arguments, 0))?
    };
    let name = this
        .as_object()
        .and_then(|object| object.to_ref().borrow().prototype.clone())
        .and_then(|prototype| {
            prototype
                .borrow()
                .get_own_value(&crate::object::PropKey::from("name"))
        })
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| std::rc::Rc::from("Error"));
    Ok(realm.error_object(&name, &message))
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
