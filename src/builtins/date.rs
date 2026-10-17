use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::object::{ObjectData, ObjectKind};
use crate::value::JsValue;
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

// QuickJS: JS_AddIntrinsicDate / js_date_* (quickjs.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.date_proto.clone();
    let constructor = install_constructor(realm, "Date", 7, constructor, &prototype);
    let target = constructor.as_object().unwrap().to_ref();
    define_method(realm, &target, "now", 0, now);
    define_method(realm, &target, "parse", 1, parse);
    define_method(realm, &target, "UTC", 7, utc);
    define_method(realm, &prototype, "getTime", 0, get_time);
    define_method(realm, &prototype, "valueOf", 0, get_time);
    define_method(realm, &prototype, "toISOString", 0, to_iso_string);
    define_method(realm, &prototype, "toJSON", 1, to_iso_string);
    define_method(realm, &prototype, "toString", 0, to_date_string);
    define_method(realm, &prototype, "toDateString", 0, to_date_string);
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
}

fn constructor(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    if !this.is_object() {
        return Ok(JsValue::string(format_date(now_value())));
    }
    let value = if arguments.is_empty() {
        now_value()
    } else if arguments.len() == 1 {
        match arg(arguments, 0) {
            JsValue::String(text) => parse_date(&text),
            value => crate::vm::to_number(value)?,
        }
    } else {
        let year = arg_number(arguments, 0)? as i32;
        let month = arg_number(arguments, 1)? as i32;
        let day = arg_number(arguments, 2).unwrap_or(1.0) as i32;
        let hour = arg_number(arguments, 3).unwrap_or(0.0) as i32;
        let minute = arg_number(arguments, 4).unwrap_or(0.0) as i32;
        let second = arg_number(arguments, 5).unwrap_or(0.0) as i32;
        let millisecond = arg_number(arguments, 6).unwrap_or(0.0) as i32;
        make_date(year, month, day, hour, minute, second, millisecond)
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

fn to_iso_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(format_date(date_value(realm, this)?)))
}

fn to_date_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(format_date(date_value(realm, this)?)))
}

fn datetime(realm: &Realm, this: &JsValue) -> JsResult<OffsetDateTime> {
    let value = date_value(realm, this)?;
    OffsetDateTime::from_unix_timestamp_nanos((value * 1_000_000.0) as i128)
        .map_err(|_| realm.range_error("Invalid Date"))
}

fn get_full_year(realm: &mut Realm, this: &JsValue, _a: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(datetime(realm, this)?.year() as f64))
}
fn get_month(realm: &mut Realm, this: &JsValue, _a: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(
        datetime(realm, this)?.month() as u8 as f64 - 1.0,
    ))
}
fn get_date(realm: &mut Realm, this: &JsValue, _a: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(datetime(realm, this)?.day() as f64))
}
fn get_day(realm: &mut Realm, this: &JsValue, _a: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(
        datetime(realm, this)?.weekday().number_days_from_sunday() as f64,
    ))
}
fn get_hours(realm: &mut Realm, this: &JsValue, _a: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(datetime(realm, this)?.hour() as f64))
}
fn get_minutes(realm: &mut Realm, this: &JsValue, _a: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(datetime(realm, this)?.minute() as f64))
}
fn get_seconds(realm: &mut Realm, this: &JsValue, _a: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(datetime(realm, this)?.second() as f64))
}
fn get_milliseconds(realm: &mut Realm, this: &JsValue, _a: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::number(
        (datetime(realm, this)?.millisecond()) as f64,
    ))
}
fn get_timezone_offset(realm: &mut Realm, this: &JsValue, _a: &[JsValue]) -> JsResult<JsValue> {
    let date = datetime(realm, this)?;
    Ok(JsValue::number(-(date.offset().whole_minutes() as f64)))
}

fn set_time(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = arg_number(arguments, 0)?;
    let object = this
        .as_object()
        .ok_or_else(|| realm.type_error("Date method called on incompatible receiver"))?;
    if let ObjectKind::Date(date) = &mut object.to_ref().borrow_mut().kind {
        *date = value;
    }
    Ok(JsValue::number(value))
}

fn parse(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_string(realm, arg(arguments, 0))?;
    Ok(JsValue::number(parse_date(&value)))
}

fn parse_date(value: &str) -> f64 {
    if let Ok(date) = OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339) {
        return date.unix_timestamp_nanos() as f64 / 1_000_000.0;
    }
    if let Ok(date) = time::Date::parse(value, &time::format_description::well_known::Iso8601::DATE)
    {
        return PrimitiveDateTime::new(date, Time::MIDNIGHT)
            .assume_utc()
            .unix_timestamp_nanos() as f64
            / 1_000_000.0;
    }
    f64::NAN
}

fn utc(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let year = arg_number(arguments, 0)? as i32;
    let month = arg_number(arguments, 1).unwrap_or(0.0) as i32;
    let day = arg_number(arguments, 2).unwrap_or(1.0) as i32;
    let hour = arg_number(arguments, 3).unwrap_or(0.0) as i32;
    let minute = arg_number(arguments, 4).unwrap_or(0.0) as i32;
    let second = arg_number(arguments, 5).unwrap_or(0.0) as i32;
    let millisecond = arg_number(arguments, 6).unwrap_or(0.0) as i32;
    let _ = realm;
    Ok(JsValue::number(make_date(
        year,
        month,
        day,
        hour,
        minute,
        second,
        millisecond,
    )))
}

fn make_date(
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    millisecond: i32,
) -> f64 {
    let year = if (0..=99).contains(&year) {
        year + 1900
    } else {
        year
    };
    let total_months = year * 12 + month;
    let normalized_year = total_months.div_euclid(12);
    let normalized_month = total_months.rem_euclid(12) + 1;
    let Some(month) = Month::try_from(normalized_month as u8).ok() else {
        return f64::NAN;
    };
    let Some(date) = Date::from_calendar_date(normalized_year, month, 1).ok() else {
        return f64::NAN;
    };
    let date = date + time::Duration::days((day - 1) as i64);
    let Some(time) = Time::from_hms_milli(
        hour.clamp(0, 23) as u8,
        minute.clamp(0, 59) as u8,
        second.clamp(0, 59) as u8,
        millisecond.clamp(0, 999) as u16,
    )
    .ok() else {
        return f64::NAN;
    };
    let datetime = PrimitiveDateTime::new(date, time).assume_utc();
    datetime.unix_timestamp_nanos() as f64 / 1_000_000.0
}

fn format_date(value: f64) -> String {
    if !value.is_finite() {
        return "Invalid Date".into();
    }
    let Ok(datetime) = OffsetDateTime::from_unix_timestamp_nanos((value * 1_000_000.0) as i128)
    else {
        return "Invalid Date".into();
    };
    datetime
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "Invalid Date".into())
}
