use super::*;
use crate::value::JsValue;

// QuickJS: standalone console implementation (quickjs-libc.c, js_console_*).
pub fn install(realm: &mut Realm) {
    let console = realm.object();
    define_method(realm, &console, "log", 0, log);
    define_method(realm, &console, "info", 0, log);
    define_method(realm, &console, "debug", 0, log);
    define_method(realm, &console, "warn", 0, warn);
    define_method(realm, &console, "error", 0, error);
    define_method(realm, &console, "dir", 1, log);
    define_method(realm, &console, "trace", 0, warn);
    define_method(realm, &console, "assert", 1, assert);
    define_builtin_object(realm, "console", &console);
}

fn format(realm: &mut Realm, values: &[JsValue]) -> JsResult<String> {
    let mut parts = Vec::with_capacity(values.len());
    for value in values {
        parts.push(crate::vm::to_string(realm, value.clone())?);
    }
    Ok(parts.join(" "))
}

fn log(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    println!("{}", format(realm, arguments)?);
    Ok(JsValue::Undefined)
}

fn warn(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    eprintln!("{}", format(realm, arguments)?);
    Ok(JsValue::Undefined)
}

fn error(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    eprintln!("{}", format(realm, arguments)?);
    Ok(JsValue::Undefined)
}

fn assert(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    if !crate::vm::to_boolean(&arg(arguments, 0)) {
        eprintln!(
            "Assertion failed: {}",
            format(realm, arguments.get(1..).unwrap_or(&[]))?
        );
    }
    Ok(JsValue::Undefined)
}
