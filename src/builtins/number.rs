use super::*;
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicNumber / js_number_proto_* (quickjs.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.number_proto.clone();
    let constructor = install_constructor(realm, "Number", 1, number, &prototype);
    let target = constructor.as_object().unwrap().to_ref();
    define_value(&target, "MAX_VALUE", JsValue::number(f64::MAX));
    define_value(&target, "MIN_VALUE", JsValue::number(f64::MIN_POSITIVE));
    define_value(&target, "NaN", JsValue::number(f64::NAN));
    define_value(&target, "POSITIVE_INFINITY", JsValue::number(f64::INFINITY));
    define_value(
        &target,
        "NEGATIVE_INFINITY",
        JsValue::number(f64::NEG_INFINITY),
    );
    define_value(
        &target,
        "MAX_SAFE_INTEGER",
        JsValue::number(9_007_199_254_740_991.0),
    );
    define_value(
        &target,
        "MIN_SAFE_INTEGER",
        JsValue::number(-9_007_199_254_740_991.0),
    );
    define_value(&target, "EPSILON", JsValue::number(f64::EPSILON));
    define_method(realm, &target, "isNaN", 1, is_nan);
    define_method(realm, &target, "isFinite", 1, is_finite);
    define_method(realm, &target, "isInteger", 1, is_integer);
    define_method(realm, &target, "isSafeInteger", 1, is_safe_integer);
    define_method(realm, &target, "parseFloat", 1, parse_float);
    define_method(realm, &target, "parseInt", 2, parse_int);

    define_method(realm, &prototype, "toString", 1, to_string);
    define_method(realm, &prototype, "toLocaleString", 0, to_string_locale);
    define_method(realm, &prototype, "valueOf", 0, value_of);
    define_method(realm, &prototype, "toFixed", 1, to_fixed);
    define_method(realm, &prototype, "toExponential", 1, to_exponential);
    define_method(realm, &prototype, "toPrecision", 1, to_precision);
}

fn number(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = if arguments.is_empty() {
        0.0
    } else {
        crate::vm::to_number(arg(arguments, 0))?
    };
    if this.is_object() && !this.is_function() {
        return Ok(JsValue::Object(realm.boxed_number(value)));
    }
    Ok(JsValue::number(value))
}

fn is_nan(_realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Bool(
        arg(arguments, 0).as_number().is_some_and(f64::is_nan),
    ))
}

fn is_finite(_realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Bool(
        arg(arguments, 0).as_number().is_some_and(f64::is_finite),
    ))
}

fn is_integer(_realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Bool(arg(arguments, 0).as_number().is_some_and(
        |value| value.is_finite() && value.fract() == 0.0,
    )))
}

fn is_safe_integer(
    _realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    Ok(JsValue::Bool(arg(arguments, 0).as_number().is_some_and(
        |value| value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0,
    )))
}

fn parse_float(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_string(realm, arg(arguments, 0))?;
    let value = value.trim_start();
    let mut end = 0;
    let bytes = value.as_bytes();
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    let mut digits = false;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
        digits = true;
    }
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
            digits = true;
        }
    }
    if digits && end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let exponent_start = end;
        end += 1;
        if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
            end += 1;
        }
        let mut exponent_digits = false;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
            exponent_digits = true;
        }
        if !exponent_digits {
            end = exponent_start;
        }
    }
    Ok(JsValue::number(if digits {
        value[..end].parse().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }))
}

fn parse_int(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_string(realm, arg(arguments, 0))?;
    let mut value = value.trim_start();
    let mut sign = 1.0;
    if let Some(rest) = value.strip_prefix('-') {
        sign = -1.0;
        value = rest;
    } else if let Some(rest) = value.strip_prefix('+') {
        value = rest;
    }
    let mut radix = if arguments.len() > 1 {
        crate::vm::to_uint32(arg_number(arguments, 1)?) as u32
    } else {
        0
    };
    if radix != 0 && !(2..=36).contains(&radix) {
        return Ok(JsValue::number(f64::NAN));
    }
    if radix == 0 {
        if let Some(rest) = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
        {
            radix = 16;
            value = rest;
        } else {
            radix = 10;
        }
    } else if radix == 16
        && let Some(rest) = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
    {
        value = rest;
    }
    let mut end = 0;
    for (index, ch) in value.char_indices() {
        if ch.is_digit(radix) {
            end = index + ch.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        return Ok(JsValue::number(f64::NAN));
    }
    let digits = &value[..end];
    let parsed = u64::from_str_radix(digits, radix)
        .map(|value| value as f64)
        .unwrap_or(f64::NAN);
    Ok(JsValue::number(parsed * sign))
}

fn to_string(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_number(realm, this)?;
    let radix = if arguments.is_empty() || arg(arguments, 0).is_undefined() {
        10
    } else {
        crate::vm::to_uint32(arg_number(arguments, 0)?) as u32
    };
    if !(2..=36).contains(&radix) {
        return Err(realm.range_error("toString() radix must be between 2 and 36"));
    }
    if radix == 10 {
        return Ok(JsValue::string(crate::vm::format_number(value)));
    }
    if !value.is_finite() {
        return Ok(JsValue::string(crate::vm::format_number(value)));
    }
    let negative = value < 0.0;
    let mut value = value.abs().trunc() as u64;
    let mut digits = Vec::new();
    if value == 0 {
        digits.push('0');
    }
    while value > 0 {
        let digit = (value % radix as u64) as u32;
        digits.push(char::from_digit(digit, radix).unwrap_or('0'));
        value /= radix as u64;
    }
    digits.reverse();
    let mut result: String = digits.into_iter().collect();
    if negative {
        result.insert(0, '-');
    }
    Ok(JsValue::string(result))
}

fn to_string_locale(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    let value = this_number(realm, this)?;
    Ok(JsValue::string(crate::vm::format_number(value)))
}

fn value_of(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(this_number(realm, this)?))
}

fn to_fixed(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_number(realm, this)?;
    let digits = to_integer(arg_number(arguments, 0).unwrap_or(0.0)).clamp(0, 100) as usize;
    Ok(JsValue::string(format!("{value:.digits$}")))
}

fn to_exponential(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_number(realm, this)?;
    if value.is_nan() {
        return Ok(JsValue::string("NaN"));
    }
    if arg(arguments, 0).is_undefined() {
        return Ok(JsValue::string(format!("{value:e}")));
    }
    let digits = to_integer(arg_number(arguments, 0)?).clamp(0, 100) as usize;
    Ok(JsValue::string(format!("{value:.digits$e}")))
}

fn to_precision(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_number(realm, this)?;
    if arg(arguments, 0).is_undefined() {
        return Ok(JsValue::string(crate::vm::format_number(value)));
    }
    let precision = to_integer(arg_number(arguments, 0)?).clamp(1, 100) as usize;
    Ok(JsValue::string(format!("{value:.precision$}")))
}
