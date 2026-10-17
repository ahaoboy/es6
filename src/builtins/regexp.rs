use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::object::{ObjectData, ObjectKind, RegExpData};
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicRegExp / js_regexp_* (quickjs.c, using the
// libregexp bytecode engine in libregexp.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.regexp_proto.clone();
    install_constructor(realm, "RegExp", 2, constructor, &prototype);
    define_method(realm, &prototype, "exec", 1, exec);
    define_method(realm, &prototype, "test", 1, test);
    define_method(realm, &prototype, "toString", 0, to_string);
    define_accessor(realm, &prototype, "source", source, None);
    define_accessor(realm, &prototype, "flags", flags, None);
    define_accessor(realm, &prototype, "global", global, None);
    define_accessor(realm, &prototype, "ignoreCase", ignore_case, None);
    define_accessor(realm, &prototype, "multiline", multiline, None);
    define_accessor(realm, &prototype, "sticky", sticky, None);
    define_accessor(realm, &prototype, "unicode", unicode, None);
    define_value(&prototype, "lastIndex", JsValue::number(0.0));
}

fn constructor(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let pattern = arg(arguments, 0);
    if pattern.is_object() {
        let regexp = pattern.as_object().unwrap();
        let data = regexp.to_ref();
        let data = data.borrow();
        if let ObjectKind::RegExp(data) = &data.kind {
            let source = data.source.clone();
            if arg(arguments, 1).is_undefined() {
                return Ok(pattern);
            }
            let flags = crate::vm::to_string(realm, arg(arguments, 1))?;
            validate_flags(realm, &flags)?;
            return Ok(make_regexp(realm, &source, &flags));
        }
    }
    let source = if arg(arguments, 0).is_undefined() {
        String::new()
    } else {
        crate::vm::to_string(realm, arg(arguments, 0))?
    };
    let flags = if arg(arguments, 1).is_undefined() {
        String::new()
    } else {
        crate::vm::to_string(realm, arg(arguments, 1))?
    };
    validate_flags(realm, &flags)?;
    Ok(make_regexp(realm, &source, &flags))
}

fn validate_flags(realm: &Realm, flags: &str) -> JsResult<()> {
    let mut seen = std::collections::HashSet::new();
    for ch in flags.chars() {
        if !"dgimsuvy".contains(ch) || !seen.insert(ch) {
            return Err(realm.syntax_error(format!("Invalid regular expression flags: {flags}")));
        }
    }
    Ok(())
}

pub(crate) fn make_regexp(realm: &mut Realm, source: &str, flags: &str) -> JsValue {
    let compile_flags: String = flags.chars().filter(|ch| "imsu".contains(*ch)).collect();
    let regex = regress::Regex::with_flags(source, compile_flags.as_str())
        .unwrap_or_else(|_| regress::Regex::new("(?:)").unwrap());
    let data = Rc::new(RegExpData {
        source: source.to_owned(),
        flags: flags.to_owned(),
        regex: Rc::new(regex),
    });
    let object = ObjectData::regexp(data, Some(realm.regexp_proto.clone()));
    JsValue::Object(Rc::new(RefCell::new(object)))
}

fn regexp_data(realm: &Realm, value: &JsValue) -> JsResult<Rc<RegExpData>> {
    match value {
        JsValue::Object(object) => match &object.borrow().kind {
            ObjectKind::RegExp(data) => Ok(data.clone()),
            _ => Err(realm.type_error("RegExp method called on incompatible receiver")),
        },
        _ => Err(realm.type_error("RegExp method called on incompatible receiver")),
    }
}

fn exec(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let data = regexp_data(realm, this)?;
    let input = crate::vm::to_string(realm, arg(arguments, 0))?;
    let start = if data.flags.contains('g') || data.flags.contains('y') {
        let last_index = get(realm, this, "lastIndex")?;
        crate::vm::to_uint32(last_index.as_number().unwrap_or(0.0)) as usize
    } else {
        0
    };
    let start = start.min(input.len());
    let Some(matched) = data
        .regex
        .find_from(&input, start)
        .next()
        .or_else(|| data.regex.find(&input))
    else {
        if data.flags.contains('g') || data.flags.contains('y') {
            set(realm, this, "lastIndex", JsValue::number(0.0))?;
        }
        return Ok(JsValue::Null);
    };
    let range = matched.range();
    if data.flags.contains('g') || data.flags.contains('y') {
        set(realm, this, "lastIndex", JsValue::number(range.end as f64))?;
    }
    let mut values = vec![JsValue::string(input[range.clone()].to_owned())];
    for index in 1..=matched.captures.len() {
        values.push(
            matched
                .group(index)
                .map(|range| JsValue::string(input[range].to_owned()))
                .unwrap_or(JsValue::Undefined),
        );
    }
    let result = realm.array(values);
    define_value(&result, "index", JsValue::number(range.start as f64));
    define_value(&result, "input", JsValue::string(input));
    Ok(JsValue::Object(result))
}

fn test(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Bool(!exec(realm, this, arguments)?.is_null()))
}

fn to_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let data = regexp_data(realm, this)?;
    Ok(JsValue::string(format!("/{}/{}", data.source, data.flags)))
}

fn source(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(regexp_data(realm, this)?.source.clone()))
}

fn flags(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(regexp_data(realm, this)?.flags.clone()))
}

fn flag(realm: &mut Realm, this: &JsValue, name: char) -> JsResult<JsValue> {
    Ok(JsValue::Bool(
        regexp_data(realm, this)?.flags.contains(name),
    ))
}

fn global(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    flag(realm, this, 'g')
}
fn ignore_case(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    flag(realm, this, 'i')
}
fn multiline(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    flag(realm, this, 'm')
}
fn sticky(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    flag(realm, this, 'y')
}
fn unicode(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    flag(realm, this, 'u')
}
