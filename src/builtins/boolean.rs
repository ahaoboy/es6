use super::*;
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicBoolean (quickjs.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.boolean_proto.clone();
    install_constructor(realm, "Boolean", 1, boolean, &prototype);
    define_method(realm, &prototype, "toString", 0, to_string);
    define_method(realm, &prototype, "valueOf", 0, value_of);
}

fn boolean(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_boolean(&arg(arguments, 0));
    if this.is_object() && !this.is_function() {
        return Ok(JsValue::Object(realm.boxed_bool(value)));
    }
    Ok(JsValue::Bool(value))
}

fn to_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(this_boolean(realm, this)?.to_string()))
}

fn value_of(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Bool(this_boolean(realm, this)?))
}
