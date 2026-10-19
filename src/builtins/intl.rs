//! `Intl` support, gated behind the `intl` feature because the CLDR data
//! substantially increases binary size.
//!
//! The current implementation provides `Intl.DateTimeFormat` and the
//! `Date.prototype.toLocale*` methods. It uses ICU4X for locale-sensitive
//! formatting and jiff for the system time zone and DST-aware conversion.

use icu_datetime::DateTimeFormatter;
use icu_datetime::fieldsets::builder::{DateFields, FieldSetBuilder};
use icu_datetime::input::{Date, DateTime, Time};
use icu_datetime::options::{Length, TimePrecision};
use icu_datetime::preferences::HourCycle;
use icu_locale::Locale;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use writeable::Writeable;

use super::*;
use crate::object::{IntlDateTimeFormatData, ObjectKind};

#[derive(Clone, Copy)]
enum DateMode {
    DateTime,
    Date,
    Time,
    Intl,
}

pub(crate) fn install_date_methods(realm: &mut Realm, prototype: &ObjRef) {
    define_method(realm, prototype, "toLocaleString", 0, to_locale_string);
    define_method(
        realm,
        prototype,
        "toLocaleDateString",
        0,
        to_locale_date_string,
    );
    define_method(
        realm,
        prototype,
        "toLocaleTimeString",
        0,
        to_locale_time_string,
    );
}

pub(crate) fn install(realm: &mut Realm) {
    let intl = realm.object();
    let date_time_format_proto = realm.object();
    intl.borrow_mut().class_name = "Intl";
    date_time_format_proto.borrow_mut().class_name = "Intl.DateTimeFormat";
    realm.intl_proto = intl.clone();
    realm.intl_date_time_format_proto = date_time_format_proto.clone();

    let constructor = install_constructor(
        realm,
        "DateTimeFormat",
        0,
        date_time_format,
        &date_time_format_proto,
    );
    let constructor_object = constructor
        .as_object()
        .expect("DateTimeFormat constructor is an object")
        .to_ref();
    define_method(
        realm,
        &constructor_object,
        "supportedLocalesOf",
        1,
        supported_locales_of,
    );
    define_method(realm, &date_time_format_proto, "format", 1, format);
    define_method(
        realm,
        &date_time_format_proto,
        "formatToParts",
        1,
        format_to_parts,
    );
    define_method(
        realm,
        &date_time_format_proto,
        "resolvedOptions",
        0,
        resolved_options,
    );

    define_value(&intl, "DateTimeFormat", constructor);
    define_method(
        realm,
        &intl,
        "getCanonicalLocales",
        1,
        get_canonical_locales,
    );
    realm
        .global
        .borrow_mut()
        .define("Intl", Property::data(JsValue::Object(intl)));
}

fn date_time_format(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let object = realm.object();
    object.borrow_mut().prototype = Some(realm.intl_date_time_format_proto.clone());
    let data = build_data(realm, arg(arguments, 0), arg(arguments, 1), DateMode::Intl)?;
    {
        let mut data_ref = object.borrow_mut();
        data_ref.kind = ObjectKind::IntlDateTimeFormat(Box::new(data));
        data_ref.class_name = "Intl.DateTimeFormat";
    }
    Ok(JsValue::Object(object))
}

fn format_date_value(
    realm: &mut Realm,
    value: f64,
    locales: JsValue,
    options: JsValue,
    mode: DateMode,
) -> JsResult<JsValue> {
    if !value.is_finite() {
        return Ok(JsValue::string("Invalid Date"));
    }
    let data = build_data(realm, locales, options, mode)?;
    Ok(JsValue::string(format_value(realm, &data, value)?))
}

fn to_locale_string(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = date_value(realm, this)?;
    format_date_value(
        realm,
        value,
        arg(arguments, 0),
        arg(arguments, 1),
        DateMode::DateTime,
    )
}

fn to_locale_date_string(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let value = date_value(realm, this)?;
    format_date_value(
        realm,
        value,
        arg(arguments, 0),
        arg(arguments, 1),
        DateMode::Date,
    )
}

fn to_locale_time_string(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let value = date_value(realm, this)?;
    format_date_value(
        realm,
        value,
        arg(arguments, 0),
        arg(arguments, 1),
        DateMode::Time,
    )
}

fn date_value(realm: &mut Realm, this: &JsValue) -> JsResult<f64> {
    let object = this
        .as_object()
        .map(|object| object.to_ref())
        .ok_or_else(|| realm.type_error("Date method called on incompatible receiver"))?;
    match &object.borrow().kind {
        ObjectKind::Date(value) => Ok(*value),
        _ => Err(realm.type_error("Date method called on incompatible receiver")),
    }
}

fn format(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = receiver(realm, this)?;
    let value = format_argument(realm, arg(arguments, 0))?;
    if !value.is_finite() {
        return Err(realm.range_error("Invalid time value"));
    }
    let data = object.borrow();
    let ObjectKind::IntlDateTimeFormat(data) = &data.kind else {
        return Err(realm.type_error("Intl.DateTimeFormat method called on incompatible receiver"));
    };
    Ok(JsValue::string(format_value(realm, data, value)?))
}

fn format_argument(realm: &mut Realm, value: JsValue) -> JsResult<f64> {
    if let JsValue::Object(object) = &value
        && let ObjectKind::Date(value) = &object.borrow().kind
    {
        return Ok(*value);
    }
    crate::vm::to_number(realm, value)
}

fn format_to_parts(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = format(realm, this, arguments)?;
    let part = realm.object();
    define_value(&part, "type", JsValue::string("literal"));
    define_value(&part, "value", value);
    Ok(JsValue::Object(realm.array(vec![JsValue::Object(part)])))
}

fn resolved_options(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    let object = receiver(realm, this)?;
    let data_ref = object.borrow();
    let ObjectKind::IntlDateTimeFormat(data) = &data_ref.kind else {
        return Err(realm.type_error("Intl.DateTimeFormat method called on incompatible receiver"));
    };
    let result = realm.object();
    define_value(&result, "locale", JsValue::string(data.locale.to_string()));
    define_value(&result, "calendar", JsValue::string(data.calendar.clone()));
    define_value(
        &result,
        "numberingSystem",
        JsValue::string(data.numbering_system.clone()),
    );
    define_value(
        &result,
        "timeZone",
        JsValue::string(data.time_zone_name.clone()),
    );
    if let Some(style) = &data.date_style {
        define_value(&result, "dateStyle", JsValue::string(style.clone()));
    }
    if let Some(style) = &data.time_style {
        define_value(&result, "timeStyle", JsValue::string(style.clone()));
    }
    let (year, month, day, weekday) = date_field_flags(data.date_fields);
    let (hour, minute, second, subsecond) = time_precision_flags(data.time_precision);
    for (name, enabled) in [
        ("year", year),
        ("month", month),
        ("day", day),
        ("weekday", weekday),
        ("hour", hour),
        ("minute", minute),
        ("second", second),
    ] {
        if enabled {
            define_value(&result, name, JsValue::Bool(true));
        }
    }
    if subsecond {
        define_value(&result, "fractionalSecondDigits", JsValue::number(3.0));
    }
    define_value(
        &result,
        "hour12",
        JsValue::Bool(data.hour12.unwrap_or(false)),
    );
    Ok(JsValue::Object(result))
}

fn supported_locales_of(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let locales = canonical_locales(realm, arg(arguments, 0))?;
    Ok(JsValue::Object(
        realm.array(locales.into_iter().map(JsValue::string).collect()),
    ))
}

fn get_canonical_locales(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let locales = canonical_locales(realm, arg(arguments, 0))?;
    Ok(JsValue::Object(
        realm.array(locales.into_iter().map(JsValue::string).collect()),
    ))
}

fn canonical_locales(realm: &mut Realm, value: JsValue) -> JsResult<Vec<String>> {
    let locales = locale_strings(realm, value)?;
    let mut result = Vec::with_capacity(locales.len());
    for value in locales {
        let locale =
            Locale::try_from_str(&value).map_err(|_| realm.range_error("Invalid language tag"))?;
        result.push(locale.to_string());
    }
    Ok(result)
}

fn locale_strings(realm: &mut Realm, value: JsValue) -> JsResult<Vec<String>> {
    match value {
        JsValue::Undefined => Ok(Vec::new()),
        JsValue::String(value) => Ok(vec![value.to_string()]),
        JsValue::Object(object) => {
            let object = Object::from_ref(object);
            if !object.to_ref().borrow().is_array() {
                return Err(realm.type_error("locales must be a string or array"));
            }
            let items = array_items(realm, &object)?;
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                result.push(crate::vm::to_string(realm, item)?);
            }
            Ok(result)
        }
        _ => Err(realm.type_error("locales must be a string or array")),
    }
}

fn first_locale(realm: &mut Realm, value: JsValue) -> JsResult<Locale> {
    let locales = locale_strings(realm, value)?;
    let mut first = None;
    for value in locales {
        let locale = Locale::try_from_str(&value)
            .map_err(|_| realm.range_error("Invalid language tag"))?;
        if first.is_none() {
            first = Some(locale);
        }
    }
    Ok(first.unwrap_or(Locale::UNKNOWN))
}

fn build_data(
    realm: &mut Realm,
    locales: JsValue,
    options: JsValue,
    mode: DateMode,
) -> JsResult<IntlDateTimeFormatData> {
    let locale = first_locale(realm, locales)?;
    let date_style = string_option(realm, &options, "dateStyle")?;
    let time_style = string_option(realm, &options, "timeStyle")?;
    if let Some(style) = &date_style
        && !matches!(style.as_str(), "full" | "long" | "medium" | "short")
    {
        return Err(realm.range_error("Invalid dateStyle"));
    }
    if let Some(style) = &time_style
        && !matches!(style.as_str(), "full" | "long" | "medium" | "short")
    {
        return Err(realm.range_error("Invalid timeStyle"));
    }

    let time_zone_name = string_option(realm, &options, "timeZone")?;
    let time_zone = match &time_zone_name {
        Some(name) => TimeZone::get(name).map_err(|_| realm.range_error("Invalid time zone"))?,
        None => TimeZone::system(),
    };
    let resolved_time_zone = time_zone
        .iana_name()
        .map(str::to_string)
        .unwrap_or_else(|| "UTC".to_string());

    let mut builder = FieldSetBuilder::new();
    let date_fields_option = date_fields_from_options(realm, &options, &date_style)?;
    builder.date_fields = date_fields_option;
    builder.time_precision = time_precision_from_options(realm, &options, &time_style)?;
    if date_style.is_some() || time_style.is_some() {
        builder.length = Some(length_from_style(
            date_style.as_deref().or(time_style.as_deref()),
        ));
    }

    apply_mode_defaults(&mut builder, mode);
    let date_fields = builder.date_fields;
    let time_precision = builder.time_precision;
    let hour12 = bool_option(realm, &options, "hour12")?;
    let field_set = builder
        .build_composite_datetime()
        .map_err(|_| realm.range_error("Unsupported date/time options"))?;
    let mut preferences = icu_datetime::DateTimeFormatterPreferences::from(locale.clone());
    if let Some(hour12) = hour12 {
        preferences.hour_cycle = Some(if hour12 {
            HourCycle::H12
        } else {
            HourCycle::H23
        });
    }
    let formatter = DateTimeFormatter::try_new(preferences, field_set)
        .map_err(|_| realm.range_error("Unsupported date/time options"))?;
    let calendar =
        string_option(realm, &options, "calendar")?.unwrap_or_else(|| "gregory".to_string());
    let numbering_system =
        string_option(realm, &options, "numberingSystem")?.unwrap_or_else(|| "latn".to_string());

    Ok(IntlDateTimeFormatData {
        formatter,
        locale,
        time_zone,
        time_zone_name: resolved_time_zone,
        date_fields,
        time_precision,
        date_style,
        time_style,
        hour12,
        calendar,
        numbering_system,
    })
}

fn apply_mode_defaults(builder: &mut FieldSetBuilder, mode: DateMode) {
    match mode {
        DateMode::DateTime => {
            if builder.date_fields.is_none() && builder.time_precision.is_none() {
                builder.date_fields = Some(DateFields::YMD);
                builder.time_precision = Some(TimePrecision::Second);
                builder.length = Some(Length::Medium);
            }
        }
        DateMode::Date => {
            if builder.date_fields.is_none() {
                builder.date_fields = Some(DateFields::YMD);
                builder.length = Some(Length::Medium);
            }
            builder.time_precision = None;
        }
        DateMode::Intl => {
            if builder.date_fields.is_none() && builder.time_precision.is_none() {
                builder.date_fields = Some(DateFields::YMD);
                builder.length = Some(Length::Short);
            }
        }
        DateMode::Time => {
            if builder.time_precision.is_none() {
                builder.time_precision = Some(TimePrecision::Second);
                builder.length = Some(Length::Medium);
            }
            builder.date_fields = None;
        }
    }
}

fn date_fields_from_options(
    realm: &mut Realm,
    options: &JsValue,
    style: &Option<String>,
) -> JsResult<Option<DateFields>> {
    if let Some(style) = style {
        return Ok(Some(if style == "full" {
            DateFields::YMDE
        } else {
            DateFields::YMD
        }));
    }
    let year = !option(realm, options, "year")?.is_undefined();
    let month = !option(realm, options, "month")?.is_undefined();
    let day = !option(realm, options, "day")?.is_undefined();
    let weekday = !option(realm, options, "weekday")?.is_undefined();
    let fields = match (year, month, day, weekday) {
        (true, true, true, true) => DateFields::YMDE,
        (true, true, true, false) => DateFields::YMD,
        (true, true, false, false) => DateFields::YM,
        (false, true, true, false) => DateFields::MD,
        (false, true, true, true) => DateFields::MDE,
        (false, true, false, false) => DateFields::M,
        (false, false, true, false) => DateFields::D,
        (false, false, false, true) => DateFields::E,
        (true, false, false, false) => DateFields::Y,
        _ => return Ok(None),
    };
    Ok(Some(fields))
}

fn time_precision_from_options(
    realm: &mut Realm,
    options: &JsValue,
    style: &Option<String>,
) -> JsResult<Option<TimePrecision>> {
    if let Some(style) = style {
        return Ok(Some(if style == "short" {
            TimePrecision::Minute
        } else {
            TimePrecision::Second
        }));
    }
    if !option(realm, options, "fractionalSecondDigits")?.is_undefined() {
        return Ok(Some(TimePrecision::Subsecond(
            icu_datetime::options::SubsecondDigits::S3,
        )));
    }
    let second = !option(realm, options, "second")?.is_undefined();
    let minute = !option(realm, options, "minute")?.is_undefined();
    let hour = !option(realm, options, "hour")?.is_undefined();
    Ok(if second {
        Some(TimePrecision::Second)
    } else if minute {
        Some(TimePrecision::Minute)
    } else if hour {
        Some(TimePrecision::Hour)
    } else {
        None
    })
}

fn length_from_style(style: Option<&str>) -> Length {
    match style {
        Some("full") => Length::Long,
        Some("long") => Length::Long,
        Some("short") => Length::Short,
        _ => Length::Medium,
    }
}

fn date_field_flags(fields: Option<DateFields>) -> (bool, bool, bool, bool) {
    match fields {
        Some(DateFields::YMD) => (true, true, true, false),
        Some(DateFields::YMDE) => (true, true, true, true),
        Some(DateFields::YM) => (true, true, false, false),
        Some(DateFields::MD) => (false, true, true, false),
        Some(DateFields::MDE) => (false, true, true, true),
        Some(DateFields::M) => (false, true, false, false),
        Some(DateFields::D) => (false, false, true, false),
        Some(DateFields::E) => (false, false, false, true),
        Some(DateFields::Y) => (true, false, false, false),
        _ => (false, false, false, false),
    }
}

fn time_precision_flags(precision: Option<TimePrecision>) -> (bool, bool, bool, bool) {
    match precision {
        Some(TimePrecision::Hour) => (true, false, false, false),
        Some(TimePrecision::Minute | TimePrecision::MinuteOptional) => (true, true, false, false),
        Some(TimePrecision::Second) => (true, true, true, false),
        Some(TimePrecision::Subsecond(_)) => (true, true, true, true),
        Some(_) => (true, true, true, false),
        None => (false, false, false, false),
    }
}

fn format_value(realm: &mut Realm, data: &IntlDateTimeFormatData, value: f64) -> JsResult<String> {
    let timestamp = Timestamp::from_millisecond(value as i64)
        .map_err(|_| realm.range_error("Date outside of the supported Intl range"))?;
    let zoned = timestamp.to_zoned(data.time_zone.clone());
    let date = Date::try_new_iso(zoned.year() as i32, zoned.month() as u8, zoned.day() as u8)
        .map_err(|_| realm.range_error("Invalid date"))?;
    let time = Time::try_new(
        zoned.hour() as u8,
        zoned.minute() as u8,
        zoned.second() as u8,
        zoned.subsec_nanosecond() as u32,
    )
    .map_err(|_| realm.range_error("Invalid time"))?;
    Ok(data
        .formatter
        .format(&DateTime { date, time })
        .write_to_string()
        .into_owned())
}

fn receiver(realm: &mut Realm, this: &JsValue) -> JsResult<ObjRef> {
    this.as_object()
        .map(|object| object.to_ref())
        .ok_or_else(|| {
            realm.type_error("Intl.DateTimeFormat method called on incompatible receiver")
        })
}

fn option(realm: &mut Realm, options: &JsValue, name: &str) -> JsResult<JsValue> {
    if options.is_undefined() {
        return Ok(JsValue::Undefined);
    }
    let object = options
        .as_object()
        .map(|object| object.to_ref())
        .ok_or_else(|| realm.type_error("options must be an object"))?;
    crate::vm::get_value_property(
        realm,
        &JsValue::Object(object),
        &crate::object::PropKey::from(name),
    )
}

fn string_option(realm: &mut Realm, options: &JsValue, name: &str) -> JsResult<Option<String>> {
    let value = option(realm, options, name)?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(crate::vm::to_string(realm, value)?))
    }
}

fn bool_option(realm: &mut Realm, options: &JsValue, name: &str) -> JsResult<Option<bool>> {
    let value = option(realm, options, name)?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(crate::coerce::to_boolean(&value)))
    }
}
