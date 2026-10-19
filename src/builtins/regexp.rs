use boa_gc::{Gc, GcRefCell};
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
            return make_regexp(realm, &source, &flags);
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
    make_regexp(realm, &source, &flags)
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

pub(crate) fn make_regexp(realm: &mut Realm, source: &str, flags: &str) -> JsResult<JsValue> {
    let compile_flags: String = flags.chars().filter(|ch| "imsu".contains(*ch)).collect();
    // Invalid patterns throw, like `js_compile_regexp` (quickjs.c:49197).
    let regex = regress::Regex::with_flags(source, compile_flags.as_str())
        .map_err(|error| realm.syntax_error(format!("Invalid regular expression: {error}")))?;
    let data = Rc::new(RegExpData {
        // Normalized source (ES2022 22.1.3.2.1): empty becomes `(?:)`.
        source: if source.is_empty() {
            "(?:)".to_owned()
        } else {
            source.to_owned()
        },
        flags: normalize_flags(flags),
        regex: Rc::new(regex),
    });
    let object = ObjectData::regexp(data, Some(realm.regexp_proto.clone()));
    Ok(JsValue::Object(Gc::new(GcRefCell::new(object))))
}

/// Sort flags into `dgimsuvy` order (ES2022 22.1.3.2.1).
fn normalize_flags(flags: &str) -> String {
    "dgimsuvy"
        .chars()
        .filter(|ch| flags.contains(*ch))
        .collect()
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
    let global = data.flags.contains('g');
    let sticky = data.flags.contains('y');
    let start = if global || sticky {
        let last_index = get(realm, this, "lastIndex")?;
        let start = crate::vm::to_uint32(last_index.as_number().unwrap_or(0.0)) as usize;
        // `lastIndex` beyond the input length fails without a match.
        if start > input.len() {
            if global || sticky {
                set(realm, this, "lastIndex", JsValue::number(0.0))?;
            }
            return Ok(JsValue::Null);
        }
        start
    } else {
        0
    };
    let matched = if sticky {
        // Sticky regexes anchor the match at `lastIndex` exactly.
        data.regex
            .find_from(&input, start)
            .next()
            .filter(|matched| matched.range().start == start)
    } else {
        data.regex.find_from(&input, start).next()
    };
    let Some(matched) = matched else {
        // A failed exec resets `lastIndex` (ES2022 22.2.5.2.2), never
        // restarting the search from the beginning.
        if global || sticky {
            set(realm, this, "lastIndex", JsValue::number(0.0))?;
        }
        return Ok(JsValue::Null);
    };
    let range = matched.range();
    if global || sticky {
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
