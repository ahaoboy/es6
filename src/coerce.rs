use std::rc::Rc;

use crate::object::{ObjectKind, PropKey};
use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};
use crate::vm::{call_value, get_value_property};

/// Maximum canonical array index, `2^32 - 2` (ES2022 6.1.7). Keys at or
/// above this are ordinary string properties, like QuickJS' `JS_NewAtomStr`
/// tagged-int atom boundary (quickjs.c:3697).
pub const MAX_ARRAY_INDEX: u64 = 4_294_967_294;

/// Dense arrays above this size grow through the sparse `props` map instead
/// of resizing the element vector, mirroring QuickJS' conversion of fast
/// arrays to slow arrays in `expand_fast_array` (quickjs.c:10452).
pub(crate) const DENSE_LIMIT: usize = 1 << 20;

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
    let value = to_primitive_hint(realm, value, Hint::Number)?;
    to_number(realm, value)
}

/// ECMAScript `ToNumber` (ES2022 7.1.4): objects go through `ToPrimitive`
/// with hint number, exactly like `JS_ToNumber` (quickjs.c:14650 area).
#[inline(always)]
pub fn to_number(realm: &mut Realm, value: JsValue) -> JsResult<f64> {
    let value = match value {
        JsValue::Object(_) => to_primitive_hint(realm, value, Hint::Number)?,
        value => value,
    };
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
            return Err(realm.type_error("Cannot convert a Symbol value to a number"));
        }
        JsValue::Object(_) => f64::NAN,
    })
}

/// ECMAScript `StringToNumber` (ES2022 7.1.4.1): strict decimal grammar,
/// `Infinity`, and `0x`/`0o`/`0b` radix forms. Accumulates radix digits as
/// doubles so arbitrary widths round like QuickJS' `js_atod` (dtoa.c:1353).
fn string_to_number(value: &str) -> f64 {
    let value = trim_ecma_whitespace(value);
    if value.is_empty() {
        return 0.0;
    }
    let (sign, rest) = match value.as_bytes()[0] {
        b'+' => (1.0, &value[1..]),
        b'-' => (-1.0, &value[1..]),
        _ => (1.0, value),
    };
    if rest == "Infinity" {
        return sign * f64::INFINITY;
    }
    if let Some(digits) = radix_digits(rest, "0x", 16) {
        return sign * radix_value(digits, 16);
    }
    if let Some(digits) = radix_digits(rest, "0o", 8) {
        return sign * radix_value(digits, 8);
    }
    if let Some(digits) = radix_digits(rest, "0b", 2) {
        return sign * radix_value(digits, 2);
    }
    if !is_decimal_literal(rest) {
        return f64::NAN;
    }
    rest.parse::<f64>().unwrap_or(f64::NAN) * sign
}

fn radix_digits<'a>(value: &'a str, prefix: &str, radix: u32) -> Option<&'a str> {
    let rest = value
        .strip_prefix(prefix)
        .or_else(|| value.strip_prefix(&prefix.to_uppercase()))?;
    if rest.is_empty() || !rest.chars().all(|ch| ch.is_digit(radix)) {
        return None;
    }
    Some(rest)
}

fn radix_value(digits: &str, radix: u32) -> f64 {
    digits.chars().fold(0.0, |value, ch| {
        value * f64::from(radix) + ch.to_digit(radix).unwrap() as f64
    })
}

/// The exact ES2022 decimal grammar accepted by `StringToNumber`.
fn is_decimal_literal(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    let mut digits = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
        digits += 1;
    }
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if index < bytes.len() && (bytes[index] == b'e' || bytes[index] == b'E') {
        index += 1;
        if index < bytes.len() && (bytes[index] == b'+' || bytes[index] == b'-') {
            index += 1;
        }
        let mut exponent_digits = 0;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
            exponent_digits += 1;
        }
        if exponent_digits == 0 {
            return false;
        }
    }
    index == bytes.len()
}

/// ECMAScript `TrimString` whitespace set (ES2022 7.1.2 + 6.2): only
/// WhiteSpace (0009 000B 000C 0020 00A0 FEFF) and LineTerminators
/// (000A 000D 2028 2029) are trimmed, unlike Rust's Unicode `trim`.
pub fn trim_ecma_whitespace(value: &str) -> &str {
    value.trim_matches(|ch: char| {
        matches!(
            ch,
            '\u{0009}'
                | '\u{000B}'
                | '\u{000C}'
                | '\u{0020}'
                | '\u{00A0}'
                | '\u{FEFF}'
                | '\u{000A}'
                | '\u{000D}'
                | '\u{2028}'
                | '\u{2029}'
        )
    })
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
            {
                let data = object.borrow();
                match &data.kind {
                    ObjectKind::Array(array) => {
                        let items = array.items.clone();
                        let length = array.length;
                        drop(data);
                        let mut parts = Vec::with_capacity(length);
                        for index in 0..length {
                            let item = items.get(index).cloned().flatten();
                            match item {
                                Some(item) if !item.is_nullish() => {
                                    parts.push(to_string(realm, item)?);
                                }
                                // Holes and nullish values stringify empty,
                                // like `Array.prototype.join`.
                                _ => parts.push(String::new()),
                            }
                        }
                        return Ok(parts.join(","));
                    }
                    ObjectKind::BoxedString(value) => return Ok(value.to_string()),
                    ObjectKind::BoxedNumber(value) => return Ok(format_number(*value)),
                    ObjectKind::BoxedBool(value) => return Ok(value.to_string()),
                    ObjectKind::TypedArray(array) => {
                        let values = array.values();
                        drop(data);
                        let mut parts = Vec::with_capacity(values.len());
                        for value in values {
                            parts.push(to_string(realm, value)?);
                        }
                        return Ok(parts.join(","));
                    }
                    ObjectKind::ArrayBuffer(_) => return Ok("[object ArrayBuffer]".into()),
                    ObjectKind::DataView(_) => return Ok("[object DataView]".into()),
                    ObjectKind::Function(function) => {
                        return Ok(format!("function {}() {{ [native code] }}", function.name));
                    }
                    ObjectKind::Error => {
                        let name = data
                            .get_own_value(&PropKey::from("name"))
                            .and_then(|value| value.as_string())
                            .unwrap_or_default();
                        let message = data
                            .get_own_value(&PropKey::from("message"))
                            .map(|value| value.as_string().unwrap_or_default())
                            .unwrap_or_default();
                        if message.is_empty() {
                            return Ok(name.to_string());
                        }
                        return Ok(format!("{name}: {message}"));
                    }
                    ObjectKind::RegExp(regexp) => {
                        return Ok(format!("/{}/{}", regexp.source, regexp.flags));
                    }
                    ObjectKind::Date(value) => return Ok(format_date(*value)),
                    _ => {}
                }
            }
            // Ordinary objects stringify through `ToPrimitive` with the
            // string hint, so user `toString`/`valueOf` methods run
            // (`JS_ToStringHintFree` in QuickJS).
            let primitive = to_primitive_hint(realm, JsValue::Object(object), Hint::String)?;
            match primitive {
                JsValue::Object(_) => Ok("[object Object]".into()),
                value => to_string(realm, value),
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
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year,
        month + 1,
        day,
        time / 3600,
        (time / 60) % 60,
        time % 60,
        ((value.rem_euclid(1000.0)) as i64)
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

/// ECMAScript `Number::toString` formatting (ES2022 6.1.6.1.20): integral
/// values print without a decimal point or exponent up to `1e21`, values
/// smaller than `1e-6` switch to exponential notation, exactly like
/// QuickJS `js_dtoa2` (quickjs.c:14579).
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
    let mut buffer = ryu::Buffer::new();
    let text = buffer.format(value);
    let Some((mantissa, exponent_text)) = text.split_once('e') else {
        // ryu's positional form always carries a `.0` on integers.
        return strip_trailing_zero_fraction(text);
    };
    let exponent: i32 = exponent_text.parse().unwrap_or(0);
    if exponent > 0 && value.fract() == 0.0 && value.abs() < 1e21 {
        if exponent <= 15 {
            // Exact in the i64 range: print directly.
            return format!("{}", value as i64);
        }
        return expand_exponent(mantissa, exponent);
    }
    if (-6..21).contains(&exponent) {
        return expand_exponent(mantissa, exponent);
    }
    // ECMAScript requires an explicit exponent sign: `1e+21`.
    if exponent >= 0 {
        format!("{mantissa}e+{exponent}")
    } else {
        format!("{mantissa}e{exponent}")
    }
}

fn strip_trailing_zero_fraction(text: &str) -> String {
    match text.strip_suffix(".0") {
        Some(stripped) => stripped.to_owned(),
        None => text.to_owned(),
    }
}

/// Expand `1.2345e18` / `1e-6` into positional notation.
fn expand_exponent(mantissa: &str, exponent: i32) -> String {
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(|ch| ch.is_ascii_digit()).collect();
    let point = mantissa
        .find('.')
        .map(|index| index - usize::from(negative));
    let fraction = point.map_or(0, |point| mantissa.len() - point - 1);
    let point = digits.len() as i32 - fraction as i32 + exponent;
    let mut result = String::new();
    if negative {
        result.push('-');
    }
    if point <= 0 {
        result.push_str("0.");
        result.push_str(&"0".repeat(point.unsigned_abs() as usize));
        result.push_str(&digits);
    } else if point as usize >= digits.len() {
        result.push_str(&digits);
        result.push_str(&"0".repeat(point as usize - digits.len()));
    } else {
        let point = point as usize;
        result.push_str(&digits[..point]);
        result.push('.');
        result.push_str(&digits[point..]);
    }
    result
}

/// ECMAScript `ToPropertyKey` (ES2022 7.1.17): negative and non-canonical
/// integers become string keys, like QuickJS' tagged-int atoms.
pub fn to_property_key(realm: &mut Realm, value: JsValue) -> JsResult<PropKey> {
    match value {
        JsValue::String(value) => Ok(crate::object::key_from_string(value)),
        JsValue::Int(value) => Ok(
            if (value as i64) >= 0 && (value as u64) <= MAX_ARRAY_INDEX {
                PropKey::Index(value as usize)
            } else {
                PropKey::from(value.to_string())
            },
        ),
        JsValue::Float(value)
            if value.fract() == 0.0 && value >= 0.0 && value <= MAX_ARRAY_INDEX as f64 =>
        {
            Ok(PropKey::Index(value as usize))
        }
        JsValue::Float(value) => Ok(PropKey::from(format_number(value))),
        JsValue::Undefined => Ok(PropKey::from("undefined")),
        JsValue::Null => Ok(PropKey::from("null")),
        JsValue::Bool(value) => Ok(PropKey::from(if value { "true" } else { "false" })),
        JsValue::Symbol(id) => Ok(PropKey::Symbol(id)),
        JsValue::Object(_) => Ok(PropKey::from(to_string(realm, value)?)),
    }
}

#[inline(never)]
pub fn add_values(realm: &mut Realm, left: JsValue, right: JsValue) -> JsResult<JsValue> {
    if let (Some(left), Some(right)) = (left.as_number(), right.as_number()) {
        return Ok(JsValue::number(left + right));
    }
    // String + string builds the result once instead of copying both
    // operands through `to_string` first.
    if let (JsValue::String(left), JsValue::String(right)) = (&left, &right) {
        let mut buffer = compact_str::CompactString::with_capacity(left.len() + right.len());
        buffer.push_str(left.as_str());
        buffer.push_str(right.as_str());
        return Ok(JsValue::String(crate::string::JsString::from(buffer)));
    }
    let left = to_primitive_hint(realm, left, Hint::Default)?;
    let right = to_primitive_hint(realm, right, Hint::Default)?;
    if matches!(left, JsValue::String(_)) || matches!(right, JsValue::String(_)) {
        let left = to_string(realm, left)?;
        let right = to_string(realm, right)?;
        let mut buffer = compact_str::CompactString::with_capacity(left.len() + right.len());
        buffer.push_str(&left);
        buffer.push_str(&right);
        return Ok(JsValue::String(crate::string::JsString::from(buffer)));
    }
    Ok(JsValue::number(
        to_numeric(realm, left)? + to_numeric(realm, right)?,
    ))
}

/// Preferred type for `ToPrimitive` (ES2022 7.1.1).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    Default,
    String,
    Number,
}

/// ECMAScript `ToPrimitive` (ES2022 7.1.1), mirroring the exotic-object
/// fast paths and method probing of `JS_ToPrimitive` in QuickJS
/// (`js_to_primitive`).
pub fn to_primitive_hint(realm: &mut Realm, value: JsValue, hint: Hint) -> JsResult<JsValue> {
    let JsValue::Object(object) = value else {
        return Ok(value);
    };
    {
        let data = object.borrow();
        match &data.kind {
            ObjectKind::BoxedString(value) => return Ok(JsValue::String(value.clone())),
            ObjectKind::BoxedNumber(value) => return Ok(JsValue::number(*value)),
            ObjectKind::BoxedBool(value) => return Ok(JsValue::Bool(*value)),
            // `Date[Symbol.toPrimitive]` maps default/string hints to the
            // string form, like QuickJS `js_date_toPrimitive`.
            ObjectKind::Date(value) => {
                let date = *value;
                drop(data);
                return Ok(match hint {
                    Hint::Number => JsValue::number(date),
                    _ => JsValue::string(format_date(date)),
                });
            }
            _ => {}
        }
    }
    let value_object = JsValue::Object(object);
    let (first, second) = match hint {
        Hint::String => ("toString", "valueOf"),
        Hint::Number | Hint::Default => ("valueOf", "toString"),
    };
    for name in [first, second] {
        let method = get_value_property(realm, &value_object, &PropKey::from(name))?;
        if method.is_function() {
            let result = call_value(realm, method, value_object.clone(), &[])?;
            if !result.is_object() {
                return Ok(result);
            }
        }
    }
    Err(realm.type_error("Cannot convert object to primitive value"))
}

pub fn to_primitive(realm: &mut Realm, value: JsValue) -> JsResult<JsValue> {
    to_primitive_hint(realm, value, Hint::Default)
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
            Ok(to_number(realm, left.clone())? == to_number(realm, right.clone())?)
        }
        (JsValue::String(_), JsValue::Int(_) | JsValue::Float(_)) => {
            Ok(to_number(realm, left.clone())? == to_number(realm, right.clone())?)
        }
        (JsValue::Bool(_), _) => {
            let left = JsValue::number(to_number(realm, left.clone())?);
            loose_equals(realm, &left, right)
        }
        (_, JsValue::Bool(_)) => {
            let right = JsValue::number(to_number(realm, right.clone())?);
            loose_equals(realm, left, &right)
        }
        (JsValue::Object(_), JsValue::Int(_) | JsValue::Float(_) | JsValue::String(_)) => {
            let left = to_primitive_hint(realm, left.clone(), Hint::Default)?;
            loose_equals(realm, &left, right)
        }
        (JsValue::Int(_) | JsValue::Float(_) | JsValue::String(_), JsValue::Object(_)) => {
            let right = to_primitive_hint(realm, right.clone(), Hint::Default)?;
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
    let left = to_primitive_hint(realm, left.clone(), Hint::Default)?;
    let right = to_primitive_hint(realm, right.clone(), Hint::Default)?;
    if let (JsValue::String(left), JsValue::String(right)) = (&left, &right) {
        return Ok(Some(match utf16_cmp(left, right) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }));
    }
    let left = to_number(realm, left)?;
    let right = to_number(realm, right)?;
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

/// ECMAScript string comparison uses UTF-16 code units, so astral characters
/// sort by their lead surrogate before `\uE000..`\uFFFF`.
pub fn utf16_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left = left.encode_utf16();
    let mut right = right.encode_utf16();
    loop {
        match (left.next(), right.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => match a.cmp(&b) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            },
        }
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

/// ECMAScript `Number::exponentiate` (ES2022 13.5.3), differing from C99
/// `pow`: `±1 ** ±Infinity` is NaN.
pub fn js_pow(base: f64, exponent: f64) -> f64 {
    if exponent.is_infinite() && base.abs() == 1.0 {
        return f64::NAN;
    }
    base.powf(exponent)
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

/// Convert a byte offset of a Rust string to its UTF-16 code-unit position.
/// ECMAScript string indices are code units, like QuickJS' wide strings.
pub fn byte_to_utf16_offset(text: &str, byte_offset: usize) -> usize {
    text.char_indices()
        .take_while(|(offset, _)| *offset < byte_offset)
        .count()
}

/// Convert a UTF-16 code-unit position to a byte offset. Surrogate halves
/// and out-of-range positions are clamped to the nearest boundary.
pub fn utf16_to_byte_offset(text: &str, unit_offset: usize) -> usize {
    let mut units = 0;
    for (offset, ch) in text.char_indices() {
        if units >= unit_offset {
            return offset;
        }
        units += ch.len_utf16();
    }
    text.len()
}

/// Total UTF-16 length of a Rust string.
pub fn utf16_length(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}
