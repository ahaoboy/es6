use std::rc::Rc;

use crate::object::{ObjectKind, PropKey};
use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};
use crate::vm::{call_value, get_value_property};
pub fn to_boolean(value: &JsValue) -> bool {
    match value {
        JsValue::Undefined | JsValue::Null => false,
        JsValue::Bool(value) => *value,
        JsValue::Int(value) => *value != 0,
        JsValue::Float(value) => *value != 0.0 && !value.is_nan(),
        JsValue::String(value) => !value.is_empty(),
        JsValue::Symbol(_) => true,
        JsValue::Object(_) => true,
    }
}

/// ECMAScript `ToNumeric` for arithmetic operators: objects are first
/// converted with `ToPrimitive` (hint number). QuickJS performs the same
/// two-stage conversion in `JS_ToNumberHintFree`/`js_unary_arith_slow`.
#[inline(always)]
pub fn to_numeric(realm: &mut Realm, value: JsValue) -> JsResult<f64> {
    if let JsValue::Int(value) = value {
        return Ok(value as f64);
    }
    if let JsValue::Float(value) = value {
        return Ok(value);
    }
    let value = to_primitive(realm, value)?;
    to_number(value)
}

#[inline(always)]
pub fn to_number(value: JsValue) -> JsResult<f64> {
    Ok(match value {
        JsValue::Undefined => f64::NAN,
        JsValue::Null => 0.0,
        JsValue::Bool(value) => {
            if value {
                1.0
            } else {
                0.0
            }
        }
        JsValue::Int(value) => value as f64,
        JsValue::Float(value) => value,
        JsValue::String(value) => string_to_number(&value),
        JsValue::Symbol(_) => {
            return Err(JsValue::string(
                "TypeError: cannot convert a Symbol value to a number",
            ));
        }
        JsValue::Object(_) => {
            return Err(JsValue::string(
                "TypeError: cannot convert object to number",
            ));
        }
    })
}

fn string_to_number(value: &str) -> f64 {
    let value = value.trim();
    if value.is_empty() {
        return 0.0;
    }
    if value == "Infinity" || value == "+Infinity" {
        return f64::INFINITY;
    }
    if value == "-Infinity" {
        return f64::NEG_INFINITY;
    }
    if let Some(rest) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return u64::from_str_radix(rest, 16)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    if let Some(rest) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        return u64::from_str_radix(rest, 2)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    if let Some(rest) = value
        .strip_prefix("0o")
        .or_else(|| value.strip_prefix("0O"))
    {
        return u64::from_str_radix(rest, 8)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    value.parse().unwrap_or(f64::NAN)
}

pub fn to_string(realm: &mut Realm, value: JsValue) -> JsResult<String> {
    match value {
        JsValue::Undefined => Ok("undefined".into()),
        JsValue::Null => Ok("null".into()),
        JsValue::Bool(value) => Ok(if value { "true" } else { "false" }.into()),
        JsValue::Int(value) => Ok(value.to_string()),
        JsValue::Float(value) => Ok(format_number(value)),
        JsValue::String(value) => Ok(value.to_string()),
        JsValue::Symbol(id) => {
            let description = realm.symbol_description(id);
            Ok(if description.is_empty() {
                "Symbol()".into()
            } else {
                format!("Symbol({description})")
            })
        }
        JsValue::Object(object) => {
            let data = object.borrow();
            match &data.kind {
                ObjectKind::Array(array) => {
                    let items = array.items.clone();
                    let length = array.length;
                    drop(data);
                    let mut parts = Vec::with_capacity(length);
                    for index in 0..length {
                        let item = items.get(index).cloned().unwrap_or(JsValue::Undefined);
                        if item.is_nullish() {
                            parts.push(String::new());
                        } else {
                            parts.push(to_string(realm, item)?);
                        }
                    }
                    Ok(parts.join(","))
                }
                ObjectKind::BoxedString(value) => Ok(value.to_string()),
                ObjectKind::BoxedNumber(value) => Ok(format_number(*value)),
                ObjectKind::BoxedBool(value) => Ok(value.to_string()),
                ObjectKind::TypedArray(array) => {
                    let values = array.values();
                    drop(data);
                    let mut parts = Vec::with_capacity(values.len());
                    for value in values {
                        parts.push(to_string(realm, value)?);
                    }
                    Ok(parts.join(","))
                }
                ObjectKind::ArrayBuffer(_) => Ok("[object ArrayBuffer]".into()),
                ObjectKind::DataView(_) => Ok("[object DataView]".into()),
                ObjectKind::Function(function) => {
                    Ok(format!("function {}() {{ [native code] }}", function.name))
                }
                ObjectKind::Error => {
                    let name = data
                        .get_own_value(&PropKey::from("name"))
                        .and_then(|value| value.as_string())
                        .unwrap_or_else(|| Rc::from("Error"));
                    let message = data
                        .get_own_value(&PropKey::from("message"))
                        .map(|value| value.as_string().unwrap_or_else(|| Rc::from("")))
                        .unwrap_or_else(|| Rc::from(""));
                    if message.is_empty() {
                        Ok(name.to_string())
                    } else {
                        Ok(format!("{name}: {message}"))
                    }
                }
                ObjectKind::RegExp(regexp) => Ok(format!("/{}/{}", regexp.source, regexp.flags)),
                ObjectKind::Date(value) => Ok(format_date(*value)),
                _ => Ok("[object Object]".into()),
            }
        }
    }
}

fn format_date(value: f64) -> String {
    if !value.is_finite() {
        return "Invalid Date".into();
    }
    let seconds = (value / 1000.0).floor() as i64;
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        year,
        month + 1,
        day,
        time / 3600,
        (time / 60) % 60,
        time % 60
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + if m <= 2 { 1 } else { 0 }, m - 1, d)
}

pub fn format_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == f64::INFINITY {
        return "Infinity".into();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    if value.fract() == 0.0 && value.abs() < 1e21 {
        return format!("{}", value as i64);
    }
    let mut buffer = ryu::Buffer::new();
    buffer.format(value).to_owned()
}

pub fn to_property_key(realm: &mut Realm, value: JsValue) -> JsResult<PropKey> {
    match value {
        JsValue::String(value) => Ok(crate::object::key_from_string(value)),
        JsValue::Int(value) => Ok(PropKey::Index(value.max(0) as usize)),
        JsValue::Float(value) if value.fract() == 0.0 && value >= 0.0 => {
            Ok(PropKey::Index(value as usize))
        }
        JsValue::Float(value) => Ok(PropKey::Str(Rc::from(format_number(value)))),
        JsValue::Undefined => Ok(PropKey::from("undefined")),
        JsValue::Null => Ok(PropKey::from("null")),
        JsValue::Bool(value) => Ok(PropKey::from(if value { "true" } else { "false" })),
        JsValue::Symbol(id) => Ok(PropKey::Symbol(id)),
        JsValue::Object(_) => Ok(PropKey::Str(Rc::from(to_string(realm, value)?))),
    }
}

pub fn add_values(realm: &mut Realm, left: JsValue, right: JsValue) -> JsResult<JsValue> {
    if let (Some(left), Some(right)) = (left.as_number(), right.as_number()) {
        return Ok(JsValue::number(left + right));
    }
    let left = to_primitive(realm, left)?;
    let right = to_primitive(realm, right)?;
    if matches!(left, JsValue::String(_)) || matches!(right, JsValue::String(_)) {
        let left = to_string(realm, left)?;
        let right = to_string(realm, right)?;
        return Ok(JsValue::string(left + &right));
    }
    Ok(JsValue::number(
        to_numeric(realm, left)? + to_numeric(realm, right)?,
    ))
}

pub fn to_primitive(realm: &mut Realm, value: JsValue) -> JsResult<JsValue> {
    match value {
        JsValue::Object(object) => {
            {
                let data = object.borrow();
                match &data.kind {
                    ObjectKind::BoxedString(value) => return Ok(JsValue::String(value.clone())),
                    ObjectKind::BoxedNumber(value) => return Ok(JsValue::number(*value)),
                    ObjectKind::BoxedBool(value) => return Ok(JsValue::Bool(*value)),
                    ObjectKind::Array(_) => {}
                    ObjectKind::Error => {}
                    _ => {}
                }
            }
            let value_object = JsValue::Object(object);
            let value_of = get_value_property(realm, &value_object, &PropKey::from("valueOf"))?;
            if value_of.is_function() {
                let result = call_value(realm, value_of, value_object.clone(), &[])?;
                if !result.is_object() {
                    return Ok(result);
                }
            }
            let to_string_fn =
                get_value_property(realm, &value_object, &PropKey::from("toString"))?;
            if to_string_fn.is_function() {
                let result = call_value(realm, to_string_fn, value_object.clone(), &[])?;
                if !result.is_object() {
                    return Ok(result);
                }
            }
            Err(realm.type_error("Cannot convert object to primitive value"))
        }
        value => Ok(value),
    }
}

pub fn same_value(left: &JsValue, right: &JsValue) -> bool {
    if strict_equals(left, right) {
        if matches!(left, JsValue::Int(_) | JsValue::Float(_)) {
            let a = left.as_number().unwrap();
            let b = right.as_number().unwrap();
            if a == 0.0 && b == 0.0 {
                return a.is_sign_negative() == b.is_sign_negative();
            }
        }
        return true;
    }
    matches!(
        (left, right),
        (JsValue::Int(_) | JsValue::Float(_), JsValue::Int(_) | JsValue::Float(_))
            if left.as_number().is_some_and(f64::is_nan)
                && right.as_number().is_some_and(f64::is_nan)
    )
}

pub fn same_value_zero(left: &JsValue, right: &JsValue) -> bool {
    if strict_equals(left, right) {
        return true;
    }
    matches!(
        (left, right),
        (JsValue::Int(_) | JsValue::Float(_), JsValue::Int(_) | JsValue::Float(_))
            if left.as_number().is_some_and(f64::is_nan)
                && right.as_number().is_some_and(f64::is_nan)
    )
}

pub fn strict_equals(left: &JsValue, right: &JsValue) -> bool {
    match (left, right) {
        (JsValue::Undefined, JsValue::Undefined) | (JsValue::Null, JsValue::Null) => true,
        (JsValue::Bool(a), JsValue::Bool(b)) => a == b,
        (JsValue::Int(a), JsValue::Int(b)) => a == b,
        (JsValue::Int(a), JsValue::Float(b)) | (JsValue::Float(b), JsValue::Int(a)) => {
            (*a as f64) == *b
        }
        (JsValue::Float(a), JsValue::Float(b)) => a == b,
        (JsValue::String(a), JsValue::String(b)) => a == b,
        (JsValue::Symbol(a), JsValue::Symbol(b)) => a == b,
        (JsValue::Object(a), JsValue::Object(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

pub fn loose_equals(realm: &mut Realm, left: &JsValue, right: &JsValue) -> JsResult<bool> {
    if strict_equals(left, right) {
        return Ok(true);
    }
    match (left, right) {
        (JsValue::Null | JsValue::Undefined, JsValue::Null | JsValue::Undefined) => Ok(true),
        (JsValue::Int(_) | JsValue::Float(_), JsValue::String(_)) => {
            Ok(to_number(left.clone())? == to_number(right.clone())?)
        }
        (JsValue::String(_), JsValue::Int(_) | JsValue::Float(_)) => {
            Ok(to_number(left.clone())? == to_number(right.clone())?)
        }
        (JsValue::Bool(_), _) => {
            loose_equals(realm, &JsValue::number(to_number(left.clone())?), right)
        }
        (_, JsValue::Bool(_)) => {
            loose_equals(realm, left, &JsValue::number(to_number(right.clone())?))
        }
        (JsValue::Object(_), JsValue::Int(_) | JsValue::Float(_) | JsValue::String(_)) => {
            let left = to_primitive(realm, left.clone())?;
            loose_equals(realm, &left, right)
        }
        (JsValue::Int(_) | JsValue::Float(_) | JsValue::String(_), JsValue::Object(_)) => {
            let right = to_primitive(realm, right.clone())?;
            loose_equals(realm, left, &right)
        }
        _ => Ok(false),
    }
}

pub(crate) fn abstract_compare(
    realm: &mut Realm,
    left: &JsValue,
    right: &JsValue,
) -> JsResult<Option<i32>> {
    if let (Some(left), Some(right)) = (left.as_number(), right.as_number()) {
        return Ok(if left < right {
            Some(-1)
        } else if left > right {
            Some(1)
        } else if left == right {
            Some(0)
        } else {
            None
        });
    }
    let left = to_primitive(realm, left.clone())?;
    let right = to_primitive(realm, right.clone())?;
    if let (JsValue::String(left), JsValue::String(right)) = (&left, &right) {
        return Ok(Some(match left.cmp(right) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }));
    }
    let left = to_number(left)?;
    let right = to_number(right)?;
    if left.is_nan() || right.is_nan() {
        Ok(None)
    } else if left < right {
        Ok(Some(-1))
    } else if left > right {
        Ok(Some(1))
    } else {
        Ok(Some(0))
    }
}

pub(crate) fn instance_of(
    realm: &mut Realm,
    value: &JsValue,
    constructor: &JsValue,
) -> JsResult<bool> {
    let Some(constructor_object) = constructor.as_object() else {
        return Err(realm.type_error("Right-hand side of instanceof is not callable"));
    };
    let prototype = get_value_property(realm, constructor, &PropKey::from("prototype"))?;
    let JsValue::Object(prototype) = prototype else {
        return Err(realm.type_error("Function has non-object prototype in instanceof"));
    };
    let mut current = match value {
        JsValue::Object(value) => value.borrow().prototype.clone(),
        _ => return Ok(false),
    };
    while let Some(object) = current {
        if Rc::ptr_eq(&object, &prototype) {
            return Ok(true);
        }
        current = object.borrow().prototype.clone();
    }
    let _ = constructor_object;
    Ok(false)
}

pub fn to_int32(value: f64) -> i32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    let value = value.trunc();
    let modulo = value.rem_euclid(4_294_967_296.0);
    if modulo >= 2_147_483_648.0 {
        (modulo - 4_294_967_296.0) as i32
    } else {
        modulo as i32
    }
}

pub fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    value.trunc().rem_euclid(4_294_967_296.0) as u32
}
