//! Date built-in. All conversions run on the `time` crate with UTC offsets,
//! mirroring QuickJS' `js_date_*` (quickjs.c) helpers built on MakeDay /
//! MakeTime / MakeDay computations (ES2022 20.4.1.11+) and TimeClip.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::object::{ObjectData, ObjectKind};
use crate::value::JsResult;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const DAY_MS: f64 = 86_400_000.0;
/// TimeClip bound (ES2022 20.4.1.19).
const MAX_TIME: f64 = 8.64e15;

// QuickJS installs these with `JS_AddIntrinsicDate` (quickjs.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.date_proto.clone();
    let constructor = install_constructor(realm, "Date", 7, constructor, &prototype);
    let target = constructor
        .as_object()
        .expect("constructor must be an object")
        .to_ref();
    define_method(realm, &target, "now", 0, now);
    define_method(realm, &target, "parse", 1, parse);
    define_method(realm, &target, "UTC", 7, utc);
    define_method(realm, &prototype, "getTime", 0, get_time);
    define_method(realm, &prototype, "valueOf", 0, get_time);
    define_method(realm, &prototype, "toISOString", 0, to_iso_string);
    define_method(realm, &prototype, "toJSON", 1, to_iso_string);
    define_method(realm, &prototype, "toString", 0, to_date_string);
    define_method(realm, &prototype, "toDateString", 0, to_date_string_part);
    define_method(realm, &prototype, "toTimeString", 0, to_time_string_part);
    define_method(realm, &prototype, "toUTCString", 0, to_utc_string);
    define_method(realm, &prototype, "toGMTString", 0, to_utc_string);
    define_method(realm, &prototype, "getFullYear", 0, get_full_year);
    define_method(realm, &prototype, "getMonth", 0, get_month);
    define_method(realm, &prototype, "getDate", 0, get_date);
    define_method(realm, &prototype, "getDay", 0, get_day);
    define_method(realm, &prototype, "getHours", 0, get_hours);
    define_method(realm, &prototype, "getMinutes", 0, get_minutes);
    define_method(realm, &prototype, "getSeconds", 0, get_seconds);
    define_method(realm, &prototype, "getMilliseconds", 0, get_milliseconds);
    define_method(
        realm,
        &prototype,
        "getTimezoneOffset",
        0,
        get_timezone_offset,
    );
    define_method(realm, &prototype, "setTime", 1, set_time);
    define_method(realm, &prototype, "setFullYear", 3, set_full_year);
    define_method(realm, &prototype, "setMonth", 2, set_month);
    define_method(realm, &prototype, "setDate", 1, set_date);
    define_method(realm, &prototype, "setHours", 4, set_hours);
    define_method(realm, &prototype, "setMinutes", 3, set_minutes);
    define_method(realm, &prototype, "setSeconds", 3, set_seconds);
    define_method(realm, &prototype, "setMilliseconds", 1, set_milliseconds);
    define_method(realm, &prototype, "getUTCFullYear", 0, get_utc_full_year);
    define_method(realm, &prototype, "getUTCMonth", 0, get_utc_month);
    define_method(realm, &prototype, "getUTCDate", 0, get_utc_date);
    define_method(realm, &prototype, "getUTCDay", 0, get_utc_day);
    define_method(realm, &prototype, "getUTCHours", 0, get_utc_hours);
    define_method(realm, &prototype, "getUTCMinutes", 0, get_utc_minutes);
    define_method(realm, &prototype, "getUTCSeconds", 0, get_utc_seconds);
    define_method(realm, &prototype, "getUTCMilliseconds", 0, get_utc_milliseconds);
    define_method(realm, &prototype, "setUTCFullYear", 3, set_utc_full_year);
    define_method(realm, &prototype, "setUTCMonth", 2, set_utc_month);
    define_method(realm, &prototype, "setUTCDate", 1, set_utc_date);
    define_method(realm, &prototype, "setUTCHours", 4, set_utc_hours);
    define_method(realm, &prototype, "setUTCMinutes", 3, set_utc_minutes);
    define_method(realm, &prototype, "setUTCSeconds", 3, set_utc_seconds);
    define_method(
        realm,
        &prototype,
        "setUTCMilliseconds",
        1,
        set_utc_milliseconds,
    );
}

fn constructor(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    if !this.is_object() {
        return Ok(JsValue::string(now_string()));
    }
    let value = if arguments.is_empty() {
        now_value()
    } else if arguments.len() == 1 {
        match arg(arguments, 0) {
            JsValue::String(text) => parse_date(&text),
            value => time_clip(crate::vm::to_number(realm, value)?),
        }
    } else {
        let year = arg_number(realm, arguments, 0)?;
        let month = arg_number_or(realm, arguments, 1, 0.0)?;
        let date = arg_number_or(realm, arguments, 2, 1.0)?;
        let hours = arg_number_or(realm, arguments, 3, 0.0)?;
        let minutes = arg_number_or(realm, arguments, 4, 0.0)?;
        let seconds = arg_number_or(realm, arguments, 5, 0.0)?;
        let milliseconds = arg_number_or(realm, arguments, 6, 0.0)?;
        make_date(year, month, date, hours, minutes, seconds, milliseconds)
    };
    let object = ObjectData::date(value, Some(realm.date_proto.clone()));
    Ok(JsValue::Object(Rc::new(RefCell::new(object))))
}

fn now_value() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as f64)
        .unwrap_or(0.0)
}

fn now(_realm: &mut Realm, _this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(now_value()))
}

fn date_value(realm: &Realm, this: &JsValue) -> JsResult<f64> {
    match this {
        JsValue::Object(object) => match &object.borrow().kind {
            ObjectKind::Date(value) => Ok(*value),
            _ => Err(realm.type_error("Date method called on incompatible receiver")),
        },
        _ => Err(realm.type_error("Date method called on incompatible receiver")),
    }
}

fn get_time(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(date_value(realm, this)?))
}

fn set_time(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = time_clip(arg_number(realm, arguments, 0)?);
    let object = this
        .as_object()
        .ok_or_else(|| realm.type_error("Date method called on incompatible receiver"))?;
    if let ObjectKind::Date(date) = &mut object.to_ref().borrow_mut().kind {
        *date = value;
    }
    Ok(JsValue::number(value))
}

fn to_iso_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = date_value(realm, this)?;
    if !value.is_finite() {
        return Err(realm.range_error("Invalid time value"));
    }
    let (year, month, day, time) = utc_fields(value);
    let milliseconds = value.rem_euclid(1000.0);
    Ok(JsValue::string(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year,
        month + 1,
        day,
        time / 3_600_000,
        (time / 60_000) % 60,
        (time / 1000) % 60,
        milliseconds as i64
    )))
}

fn to_date_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = date_value(realm, this)?;
    if !value.is_finite() {
        return Ok(JsValue::string("Invalid Date"));
    }
    let (year, month, day, time) = utc_fields(value);
    let weekday = weekday_of(value);
    Ok(JsValue::string(format!(
        "{} {} {:02} {:04} {:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)",
        WEEKDAYS[weekday],
        MONTHS[month as usize],
        day,
        year,
        time / 3_600_000,
        (time / 60_000) % 60,
        (time / 1000) % 60
    )))
}

fn to_date_string_part(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    let value = date_value(realm, this)?;
    if !value.is_finite() {
        return Ok(JsValue::string("Invalid Date"));
    }
    let (year, month, day, _) = utc_fields(value);
    let weekday = weekday_of(value);
    Ok(JsValue::string(format!(
        "{} {} {:02} {:04}",
        WEEKDAYS[weekday],
        MONTHS[month as usize],
        day,
        year
    )))
}

fn to_time_string_part(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    let value = date_value(realm, this)?;
    if !value.is_finite() {
        return Ok(JsValue::string("Invalid Date"));
    }
    let (_, _, _, time) = utc_fields(value);
    Ok(JsValue::string(format!(
        "{:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)",
        time / 3_600_000,
        (time / 60_000) % 60,
        (time / 1000) % 60
    )))
}

fn to_utc_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = date_value(realm, this)?;
    if !value.is_finite() {
        return Ok(JsValue::string("Invalid Date"));
    }
    let (year, month, day, time) = utc_fields(value);
    let weekday = weekday_of(value);
    Ok(JsValue::string(format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[weekday],
        day,
        MONTHS[month as usize],
        year,
        time / 3_600_000,
        (time / 60_000) % 60,
        (time / 1000) % 60
    )))
}

fn now_string() -> String {
    let value = now_value();
    let (year, month, day, time) = utc_fields(value);
    format!(
        "{} {} {:02} {:04} {:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)",
        WEEKDAYS[weekday_of(value)],
        MONTHS[month as usize],
        day,
        year,
        time / 3_600_000,
        (time / 60_000) % 60,
        (time / 1000) % 60
    )
}

/// (year, month 0-based, day, milliseconds since midnight) of a UTC day.
fn utc_fields(value: f64) -> (i64, i64, i64, i64) {
    let days = (value.div_euclid(DAY_MS)) as i64;
    let time = value.rem_euclid(DAY_MS) as i64;
    let (year, month, day) = civil_from_days(days);
    (year, month, day, time)
}

fn weekday_of(value: f64) -> usize {
    let days = value.div_euclid(DAY_MS) as i64;
    // 1970-01-01 was a Thursday (index 4).
    (days.rem_euclid(7) + 4) as usize % 7
}

/// Days since the epoch for a (year, month, day), like `MakeDay`
/// (ES2022 20.4.1.12). This is Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - if month <= 1 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    // `month` is 0-based; March opens the shifted year.
    let mp = if month <= 1 { month + 10 } else { month - 2 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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

/// `TimeClip` (ES2022 20.4.1.19): NaN outside the 8.64e15 range.
fn time_clip(value: f64) -> f64 {
    if value.is_finite() && value.abs() <= MAX_TIME {
        value.trunc()
    } else {
        f64::NAN
    }
}

/// `MakeTime` + `MakeDay` + epoch computation with full overflow roll-over
/// (ES2022 20.4.1.11/12/13), unlike a clamped `Time::from_hms`.
fn make_date(
    year: f64,
    month: f64,
    date: f64,
    hours: f64,
    minutes: f64,
    seconds: f64,
    milliseconds: f64,
) -> f64 {
    if ![year, month, date, hours, minutes, seconds, milliseconds]
        .iter()
        .all(|value| value.is_finite())
    {
        return f64::NAN;
    }
    let year = year.trunc();
    // 0..=99 maps to 1900..=1999.
    let year = if (0.0..=99.0).contains(&year) {
        year + 1900.0
    } else {
        year
    };
    let day = days_from_civil(year as i64, month.trunc() as i64, date.trunc() as i64);
    let time = ((hours.trunc() * 60.0 + minutes.trunc()) * 60.0 + seconds.trunc()) * 1000.0
        + milliseconds.trunc();
    time_clip(day as f64 * DAY_MS + time)
}

/// A single getter/setter over one UTC field, avoiding per-field duplicates
/// like QuickJS' `js_date_getter`/`js_date_setter` helpers.
#[derive(Clone, Copy)]
enum Field {
    FullYear,
    Month,
    Date,
    Hours,
    Minutes,
    Seconds,
    Milliseconds,
    Day,
}

fn field_value(value: f64, field: Field) -> f64 {
    if !value.is_finite() {
        return f64::NAN;
    }
    let (year, month, day, time) = utc_fields(value);
    match field {
        Field::FullYear => year as f64,
        Field::Month => month as f64,
        Field::Date => day as f64,
        Field::Hours => (time / 3_600_000) as f64,
        Field::Minutes => ((time / 60_000) % 60) as f64,
        Field::Seconds => ((time / 1000) % 60) as f64,
        Field::Milliseconds => (value.rem_euclid(1000.0)) as f64,
        Field::Day => weekday_of(value) as f64,
    }
}

fn getter(field: Field) -> impl Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue> {
    move |realm, this, _arguments| {
        let value = date_value(realm, this)?;
        Ok(JsValue::number(field_value(value, field)))
    }
}
fn setter(field: Field) -> impl Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue> {
    move |realm, this, arguments| {
        let original = date_value(realm, this)?;
        let (mut year, mut month, mut date, mut hours, mut minutes, mut seconds, millis) =
            if original.is_finite() {
                let (year, month, day, time) = utc_fields(original);
                (
                    year as f64,
                    month as f64,
                    day as f64,
                    (time / 3_600_000) as f64,
                    ((time / 60_000) % 60) as f64,
                    ((time / 1000) % 60) as f64,
                    original.rem_euclid(1000.0),
                )
            } else if matches!(field, Field::FullYear) {
                // setFullYear on an invalid date resets the other fields.
                (f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0)
            } else {
                return Ok(JsValue::number(f64::NAN));
            };
        if matches!(field, Field::FullYear) {
            year = arg_number(realm, arguments, 0)?;
            if arguments.len() > 1 {
                month = arg_number(realm, arguments, 1)?;
            }
            if arguments.len() > 2 {
                date = arg_number(realm, arguments, 2)?;
            }
        } else if matches!(field, Field::Month) {
            month = arg_number(realm, arguments, 0)?;
            if arguments.len() > 1 {
                date = arg_number(realm, arguments, 1)?;
            }
        } else if matches!(field, Field::Date) {
            date = arg_number(realm, arguments, 0)?;
        } else if matches!(field, Field::Hours) {
            hours = arg_number(realm, arguments, 0)?;
            if arguments.len() > 1 {
                minutes = arg_number(realm, arguments, 1)?;
            }
            if arguments.len() > 2 {
                seconds = arg_number(realm, arguments, 2)?;
            }
            if arguments.len() > 3 {
                return set_millis(realm, this, arguments, 3, original);
            }
        } else if matches!(field, Field::Minutes) {
            minutes = arg_number(realm, arguments, 0)?;
            if arguments.len() > 1 {
                seconds = arg_number(realm, arguments, 1)?;
            }
            if arguments.len() > 2 {
                return set_millis(realm, this, arguments, 2, original);
            }
        } else if matches!(field, Field::Seconds) {
            seconds = arg_number(realm, arguments, 0)?;
            if arguments.len() > 1 {
                return set_millis(realm, this, arguments, 1, original);
            }
        }
        let milliseconds = match field {
            Field::Milliseconds => arg_number(realm, arguments, 0)?,
            _ => millis,
        };
        let value = make_date(year, month, date, hours, minutes, seconds, milliseconds);
        let object = this
            .as_object()
            .ok_or_else(|| realm.type_error("Date method called on incompatible receiver"))?;
        if let ObjectKind::Date(date_value) = &mut object.to_ref().borrow_mut().kind {
            *date_value = value;
        }
        Ok(JsValue::number(value))
    }
}

/// Trailing milliseconds argument of the multi-argument setters.
fn set_millis(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
    index: usize,
    original: f64,
) -> JsResult<JsValue> {
    let milliseconds = arg_number(realm, arguments, index)?;
    let object = this
        .as_object()
        .ok_or_else(|| realm.type_error("Date method called on incompatible receiver"))?;
    let value = if original.is_finite() {
        time_clip(original - original.rem_euclid(1000.0) + milliseconds)
    } else {
        f64::NAN
    };
    if let ObjectKind::Date(date_value) = &mut object.to_ref().borrow_mut().kind {
        *date_value = value;
    }
    Ok(JsValue::number(value))
}

macro_rules! date_fns {
    ($( $name:ident = $field:ident ),* $(,)?) => {
        $(
            fn $name(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
                getter(Field::$field)(realm, this, arguments)
            }
        )*
    };
}

date_fns! {
    get_full_year = FullYear,
    get_month = Month,
    get_date = Date,
    get_hours = Hours,
    get_minutes = Minutes,
    get_seconds = Seconds,
    get_milliseconds = Milliseconds,
    get_day = Day,
    get_utc_full_year = FullYear,
    get_utc_month = Month,
    get_utc_date = Date,
    get_utc_hours = Hours,
    get_utc_minutes = Minutes,
    get_utc_seconds = Seconds,
    get_utc_milliseconds = Milliseconds,
    get_utc_day = Day,
}

macro_rules! date_setters {
    ($( $name:ident = $field:ident ),* $(,)?) => {
        $(
            fn $name(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
                setter(Field::$field)(realm, this, arguments)
            }
        )*
    };
}

date_setters! {
    set_full_year = FullYear,
    set_month = Month,
    set_date = Date,
    set_hours = Hours,
    set_minutes = Minutes,
    set_seconds = Seconds,
    set_milliseconds = Milliseconds,
    set_utc_full_year = FullYear,
    set_utc_month = Month,
    set_utc_date = Date,
    set_utc_hours = Hours,
    set_utc_minutes = Minutes,
    set_utc_seconds = Seconds,
    set_utc_milliseconds = Milliseconds,
}

fn get_timezone_offset(_realm: &mut Realm, _this: &JsValue, _a: &[JsValue]) -> JsResult<JsValue> {
    // The engine keeps UTC semantics, so the offset is always zero.
    Ok(JsValue::number(0.0))
}

fn parse(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_string(realm, arg(arguments, 0))?;
    Ok(JsValue::number(parse_date(&value)))
}

/// ISO-8601 date and RFC 3339 parsing, with a `NaN` fallback for anything
/// else (spec-defined behavior for unrecognized formats).
fn parse_date(value: &str) -> f64 {
    let value = crate::coerce::trim_ecma_whitespace(value);
    let Some((date_part, time_part)) = value.split_once(['T', 't']) else {
        return parse_date_only(value)
            .map(|day| day * DAY_MS)
            .unwrap_or(f64::NAN);
    };
    let date_part = parse_date_only(date_part);
    let time_part = parse_time(time_part);
    match (date_part, time_part) {
        (Some(day), Some(time)) => time_clip(day * DAY_MS + time),
        _ => f64::NAN,
    }
}

fn parse_date_only(value: &str) -> Option<f64> {
    let negative = value.starts_with('-');
    let rest = if negative || value.starts_with('+') {
        &value[1..]
    } else {
        value
    };
    let mut parts = rest.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    if !(rest.len() >= 8) && year.abs() > 275_760 {
        return None;
    }
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month - 1, day) as f64 * if negative { -1.0 } else { 1.0 })
}

fn parse_time(value: &str) -> Option<f64> {
    let value = value.trim_end_matches('Z').trim_end_matches('z');
    let mut parts = value.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds = match parts.next() {
        Some(seconds) => seconds
            .split('.')
            .next()
            .and_then(|whole| whole.parse::<f64>().ok())?,
        None => 0.0,
    };
    let milliseconds = match value.split_once('.') {
        Some((_, fraction)) => {
            let digits = fraction
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            if digits.is_empty() {
                0.0
            } else {
                let digits = &digits[..digits.len().min(3)];
                format!("{digits:0<3}").parse::<f64>().unwrap_or(0.0)
            }
        }
        None => 0.0,
    };
    if !(0.0..=23.0).contains(&hours)
        || !(0.0..=59.0).contains(&minutes)
        || !(0.0..=59.0).contains(&seconds)
    {
        return None;
    }
    Some((hours * 3_600.0 + minutes * 60.0 + seconds) * 1000.0 + milliseconds)
}

fn utc(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let year = arg_number(realm, arguments, 0)?;
    let month = arg_number_or(realm, arguments, 1, 0.0)?;
    let day = arg_number_or(realm, arguments, 2, 1.0)?;
    let hours = arg_number_or(realm, arguments, 3, 0.0)?;
    let minutes = arg_number_or(realm, arguments, 4, 0.0)?;
    let seconds = arg_number_or(realm, arguments, 5, 0.0)?;
    let milliseconds = arg_number_or(realm, arguments, 6, 0.0)?;
    Ok(JsValue::number(make_date(
        year, month, day, hours, minutes, seconds, milliseconds,
    )))
}
