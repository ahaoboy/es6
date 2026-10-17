use super::*;
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicMath / js_math_* (quickjs.c).
pub fn install(realm: &mut Realm) {
    let math = realm.object();
    define_value(&math, "E", JsValue::number(std::f64::consts::E));
    define_value(&math, "LN10", JsValue::number(std::f64::consts::LN_10));
    define_value(&math, "LN2", JsValue::number(std::f64::consts::LN_2));
    define_value(&math, "LOG10E", JsValue::number(std::f64::consts::LOG10_E));
    define_value(&math, "LOG2E", JsValue::number(std::f64::consts::LOG2_E));
    define_value(&math, "PI", JsValue::number(std::f64::consts::PI));
    define_value(
        &math,
        "SQRT1_2",
        JsValue::number(std::f64::consts::FRAC_1_SQRT_2),
    );
    define_value(&math, "SQRT2", JsValue::number(std::f64::consts::SQRT_2));
    define_method(realm, &math, "abs", 1, abs);
    define_method(realm, &math, "acos", 1, acos);
    define_method(realm, &math, "acosh", 1, acosh);
    define_method(realm, &math, "asin", 1, asin);
    define_method(realm, &math, "asinh", 1, asinh);
    define_method(realm, &math, "atan", 1, atan);
    define_method(realm, &math, "atan2", 2, atan2);
    define_method(realm, &math, "atanh", 1, atanh);
    define_method(realm, &math, "cbrt", 1, cbrt);
    define_method(realm, &math, "ceil", 1, ceil);
    define_method(realm, &math, "clz32", 1, clz32);
    define_method(realm, &math, "cos", 1, cos);
    define_method(realm, &math, "cosh", 1, cosh);
    define_method(realm, &math, "exp", 1, exp);
    define_method(realm, &math, "expm1", 1, expm1);
    define_method(realm, &math, "floor", 1, floor);
    define_method(realm, &math, "fround", 1, fround);
    define_method(realm, &math, "hypot", 2, hypot);
    define_method(realm, &math, "imul", 2, imul);
    define_method(realm, &math, "log", 1, log);
    define_method(realm, &math, "log10", 1, log10);
    define_method(realm, &math, "log1p", 1, log1p);
    define_method(realm, &math, "log2", 1, log2);
    define_method(realm, &math, "max", 2, max);
    define_method(realm, &math, "min", 2, min);
    define_method(realm, &math, "pow", 2, pow);
    define_method(realm, &math, "random", 0, random);
    define_method(realm, &math, "round", 1, round);
    define_method(realm, &math, "sign", 1, sign);
    define_method(realm, &math, "sin", 1, sin);
    define_method(realm, &math, "sinh", 1, sinh);
    define_method(realm, &math, "sqrt", 1, sqrt);
    define_method(realm, &math, "tan", 1, tan);
    define_method(realm, &math, "tanh", 1, tanh);
    define_method(realm, &math, "trunc", 1, trunc);
    define_builtin_object(realm, "Math", &math);
}

fn unary(arguments: &[JsValue], operation: impl Fn(f64) -> f64) -> JsResult<JsValue> {
    Ok(JsValue::number(operation(arg_number(arguments, 0)?)))
}

fn binary(arguments: &[JsValue], operation: impl Fn(f64, f64) -> f64) -> JsResult<JsValue> {
    Ok(JsValue::number(operation(
        arg_number(arguments, 0)?,
        arg_number(arguments, 1)?,
    )))
}

fn abs(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::abs)
}
fn acos(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::acos)
}
fn acosh(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::acosh)
}
fn asin(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::asin)
}
fn asinh(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::asinh)
}
fn atan(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::atan)
}
fn atan2(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    binary(a, f64::atan2)
}
fn atanh(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::atanh)
}
fn cbrt(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::cbrt)
}
fn ceil(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::ceil)
}
fn clz32(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(
        crate::vm::to_uint32(arg_number(a, 0)?).leading_zeros() as f64,
    ))
}
fn cos(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::cos)
}
fn cosh(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::cosh)
}
fn exp(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::exp)
}
fn expm1(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::exp_m1)
}
fn floor(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::floor)
}
fn fround(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, |value| value as f32 as f64)
}
fn hypot(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    let mut sum = 0.0;
    for value in a {
        let number = crate::vm::to_number(value.clone())?;
        sum += number * number;
    }
    Ok(JsValue::number(sum.sqrt()))
}
fn imul(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    let left = crate::vm::to_int32(arg_number(a, 0)?);
    let right = crate::vm::to_int32(arg_number(a, 1)?);
    Ok(JsValue::Int(left.wrapping_mul(right)))
}
fn log(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::ln)
}
fn log10(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::log10)
}
fn log1p(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::ln_1p)
}
fn log2(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::log2)
}
fn max(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    let mut result = f64::NEG_INFINITY;
    for value in a {
        result = result.max(crate::vm::to_number(value.clone())?);
    }
    Ok(JsValue::number(result))
}
fn min(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    let mut result = f64::INFINITY;
    for value in a {
        result = result.min(crate::vm::to_number(value.clone())?);
    }
    Ok(JsValue::number(result))
}
fn pow(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    binary(a, f64::powf)
}
fn random(_r: &mut Realm, _t: &JsValue, _a: &[JsValue]) -> JsResult<JsValue> {
    use std::cell::Cell;
    thread_local! { static SEED: Cell<u64> = const { Cell::new(0x9E37_79B9_7F4A_7C15) }; }
    let value = SEED.with(|seed| {
        let mut state = seed.get();
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        seed.set(state);
        (state >> 11) as f64 / (1u64 << 53) as f64
    });
    Ok(JsValue::number(value))
}
fn round(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number((arg_number(a, 0)? + 0.5).floor()))
}
fn sign(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    let value = arg_number(a, 0)?;
    Ok(JsValue::number(if value.is_nan() || value == 0.0 {
        value
    } else if value > 0.0 {
        1.0
    } else {
        -1.0
    }))
}
fn sin(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::sin)
}
fn sinh(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::sinh)
}
fn sqrt(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::sqrt)
}
fn tan(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::tan)
}
fn tanh(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::tanh)
}
fn trunc(_r: &mut Realm, _t: &JsValue, a: &[JsValue]) -> JsResult<JsValue> {
    unary(a, f64::trunc)
}
