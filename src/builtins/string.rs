use std::rc::Rc;

use super::*;
use crate::object::{RegExpData, utf16_char};
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicString / js_string_proto_* (quickjs.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.string_proto.clone();
    install_constructor(realm, "String", 1, constructor, &prototype);
    define_method(realm, &prototype, "toString", 0, to_string);
    define_method(realm, &prototype, "valueOf", 0, to_string);
    define_method(realm, &prototype, "charAt", 1, char_at);
    define_method(realm, &prototype, "charCodeAt", 1, char_code_at);
    define_method(realm, &prototype, "codePointAt", 1, code_point_at);
    define_method(realm, &prototype, "at", 1, at);
    define_method(realm, &prototype, "indexOf", 1, index_of);
    define_method(realm, &prototype, "lastIndexOf", 1, last_index_of);
    define_method(realm, &prototype, "includes", 1, includes);
    define_method(realm, &prototype, "startsWith", 1, starts_with);
    define_method(realm, &prototype, "endsWith", 1, ends_with);
    define_method(realm, &prototype, "slice", 2, slice);
    define_method(realm, &prototype, "substring", 2, substring);
    define_method(realm, &prototype, "substr", 2, substr);
    define_method(realm, &prototype, "toUpperCase", 0, to_upper_case);
    define_method(realm, &prototype, "toLowerCase", 0, to_lower_case);
    define_method(realm, &prototype, "toLocaleUpperCase", 0, to_upper_case);
    define_method(realm, &prototype, "toLocaleLowerCase", 0, to_lower_case);
    define_method(realm, &prototype, "trim", 0, trim);
    define_method(realm, &prototype, "trimStart", 0, trim_start);
    define_method(realm, &prototype, "trimEnd", 0, trim_end);
    define_method(realm, &prototype, "concat", 1, concat);
    define_method(realm, &prototype, "repeat", 1, repeat);
    define_method(realm, &prototype, "padStart", 1, pad_start);
    define_method(realm, &prototype, "padEnd", 1, pad_end);
    define_method(realm, &prototype, "split", 2, split);
    define_method(realm, &prototype, "replace", 2, replace);
    define_method(realm, &prototype, "replaceAll", 2, replace_all);
    define_method(realm, &prototype, "match", 1, match_);
    define_method(realm, &prototype, "search", 1, search);
    define_method(realm, &prototype, "localeCompare", 1, locale_compare);
    let iterator = realm.well_known_symbol("iterator");
    define_method_symbol(
        realm,
        &prototype,
        iterator,
        "[Symbol.iterator]",
        0,
        iterator_values,
    );

    let constructor = realm.global_get("String").as_object().unwrap();
    let target = constructor.to_ref();
    define_method(realm, &target, "fromCharCode", 1, from_char_code);
    define_method(realm, &target, "fromCodePoint", 1, from_code_point);
    define_method(realm, &target, "raw", 1, raw);
}

fn constructor(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = if arguments.is_empty() {
        Rc::from("")
    } else {
        Rc::from(crate::vm::to_string(realm, arg(arguments, 0))?)
    };
    if this.is_object() && !this.is_function() {
        let object = realm.boxed_string(value);
        return Ok(JsValue::Object(object));
    }
    Ok(JsValue::String(value))
}

fn to_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(this_string(realm, this)?))
}

fn char_at(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let index = to_integer(arg_number(realm, arguments, 0)?);
    if index < 0 {
        return Ok(JsValue::string(""));
    }
    Ok(JsValue::string(
        utf16_char(&value, index as usize)
            .map(|ch| ch.to_string())
            .unwrap_or_default(),
    ))
}

fn char_code_at(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let index = to_integer(arg_number(realm, arguments, 0)?);
    if index < 0 {
        return Ok(JsValue::number(f64::NAN));
    }
    Ok(value
        .encode_utf16()
        .nth(index as usize)
        .map(|unit| JsValue::number(unit as f64))
        .unwrap_or(JsValue::number(f64::NAN)))
}

fn code_point_at(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let index = to_integer(arg_number(realm, arguments, 0)?);
    if index < 0 {
        return Ok(JsValue::Undefined);
    }
    Ok(value
        .chars()
        .nth(index as usize)
        .map(|ch| JsValue::number(ch as u32 as f64))
        .unwrap_or(JsValue::Undefined))
}

fn at(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let chars: Vec<char> = value.chars().collect();
    let index = to_integer(arg_number(realm, arguments, 0)?);
    let index = if index < 0 {
        chars.len() as i64 + index
    } else {
        index
    };
    Ok(if index >= 0 && (index as usize) < chars.len() {
        JsValue::string(chars[index as usize].to_string())
    } else {
        JsValue::Undefined
    })
}

fn index_of(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let search = crate::vm::to_string(realm, arg(arguments, 0))?;
    let start = to_integer(arg_number(realm, arguments, 1).unwrap_or(0.0)).max(0) as usize;
    if start > value.len() {
        return Ok(JsValue::number(-1.0));
    }
    Ok(JsValue::number(
        value[start.min(value.len())..]
            .find(&search)
            .map(|index| (index + start.min(value.len())) as f64)
            .unwrap_or(-1.0),
    ))
}

fn last_index_of(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let search = crate::vm::to_string(realm, arg(arguments, 0))?;
    let end = if arguments.len() > 1 {
        to_integer(arg_number(realm, arguments, 1)?).max(0) as usize
    } else {
        value.len()
    };
    let end = end.min(value.len());
    Ok(JsValue::number(
        value[..end]
            .rfind(&search)
            .map(|index| index as f64)
            .unwrap_or(-1.0),
    ))
}

fn includes(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let search = crate::vm::to_string(realm, arg(arguments, 0))?;
    let start = to_integer(arg_number(realm, arguments, 1).unwrap_or(0.0)).max(0) as usize;
    Ok(JsValue::Bool(
        start <= value.len() && value[start..].contains(&search),
    ))
}

fn starts_with(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let search = crate::vm::to_string(realm, arg(arguments, 0))?;
    let start = to_integer(arg_number(realm, arguments, 1).unwrap_or(0.0)).max(0) as usize;
    Ok(JsValue::Bool(
        start <= value.len() && value[start..].starts_with(&search),
    ))
}

fn ends_with(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let search = crate::vm::to_string(realm, arg(arguments, 0))?;
    let end = if arguments.get(1).is_some_and(|value| !value.is_undefined()) {
        to_integer(arg_number(realm, arguments, 1)?).clamp(0, value.len() as i64) as usize
    } else {
        value.len()
    };
    Ok(JsValue::Bool(value[..end].ends_with(&search)))
}

fn slice(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value: Vec<char> = this_string(realm, this)?.chars().collect();
    let length = value.len();
    let start = normalize_index(arg_number(realm, arguments, 0)?, length);
    let end = normalize_index(arg_number(realm, arguments, 1).unwrap_or(length as f64), length);
    Ok(JsValue::string(
        value[if start < end { start } else { end }..end.max(start)]
            .iter()
            .collect::<String>(),
    ))
}

fn substring(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value: Vec<char> = this_string(realm, this)?.chars().collect();
    let length = value.len();
    let mut start = to_integer(arg_number(realm, arguments, 0)?).clamp(0, length as i64) as usize;
    let mut end = if arguments.get(1).is_some_and(|value| !value.is_undefined()) {
        to_integer(arg_number(realm, arguments, 1)?).clamp(0, length as i64) as usize
    } else {
        length
    };
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    Ok(JsValue::string(
        value[start..end].iter().collect::<String>(),
    ))
}

fn substr(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value: Vec<char> = this_string(realm, this)?.chars().collect();
    let length = value.len();
    let start = normalize_index(arg_number(realm, arguments, 0)?, length);
    let count = if arguments.get(1).is_some_and(|value| !value.is_undefined()) {
        arg_number(realm, arguments, 1)?.max(0.0) as usize
    } else {
        length - start
    };
    Ok(JsValue::string(
        value[start..(start + count).min(length)]
            .iter()
            .collect::<String>(),
    ))
}

fn to_upper_case(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(this_string(realm, this)?.to_uppercase()))
}

fn to_lower_case(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(this_string(realm, this)?.to_lowercase()))
}

fn trim(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(this_string(realm, this)?.trim()))
}

fn trim_start(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(this_string(realm, this)?.trim_start()))
}

fn trim_end(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(this_string(realm, this)?.trim_end()))
}

fn concat(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let mut result = this_string(realm, this)?;
    for argument in arguments {
        result.push_str(&crate::vm::to_string(realm, argument.clone())?);
    }
    Ok(JsValue::string(result))
}

fn repeat(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let count = arg_number(realm, arguments, 0)?.max(0.0) as usize;
    Ok(JsValue::string(value.repeat(count)))
}

fn pad_start(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    pad(realm, this, arguments, true)
}

fn pad_end(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    pad(realm, this, arguments, false)
}

fn pad(realm: &mut Realm, this: &JsValue, arguments: &[JsValue], start: bool) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let target = arg_number(realm, arguments, 0)?.max(0.0) as usize;
    let fill = if arguments.len() > 1 && !arg(arguments, 1).is_undefined() {
        crate::vm::to_string(realm, arg(arguments, 1))?
    } else {
        " ".into()
    };
    let current = value.chars().count();
    if target <= current || fill.is_empty() {
        return Ok(JsValue::string(value));
    }
    let needed = target - current;
    let mut padding = String::new();
    while padding.chars().count() < needed {
        padding.push_str(&fill);
    }
    let padding: String = padding.chars().take(needed).collect();
    Ok(JsValue::string(if start {
        padding + &value
    } else {
        value + &padding
    }))
}

fn split(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let separator = arg(arguments, 0);
    let limit = if arguments.get(1).is_some_and(|value| !value.is_undefined()) {
        crate::vm::to_uint32(arg_number(realm, arguments, 1)?.max(0.0)) as usize
    } else {
        usize::MAX
    };
    if limit == 0 {
        return Ok(JsValue::Object(realm.array(Vec::new())));
    }
    if separator.is_undefined() {
        return Ok(JsValue::Object(realm.array(vec![JsValue::string(value)])));
    }
    let parts = if let JsValue::Object(object) = &separator {
        let data = object.borrow();
        if let crate::object::ObjectKind::RegExp(regexp) = &data.kind {
            let regexp = regexp.clone();
            drop(data);
            split_regex(realm, &value, &regexp)?
        } else {
            drop(data);
            value
                .split(&crate::vm::to_string(realm, separator.clone())?)
                .map(|part| JsValue::string(part.to_owned()))
                .collect()
        }
    } else {
        value
            .split(&crate::vm::to_string(realm, separator)?)
            .map(|part| JsValue::string(part.to_owned()))
            .collect()
    };
    Ok(JsValue::Object(
        realm.array(parts.into_iter().take(limit).collect()),
    ))
}

fn split_regex(realm: &mut Realm, value: &str, regexp: &RegExpData) -> JsResult<Vec<JsValue>> {
    let mut result = Vec::new();
    let mut last = 0;
    for matched in regexp.regex.find_iter(value) {
        let range = matched.range();
        result.push(JsValue::string(value[last..range.start].to_owned()));
        last = range.end;
        for group in 1..matched.captures.len() {
            if let Some(range) = matched.group(group) {
                result.push(JsValue::string(value[range].to_owned()));
            } else {
                result.push(JsValue::Undefined);
            }
        }
    }
    result.push(JsValue::string(value[last..].to_owned()));
    let _ = realm;
    Ok(result)
}

fn replace(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    replace_impl(realm, this, arguments, false)
}

fn replace_all(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    replace_impl(realm, this, arguments, true)
}

fn replace_impl(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
    all: bool,
) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let search = arg(arguments, 0);
    let replacement = arg(arguments, 1);
    if let JsValue::Object(object) = &search {
        let regexp = {
            let data = object.borrow();
            if let crate::object::ObjectKind::RegExp(regexp) = &data.kind {
                Some(regexp.clone())
            } else {
                None
            }
        };
        if let Some(regexp) = regexp {
            let all = all || regexp.flags.contains('g');
            return replace_regex(realm, &value, &regexp, replacement, all);
        }
    }
    let needle = crate::vm::to_string(realm, search)?;
    if needle.is_empty() {
        return Ok(JsValue::string(value));
    }
    let mut result = String::new();
    let mut offset = 0;
    while let Some(index) = value[offset..].find(&needle) {
        let absolute = offset + index;
        result.push_str(&value[offset..absolute]);
        let replacement_value = if replacement.is_function() {
            super::call(
                realm,
                replacement.clone(),
                JsValue::Undefined,
                &[JsValue::string(needle.clone())],
            )?
        } else {
            JsValue::string(expand_replacement(
                &crate::vm::to_string(realm, replacement.clone())?,
                &needle,
                &needle,
                absolute,
                &value,
            ))
        };
        result.push_str(&crate::vm::to_string(realm, replacement_value)?);
        offset = absolute + needle.len();
        if !all {
            break;
        }
    }
    result.push_str(&value[offset..]);
    Ok(JsValue::string(result))
}

fn replace_regex(
    realm: &mut Realm,
    value: &str,
    regexp: &RegExpData,
    replacement: JsValue,
    all: bool,
) -> JsResult<JsValue> {
    let mut result = String::new();
    let mut last = 0;
    for matched in regexp.regex.find_iter(value) {
        let range = matched.range();
        result.push_str(&value[last..range.start]);
        let matched_text = &value[range.clone()];
        let replacement_value = if replacement.is_function() {
            let mut arguments = vec![JsValue::string(matched_text.to_owned())];
            for group in 1..matched.captures.len() {
                arguments.push(
                    matched
                        .group(group)
                        .map(|range| JsValue::string(value[range].to_owned()))
                        .unwrap_or(JsValue::Undefined),
                );
            }
            arguments.push(JsValue::number(range.start as f64));
            arguments.push(JsValue::string(value.to_owned()));
            super::call(realm, replacement.clone(), JsValue::Undefined, &arguments)?
        } else {
            JsValue::string(expand_replacement(
                &crate::vm::to_string(realm, replacement.clone())?,
                matched_text,
                &value[range.clone()],
                range.start,
                value,
            ))
        };
        result.push_str(&crate::vm::to_string(realm, replacement_value)?);
        last = range.end;
        if !all {
            break;
        }
    }
    result.push_str(&value[last..]);
    Ok(JsValue::string(result))
}

fn expand_replacement(
    replacement: &str,
    matched: &str,
    _matched_group: &str,
    _index: usize,
    _input: &str,
) -> String {
    replacement.replace("$&", matched)
}

fn match_(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let regexp = if let Some(regexp) = regexp_data(&arg(arguments, 0)) {
        regexp
    } else {
        let source = if arg(arguments, 0).is_undefined() {
            String::new()
        } else {
            crate::vm::to_string(realm, arg(arguments, 0))?
        };
        let regex = regress::Regex::new(&source)
            .map_err(|error| realm.syntax_error(format!("Invalid regular expression: {error}")))?;
        Rc::new(RegExpData {
            source,
            flags: String::new(),
            regex: Rc::new(regex),
        })
    };
    if regexp.flags.contains('g') {
        let mut matches = Vec::new();
        for matched in regexp.regex.find_iter(&value) {
            let range = matched.range();
            matches.push(JsValue::string(value[range].to_owned()));
        }
        return Ok(if matches.is_empty() {
            JsValue::Null
        } else {
            JsValue::Object(realm.array(matches))
        });
    }
    let Some(matched) = regexp.regex.find(&value) else {
        return Ok(JsValue::Null);
    };
    let range = matched.range();
    let mut values = vec![JsValue::string(value[range.clone()].to_owned())];
    for index in 1..=matched.captures.len() {
        values.push(
            matched
                .group(index)
                .map(|range| JsValue::string(value[range].to_owned()))
                .unwrap_or(JsValue::Undefined),
        );
    }
    let result = realm.array(values);
    define_value(&result, "index", JsValue::number(range.start as f64));
    define_value(&result, "input", JsValue::string(value));
    Ok(JsValue::Object(result))
}

fn search(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    if let Some(regexp) = regexp_data(&arg(arguments, 0)) {
        return Ok(JsValue::number(
            regexp
                .regex
                .find(&value)
                .map(|matched| matched.range().start as f64)
                .unwrap_or(-1.0),
        ));
    }
    let pattern = crate::vm::to_string(realm, arg(arguments, 0))?;
    let regex = regress::Regex::new(&pattern)
        .map_err(|error| realm.syntax_error(format!("Invalid regular expression: {error}")))?;
    Ok(JsValue::number(
        regex
            .find(&value)
            .map(|matched| matched.range().start as f64)
            .unwrap_or(-1.0),
    ))
}

fn locale_compare(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let other = crate::vm::to_string(realm, arg(arguments, 0))?;
    Ok(JsValue::number(match value.cmp(&other) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

fn iterator_values(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    Ok(JsValue::Object(
        realm.iterator(
            value
                .chars()
                .map(|ch| JsValue::string(ch.to_string()))
                .collect(),
        ),
    ))
}

fn regexp_data(value: &JsValue) -> Option<Rc<RegExpData>> {
    match value {
        JsValue::Object(object) => match &object.borrow().kind {
            crate::object::ObjectKind::RegExp(regexp) => Some(regexp.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn from_char_code(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let mut result = String::new();
    for argument in arguments {
        let unit = crate::vm::to_uint32(arg_number(realm, std::slice::from_ref(argument), 0)?) & 0xFFFF;
        if let Some(ch) = char::from_u32(unit) {
            result.push(ch);
        }
    }
    let _ = realm;
    Ok(JsValue::string(result))
}

fn from_code_point(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let mut result = String::new();
    for argument in arguments {
        let value = arg_number(realm, std::slice::from_ref(argument), 0)?;
        if !(0.0..=0x10FFFF as f64).contains(&value) {
            return Err(realm.range_error("Invalid code point"));
        }
        result.push(char::from_u32(value as u32).unwrap_or('\u{FFFD}'));
    }
    Ok(JsValue::string(result))
}

fn raw(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let template = arg(arguments, 0);
    let raw = get(realm, &template, "raw")?;
    let values = arguments.get(1..).unwrap_or(&[]);
    let pieces = crate::vm::iterable_items(realm, raw)?;
    let mut result = String::new();
    for (index, piece) in pieces.iter().enumerate() {
        result.push_str(&crate::vm::to_string(realm, piece.clone())?);
        if index < values.len() {
            result.push_str(&crate::vm::to_string(realm, values[index].clone())?);
        }
    }
    Ok(JsValue::string(result))
}
