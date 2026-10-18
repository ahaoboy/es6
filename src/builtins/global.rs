use super::*;
use crate::value::{Env, JsValue};

// QuickJS: JS_AddIntrinsicBaseObjects / js_global_* (quickjs.c).
pub fn install(realm: &mut Realm) {
    let global = realm.global.clone();
    define_value(&global, "undefined", JsValue::Undefined);
    define_value(&global, "NaN", JsValue::number(f64::NAN));
    define_value(&global, "Infinity", JsValue::number(f64::INFINITY));
    define_value(&global, "globalThis", JsValue::Object(global.clone()));

    let number = realm.global_get("Number");
    if let Some(object) = number.as_object() {
        let parse_int = object.get("parseInt");
        let parse_float = object.get("parseFloat");
        define_value(&global, "parseInt", parse_int);
        define_value(&global, "parseFloat", parse_float);
    }

    define_method(realm, &global, "isNaN", 1, is_nan);
    define_method(realm, &global, "isFinite", 1, is_finite);
    define_method(realm, &global, "eval", 1, eval);
    define_method(realm, &global, "print", 0, print);
    define_method(
        realm,
        &global,
        "encodeURIComponent",
        1,
        encode_uri_component,
    );
    define_method(
        realm,
        &global,
        "decodeURIComponent",
        1,
        decode_uri_component,
    );
    define_method(realm, &global, "encodeURI", 1, encode_uri);
    define_method(realm, &global, "decodeURI", 1, decode_uri);
}

fn is_nan(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_number(realm, arg(arguments, 0))?;
    let _ = realm;
    Ok(JsValue::Bool(value.is_nan()))
}

fn is_finite(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_number(realm, arg(arguments, 0))?;
    let _ = realm;
    Ok(JsValue::Bool(value.is_finite()))
}

fn eval(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let source = arg(arguments, 0);
    let JsValue::String(source) = source else {
        return Ok(source);
    };
    let program =
        crate::parser::parse(&source).map_err(|error| realm.syntax_error(error.to_string()))?;
    let definition = crate::compiler::compile_program(&program)
        .map_err(|error| realm.syntax_error(error.to_string()))?;
    let env = Env::new(definition.env_size as usize, None);
    crate::vm::execute(
        realm,
        definition,
        env,
        JsValue::Object(realm.global.clone()),
    )
}

fn print(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let mut parts = Vec::new();
    for argument in arguments {
        parts.push(crate::vm::to_string(realm, argument.clone())?);
    }
    println!("{}", parts.join(" "));
    Ok(JsValue::Undefined)
}

fn encode_uri_component(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let value = crate::vm::to_string(realm, arg(arguments, 0))?;
    Ok(JsValue::string(percent_encode(&value, false)))
}

fn encode_uri(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_string(realm, arg(arguments, 0))?;
    Ok(JsValue::string(percent_encode(&value, true)))
}

fn decode_uri_component(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let value = crate::vm::to_string(realm, arg(arguments, 0))?;
    percent_decode(&value).ok_or_else(|| realm.uri_error("URI malformed"))
}

fn decode_uri(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    decode_uri_component(realm, _this, arguments)
}

fn percent_encode(value: &str, keep_uri_chars: bool) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        let allowed = byte.is_ascii_alphanumeric()
            || "-_.!~*'()".contains(byte as char)
            || (keep_uri_chars && ";/?:@&=+$,#".contains(byte as char));
        if allowed {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

fn percent_decode(value: &str) -> Option<JsValue> {
    let bytes = value.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            result.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            result.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(result).ok().map(JsValue::string)
}

impl From<&str> for JsValue {
    fn from(value: &str) -> Self {
        JsValue::string(value)
    }
}
