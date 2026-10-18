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
        crate::vm::to_number(realm, arg(arguments, 0))?
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
    let value = crate::coerce::trim_ecma_whitespace(&value);
    // `parseFloat` accepts a leading `Infinity` (ES2022 19.2.5).
    for prefix in ["Infinity", "+Infinity", "-Infinity"] {
        if let Some(rest) = value.strip_prefix(prefix)
            && (rest.is_empty() || !rest.as_bytes()[0].is_ascii_digit() || rest.starts_with('.'))
        {
            return Ok(JsValue::number(match prefix {
                "-Infinity" => f64::NEG_INFINITY,
                _ => f64::INFINITY,
            }));
        }
    }
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
    let mut value = crate::coerce::trim_ecma_whitespace(&value);
    let mut sign = 1.0;
    if let Some(rest) = value.strip_prefix('-') {
        sign = -1.0;
        value = rest;
    } else if let Some(rest) = value.strip_prefix('+') {
        value = rest;
    }
    let mut radix = if arguments.len() > 1 {
        crate::vm::to_uint32(arg_number(realm, arguments, 1)?) as u32
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
        } else if let Some(rest) = value
            .strip_prefix("0o")
            .or_else(|| value.strip_prefix("0O"))
        {
            radix = 8;
            value = rest;
        } else if let Some(rest) = value
            .strip_prefix("0b")
            .or_else(|| value.strip_prefix("0B"))
        {
            radix = 2;
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
    // Accumulate exactly in u128 while it fits, so only one rounding
    // happens (correctly-rounded like every engine); longer inputs fall
    // back to double accumulation.
    let digits = &value[..end];
    let radix_u = u128::from(radix);
    let mut exact: u128 = 0;
    let mut base: u128 = 1;
    let mut overflow = false;
    for ch in digits.chars().rev() {
        let digit = u128::from(ch.to_digit(radix).unwrap_or(0));
        match digit
            .checked_mul(base)
            .and_then(|scaled| exact.checked_add(scaled))
        {
            Some(sum) => exact = sum,
            None => {
                overflow = true;
                break;
            }
        }
        match base.checked_mul(radix_u) {
            Some(next) => base = next,
            None => break,
        }
    }
    let parsed = if overflow {
        digits.chars().fold(0.0, |number, ch| {
            number * f64::from(radix) + f64::from(ch.to_digit(radix).unwrap_or(0))
        })
    } else {
        exact as f64
    };
    Ok(JsValue::number(parsed * sign))
}

fn to_string(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_number(realm, this)?;
    let radix = if arguments.is_empty() || arg(arguments, 0).is_undefined() {
        10
    } else {
        crate::vm::to_uint32(arg_number(realm, arguments, 0)?) as u32
    };
    if !(2..=36).contains(&radix) {
        return Err(realm.range_error("toString() radix must be between 2 and 36"));
    }
    if radix == 10 || !value.is_finite() {
        return Ok(JsValue::string(crate::vm::format_number(value)));
    }
    Ok(JsValue::string(format_radix(value, radix)))
}

/// `Number.prototype.toString(radix)` (ES2022 6.1.6.1.20): exact integer
/// part plus the fraction obtained by repeated radix multiplication, like
/// `js_dtoa` FREE format with an arbitrary radix (dtoa.c:1107).
fn format_radix(value: f64, radix: u32) -> String {
    let negative = value.is_sign_negative();
    let value = value.abs();
    let mut integer = value.trunc();
    let mut fraction = value.fract();
    // The integer part of every finite double fits in u128.
    let mut digits: Vec<u8> = Vec::new();
    while integer > 0.0 {
        let digit = (integer % f64::from(radix)).trunc() as u8;
        digits.push(digit);
        integer = (integer / f64::from(radix)).trunc();
        if integer * f64::from(radix) == integer && integer > 0.0 {
            break;
        }
    }
    if digits.is_empty() {
        digits.push(0);
    }
    digits.reverse();
    let mut result = String::new();
    if negative {
        result.push('-');
    }
    for digit in digits {
        result.push(char::from_digit(digit as u32, radix).unwrap_or('0'));
    }
    if fraction > 0.0 {
        result.push('.');
        // Emit fraction digits until the remainder is exhausted; doubles
        // need at most ~1100 binary digits.
        for _ in 0..1100 {
            fraction *= f64::from(radix);
            let digit = fraction.trunc() as u8;
            result.push(char::from_digit(digit as u32, radix).unwrap_or('0'));
            fraction -= digit as f64;
            if fraction <= 0.0 {
                break;
            }
        }
    }
    result
}

/// Decimal digits of a finite non-zero double from its shortest
/// round-trip representation: `(digits, point)` where the value is
/// `0.digits × 10^point`, with no non-significant leading zeros.
fn decimal_digits(value: f64) -> (String, i32) {
    let mut buffer = ryu::Buffer::new();
    let text = buffer.format(value.abs());
    let negative = text.starts_with('-');
    let text = if negative { &text[1..] } else { text };
    let (mantissa, exponent_text) = match text.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().unwrap_or(0)),
        None => (text, 0),
    };
    let mut digits: String = mantissa.chars().filter(|ch| ch.is_ascii_digit()).collect();
    let mut point = mantissa
        .find('.')
        .map_or(digits.len() as i32, |index| index as i32);
    point += exponent_text;
    // Drop non-significant leading zeros: `0.000123` must become
    // `("123", -3)`, not `("0000123", 1)`.
    let stripped = digits.len() - digits.trim_start_matches('0').len();
    if stripped > 0 {
        digits = digits[stripped..].to_owned();
        point -= stripped as i32;
    }
    if digits.is_empty() {
        return ("0".into(), 1);
    }
    (digits, point)
}

/// Round `0.digits × 10^point` to `keep` significant digits, half-up.
fn round_digits(digits: &str, point: i32, keep: usize) -> (String, i32) {
    let mut kept: String = digits.chars().take(keep).collect();
    let mut point = point;
    if digits.len() > keep {
        let round_up = digits.chars().nth(keep).is_some_and(|ch| ch > '4');
        if round_up {
            let mut overflow = true;
            for index in (0..kept.len()).rev() {
                let digit = kept.as_bytes()[index] - b'0' + 1;
                if digit < 10 {
                    kept.replace_range(index..index + 1, &((digit + b'0') as char).to_string());
                    overflow = false;
                    break;
                }
                kept.replace_range(index..index + 1, "0");
            }
            if overflow {
                kept.insert(0, '1');
                point += 1;
            }
        }
    }
    while kept.len() < keep {
        kept.push('0');
    }
    (kept, point)
}

/// Render `0.digits × 10^point` in positional notation with `fraction`
/// digits after the decimal point (digits must be pre-padded).
fn render_fixed(digits: &str, point: i32, negative: bool, fraction: usize) -> String {
    let mut result = String::new();
    if negative {
        result.push('-');
    }
    if point <= 0 {
        // `0.00…digits`: leading zeros after the decimal point.
        result.push_str("0.");
        result.push_str(&"0".repeat(point.unsigned_abs() as usize));
        let mut tail = digits.to_owned();
        while tail.len() < fraction {
            tail.push('0');
        }
        result.push_str(&tail[..fraction]);
    } else {
        let point = point as usize;
        if point >= digits.len() {
            result.push_str(digits);
            result.push_str(&"0".repeat(point - digits.len()));
            result.push('.');
        } else {
            result.push_str(&digits[..point]);
            result.push('.');
        }
        let mut tail: String = digits[point..].to_owned();
        while tail.len() < fraction {
            tail.push('0');
        }
        result.push_str(&tail[..fraction]);
    }
    if fraction == 0 {
        result.pop();
    }
    result
}

/// Render `0.digits × 10^point` in exponential notation with the decimal
/// point after the first digit (ES2022 6.1.6.1.20 `e` form).
fn render_exponential(digits: &str, point: i32, negative: bool, fraction: usize) -> String {
    let mut result = String::new();
    if negative {
        result.push('-');
    }
    result.push(digits.as_bytes()[0] as char);
    if fraction > 0 {
        result.push('.');
        let mut tail: String = digits[1..].to_owned();
        while tail.len() < fraction {
            tail.push('0');
        }
        result.push_str(&tail[..fraction]);
    }
    let exponent = point - 1;
    if exponent >= 0 {
        result.push_str(&format!("e+{exponent}"));
    } else {
        result.push_str(&format!("e{exponent}"));
    }
    result
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

/// `Number.prototype.toFixed` (ES2022 20.1.3.3): fixed notation with exact
/// half-up decimal rounding, like `js_dtoa2` with FRAC format
/// (quickjs.c:14579).
fn to_fixed(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_number(realm, this)?;
    let digits = to_integer(arg_number_or(realm, arguments, 0, 0.0)?);
    if !(0..=100).contains(&digits) {
        return Err(realm.range_error("toFixed() digits argument must be between 0 and 100"));
    }
    if value.is_nan() {
        return Ok(JsValue::string("NaN"));
    }
    if value.is_infinite() {
        return Ok(JsValue::string(crate::vm::format_number(value)));
    }
    let digits = digits as usize;
    if value.abs() >= 1e21 {
        return Ok(JsValue::string(crate::vm::format_number(value)));
    }
    let negative = value.is_sign_negative();
    let (raw, point) = decimal_digits(value);
    let keep = (point + digits as i32).max(1) as usize;
    let (rounded, point) = round_digits(&raw, point, keep);
    Ok(JsValue::string(render_fixed(
        &rounded, point, negative, digits,
    )))
}

/// `Number.prototype.toExponential` (ES2022 20.1.3.2).
fn to_exponential(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_number(realm, this)?;
    if value.is_nan() {
        return Ok(JsValue::string("NaN"));
    }
    if value.is_infinite() {
        return Ok(JsValue::string(crate::vm::format_number(value)));
    }
    if arg(arguments, 0).is_undefined() {
        let mut buffer = ryu::Buffer::new();
        let text = buffer.format(value);
        return Ok(JsValue::string(match text.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => {
                format!("{mantissa}e+{exponent}")
            }
            _ => text.to_owned(),
        }));
    }
    let digits = to_integer(arg_number(realm, arguments, 0)?);
    if !(0..=100).contains(&digits) {
        return Err(realm.range_error("toExponential() argument must be between 0 and 100"));
    }
    let digits = digits as usize;
    let negative = value.is_sign_negative();
    let (raw, point) = decimal_digits(value);
    let (rounded, point) = round_digits(&raw, point, digits + 1);
    Ok(JsValue::string(render_exponential(
        &rounded, point, negative, digits,
    )))
}

/// `Number.prototype.toPrecision` (ES2022 20.1.3.5): significant digits
/// with the fixed/exponential switch.
fn to_precision(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_number(realm, this)?;
    if arg(arguments, 0).is_undefined() {
        return Ok(JsValue::string(crate::vm::format_number(value)));
    }
    if value.is_nan() {
        return Ok(JsValue::string("NaN"));
    }
    if value.is_infinite() {
        return Ok(JsValue::string(crate::vm::format_number(value)));
    }
    let precision = to_integer(arg_number(realm, arguments, 0)?);
    if !(1..=100).contains(&precision) {
        return Err(realm.range_error("toPrecision() argument must be between 1 and 100"));
    }
    let precision = precision as usize;
    let negative = value.is_sign_negative();
    let (raw, point) = decimal_digits(value);
    let (rounded, point) = round_digits(&raw, point, precision);
    // Exponential when the exponent is outside the precision window
    // (ES2022 20.1.3.5 step 10).
    if point < -6 || point > precision as i32 {
        Ok(JsValue::string(render_exponential(
            &rounded,
            point,
            negative,
            precision - 1,
        )))
    } else {
        let fraction = precision.saturating_sub(point.max(0) as usize);
        Ok(JsValue::string(render_fixed(
            &rounded, point, negative, fraction,
        )))
    }
}
