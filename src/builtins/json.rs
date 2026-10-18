use super::*;
use crate::object::{ObjectKind, Property};
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicJSON / js_json_* (quickjs.c).
pub fn install(realm: &mut Realm) {
    let json = realm.object();
    define_method(realm, &json, "parse", 2, parse_json);
    define_method(realm, &json, "stringify", 3, stringify);
    define_builtin_object(realm, "JSON", &json);
}

fn parse_json(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let source = crate::vm::to_string(realm, arg(arguments, 0))?;
    let value: serde_json::Value = serde_json::from_str(&source)
        .map_err(|error| realm.syntax_error(format!("JSON.parse: {error}")))?;
    Ok(from_json(realm, &value))
}

fn from_json(realm: &mut Realm, value: &serde_json::Value) -> JsValue {
    match value {
        serde_json::Value::Null => JsValue::Null,
        serde_json::Value::Bool(value) => JsValue::Bool(*value),
        serde_json::Value::Number(value) => JsValue::number(value.as_f64().unwrap_or(f64::NAN)),
        serde_json::Value::String(value) => JsValue::string(value.clone()),
        serde_json::Value::Array(values) => {
            let items = values.iter().map(|value| from_json(realm, value)).collect();
            JsValue::Object(realm.array(items))
        }
        serde_json::Value::Object(values) => {
            let object = realm.object();
            for (key, value) in values {
                object
                    .borrow_mut()
                    .define(key.as_str(), Property::data(from_json(realm, value)));
            }
            JsValue::Object(object)
        }
    }
}

fn stringify(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = arg(arguments, 0);
    let mut output = String::new();
    if !to_json(realm, &value, &mut output)? {
        return Ok(JsValue::Undefined);
    }
    Ok(JsValue::string(output))
}

fn to_json(realm: &mut Realm, value: &JsValue, output: &mut String) -> JsResult<bool> {
    // `toJSON` protocol first, exactly like `js_json_check` in QuickJS.
    if let JsValue::Object(_) = value {
        let method =
            crate::vm::get_value_property(realm, value, &crate::object::PropKey::from("toJSON"))?;
        if method.is_function() {
            let result = crate::vm::call_value(realm, method, value.clone(), &[])?;
            return to_json(realm, &result, output);
        }
    }
    match value {
        // Top-level undefined is not serialized; array elements emit null.
        JsValue::Undefined => Ok(false),
        JsValue::Null => {
            output.push_str("null");
            Ok(true)
        }
        JsValue::Bool(value) => {
            output.push_str(if *value { "true" } else { "false" });
            Ok(true)
        }
        JsValue::Int(value) => {
            output.push_str(&value.to_string());
            Ok(true)
        }
        JsValue::Float(value) => {
            if value.is_finite() {
                output.push_str(&crate::vm::format_number(*value));
                Ok(true)
            } else {
                output.push_str("null");
                Ok(true)
            }
        }
        JsValue::String(value) => {
            let quoted = serde_json::to_string(value.as_ref()).unwrap_or_else(|_| "\"\"".into());
            output.push_str(&quoted);
            Ok(true)
        }
        // Functions and symbols are skipped in objects, `null` in arrays.
        JsValue::Symbol(_) => Ok(false),
        JsValue::Object(object) => {
            let data = object.borrow();
            match &data.kind {
                ObjectKind::Function(_) => Ok(false),
                ObjectKind::Array(array) => {
                    let items = array.items.clone();
                    let length = object.borrow().array_len().unwrap_or(items.len());
                    drop(data);
                    output.push('[');
                    let mut first = true;
                    // Holes and `undefined` elements serialize as null;
                    // the dense prefix plus recorded length cover all
                    // elements (ES2022 25.5.2.4).
                    for index in 0..length {
                        if !first {
                            output.push(',');
                        }
                        first = false;
                        match items.get(index).cloned().flatten() {
                            Some(item) => {
                                if !to_json(realm, &item, output)? {
                                    output.push_str("null");
                                }
                            }
                            None => output.push_str("null"),
                        }
                    }
                    output.push(']');
                    Ok(true)
                }
                ObjectKind::BoxedString(value) => {
                    let text = value.clone();
                    drop(data);
                    to_json(realm, &JsValue::String(text), output)
                }
                ObjectKind::BoxedNumber(value) => {
                    let number = *value;
                    drop(data);
                    to_json(realm, &JsValue::number(number), output)
                }
                ObjectKind::BoxedBool(value) => {
                    let boolean = *value;
                    drop(data);
                    to_json(realm, &JsValue::Bool(boolean), output)
                }
                _ => {
                    let keys = data.enumerable_keys();
                    drop(data);
                    output.push('{');
                    let mut first = true;
                    for key in keys {
                        let Some(key_text) = key.to_rc_string() else {
                            continue;
                        };
                        let item = crate::vm::get_value_property(realm, value, &key)?;
                        if item.is_undefined() || item.is_function() || item.is_symbol() {
                            continue;
                        }
                        if !first {
                            output.push(',');
                        }
                        first = false;
                        output.push_str(
                            &serde_json::to_string(key_text.as_ref())
                                .unwrap_or_else(|_| "\"\"".into()),
                        );
                        output.push(':');
                        let _ = to_json(realm, &item, output)?;
                    }
                    output.push('}');
                    Ok(true)
                }
            }
        }
    }
}
