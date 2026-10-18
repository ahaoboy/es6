//! Error construction, stack traces and exception display.
//!
//! Error objects mirror the `JS_ThrowError2` chain (quickjs.c:8444) and
//! `build_backtrace` (quickjs.c:8189).

use std::rc::Rc;

use super::CALL_STACK;
use crate::object::{Property, PropKey};
use crate::runtime::Realm;
use crate::value::JsValue;

pub fn type_error(realm: &Realm, message: impl Into<String>) -> JsValue {
    realm.type_error(message)
}

pub fn range_error(realm: &Realm, message: impl Into<String>) -> JsValue {
    realm.range_error(message)
}

pub(crate) fn stack_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("ES6_TRACE_STACK").is_some())
}

pub(crate) fn push_call_stack(name: Rc<str>) {
    CALL_STACK.with(|stack| stack.borrow_mut().push(name));
}

pub(crate) fn pop_call_stack() {
    CALL_STACK.with(|stack| {
        stack.borrow_mut().pop();
    });
}

/// Attach a `stack` property to a thrown error object, like
/// `build_backtrace` (quickjs.c:8189).
pub(crate) fn record_error_stack(exception: &JsValue) {
    if !stack_trace_enabled() {
        return;
    }
    let JsValue::Object(object) = exception else {
        return;
    };
    if object.borrow().has_own(&PropKey::from("stack")) {
        return;
    }
    let stack = CALL_STACK.with(|frames| {
        let frames = frames.borrow();
        let mut text = String::from("Error stack:\n");
        for name in frames.iter().rev() {
            text.push_str("  at ");
            text.push_str(name);
            text.push('\n');
        }
        text
    });
    object
        .borrow_mut()
        .define("stack", Property::hidden(JsValue::string(stack)));
}

pub fn display_error(_realm: &Realm, value: JsValue) -> String {
    display_error_value(&value)
}

fn display_error_value(value: &JsValue) -> String {
    match value {
        JsValue::String(text) => text.to_string(),
        JsValue::Object(object) => {
            let data = object.borrow();
            if data.is_error() {
                let name = data
                    .get_own_value(&PropKey::from("name"))
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| Rc::from("Error"));
                let message = data
                    .get_own_value(&PropKey::from("message"))
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| Rc::from(""));
                if message.is_empty() {
                    name.to_string()
                } else {
                    format!("{name}: {message}")
                }
            } else {
                "[object Object]".into()
            }
        }
        other => format!("{other:?}"),
    }
}
