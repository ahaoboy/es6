//! String built-ins.
//!
//! ECMAScript strings are sequences of UTF-16 code units; the backing store
//! is a UTF-8 `String`, so all index arithmetic goes through explicit
//! code-unit conversion (`byte_to_utf16_offset`/`utf16_to_byte_offset`), the
//! same split QuickJS makes between its 8/16-bit strings (`JSString`,
//! quickjs.c:696). Unpaired surrogates cannot be represented and become
//! U+FFFD.

use std::rc::Rc;

use super::*;
use crate::coerce::{byte_to_utf16_offset, trim_ecma_whitespace, utf16_to_byte_offset};
use crate::object::{RegExpData, utf16_char};
use crate::value::JsValue;

/// QuickJS' `JS_STRING_LEN_MAX` (quickjs.c:222).
const MAX_STRING_UNITS: usize = (1 << 30) - 24;

// QuickJS installs these with `JS_AddIntrinsicString` / `js_string_proto_*`.
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
    let units: Vec<u16> = value.encode_utf16().collect();
    let Some(first) = units.get(index as usize).copied() else {
        return Ok(JsValue::Undefined);
    };
    // Combine a surrogate pair into one code point (ES2022 6.1.4).
    let code = if (0xD800..=0xDBFF).contains(&first)
        && let Some(second) = units.get(index as usize + 1).copied()
        && (0xDC00..=0xDFFF).contains(&second)
    {
        0x10000 + ((first as u32 - 0xD800) << 10) + (second as u32 - 0xDC00)
    } else {
        first as u32
    };
    Ok(JsValue::number(code as f64))
}

fn at(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let units: Vec<u16> = value.encode_utf16().collect();
    let length = units.len() as i64;
    let index = to_integer(arg_number(realm, arguments, 0)?);
    let index = if index < 0 { length + index } else { index };
    if !(0..length).contains(&index) {
        return Ok(JsValue::Undefined);
    }
    Ok(JsValue::string(String::from_utf16_lossy(
        &units[index as usize..index as usize + 1],
    )))
}

fn index_of(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let search = crate::vm::to_string(realm, arg(arguments, 0))?;
    let start = to_integer(arg_number_or(realm, arguments, 1, 0.0)?).max(0);
    if start as usize > crate::coerce::utf16_length(&value) {
        return Ok(JsValue::number(-1.0));
    }
    let start_byte = utf16_to_byte_offset(&value, start as usize);
    Ok(JsValue::number(
        value[start_byte..]
            .find(&search)
            .map(|index| byte_to_utf16_offset(&value, start_byte + index))
            .map(|index| index as f64)
            .unwrap_or(-1.0),
    ))
}

fn last_index_of(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let search = crate::vm::to_string(realm, arg(arguments, 0))?;
    let units = crate::coerce::utf16_length(&value);
    let end = if arguments.len() > 1 {
        to_integer(arg_number(realm, arguments, 1)?).clamp(0, i64::MAX)
    } else {
        units as i64
    } as usize;
    let end_byte = utf16_to_byte_offset(&value, end.min(units));
    Ok(JsValue::number(
        value[..end_byte]
            .rfind(&search)
            .map(|index| byte_to_utf16_offset(&value, index) as f64)
            .unwrap_or(-1.0),
    ))
}

fn includes(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let search = crate::vm::to_string(realm, arg(arguments, 0))?;
    let start = to_integer(arg_number_or(realm, arguments, 1, 0.0)?).max(0) as usize;
    let start_byte = utf16_to_byte_offset(&value, start);
    Ok(JsValue::Bool(
        start <= crate::coerce::utf16_length(&value) && value[start_byte..].contains(&search),
    ))
}

fn starts_with(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let search = crate::vm::to_string(realm, arg(arguments, 0))?;
    let start = to_integer(arg_number_or(realm, arguments, 1, 0.0)?).max(0) as usize;
    let start_byte = utf16_to_byte_offset(&value, start);
    Ok(JsValue::Bool(
        start <= crate::coerce::utf16_length(&value) && value[start_byte..].starts_with(&search),
    ))
}

fn ends_with(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let search = crate::vm::to_string(realm, arg(arguments, 0))?;
    let units = crate::coerce::utf16_length(&value);
    let end = if arguments.get(1).is_some_and(|value| !value.is_undefined()) {
        to_integer(arg_number(realm, arguments, 1)?).clamp(0, units as i64) as usize
    } else {
        units
    };
    let end_byte = utf16_to_byte_offset(&value, end);
    Ok(JsValue::Bool(value[..end_byte].ends_with(&search)))
}

fn slice(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let units: Vec<u16> = value.encode_utf16().collect();
    let length = units.len();
    let start = normalize_index(arg_number_or(realm, arguments, 0, 0.0)?, length);
    let end = normalize_index(arg_number_or(realm, arguments, 1, length as f64)?, length);
    if start >= end {
        return Ok(JsValue::string(""));
    }
    Ok(JsValue::string(String::from_utf16_lossy(
        &units[start..end],
    )))
}

fn substring(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let units: Vec<u16> = value.encode_utf16().collect();
    let length = units.len();
    let mut start =
        to_integer(arg_number_or(realm, arguments, 0, 0.0)?).clamp(0, length as i64) as usize;
    let mut end = if arguments.get(1).is_some_and(|value| !value.is_undefined()) {
        to_integer(arg_number(realm, arguments, 1)?).clamp(0, length as i64) as usize
    } else {
        length
    };
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    Ok(JsValue::string(String::from_utf16_lossy(
        &units[start..end],
    )))
}

fn substr(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let units: Vec<u16> = value.encode_utf16().collect();
    let length = units.len();
    let start = normalize_index(arg_number_or(realm, arguments, 0, 0.0)?, length);
    let count = if arguments.get(1).is_some_and(|value| !value.is_undefined()) {
        to_integer(arg_number(realm, arguments, 1)?).max(0) as usize
    } else {
        length - start
    };
    let end = (start + count).min(length);
    Ok(JsValue::string(String::from_utf16_lossy(
        &units[start..end],
    )))
}

fn to_upper_case(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(this_string(realm, this)?.to_uppercase()))
}

fn to_lower_case(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(this_string(realm, this)?.to_lowercase()))
}

fn trim(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::string(trim_ecma_whitespace(&this_string(
        realm, this,
    )?)))
}

fn trim_start(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    Ok(JsValue::string(
        value.trim_start_matches(is_ecma_whitespace),
    ))
}

fn trim_end(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    Ok(JsValue::string(value.trim_end_matches(is_ecma_whitespace)))
}

/// The ECMAScript TrimString set (ES2022 7.1.2): WhiteSpace plus
/// LineTerminators, unlike Rust's Unicode `char::is_whitespace`.
fn is_ecma_whitespace(ch: char) -> bool {
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
    let count = to_integer(arg_number(realm, arguments, 0)?);
    if count < 0 {
        return Err(realm.range_error("Invalid count value"));
    }
    let units = crate::coerce::utf16_length(&value);
    if count as usize * units > MAX_STRING_UNITS {
        return Err(realm.range_error("Invalid string length"));
    }
    Ok(JsValue::string(value.repeat(count as usize)))
}

fn pad_start(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    pad(realm, this, arguments, true)
}

fn pad_end(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    pad(realm, this, arguments, false)
}

fn pad(realm: &mut Realm, this: &JsValue, arguments: &[JsValue], start: bool) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let target = to_integer(arg_number(realm, arguments, 0)?).max(0) as usize;
    let fill = if arguments.len() > 1 && !arg(arguments, 1).is_undefined() {
        crate::vm::to_string(realm, arg(arguments, 1))?
    } else {
        " ".into()
    };
    let current = crate::coerce::utf16_length(&value);
    if target <= current || fill.is_empty() {
        return Ok(JsValue::string(value));
    }
    if target > MAX_STRING_UNITS {
        return Err(realm.range_error("Invalid string length"));
    }
    let needed = target - current;
    let fill_units: Vec<u16> = fill.encode_utf16().collect();
    let mut padding: Vec<u16> = Vec::with_capacity(needed);
    while padding.len() < needed {
        let remaining = needed - padding.len();
        padding.extend_from_slice(&fill_units[..remaining.min(fill_units.len())]);
    }
    let padding = String::from_utf16_lossy(&padding);
    Ok(JsValue::string(if start {
        padding + &value
    } else {
        value + &padding
    }))
}

fn split(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let separator = arg(arguments, 0);
    // ToUint32 of the limit: negative values wrap to huge positive numbers.
    let limit = if arguments.get(1).is_some_and(|value| !value.is_undefined()) {
        crate::vm::to_uint32(arg_number(realm, arguments, 1)?) as usize
    } else {
        usize::MAX
    };
    if limit == 0 {
        return Ok(JsValue::Object(realm.array(Vec::new())));
    }
    if separator.is_undefined() {
        return Ok(JsValue::Object(realm.array(vec![JsValue::string(value)])));
    }
    let parts = if let Some(regexp) = regexp_data(&separator) {
        split_regex(realm, &value, &regexp)?
    } else {
        let separator = crate::vm::to_string(realm, separator)?;
        if separator.is_empty() {
            // Split into single UTF-16 code units (ES2022 22.1.3.19).
            value
                .encode_utf16()
                .map(|unit| JsValue::string(String::from_utf16_lossy(&[unit])))
                .collect()
        } else {
            value
                .split(&separator)
                .map(|part| JsValue::string(part.to_owned()))
                .collect()
        }
    };
    Ok(JsValue::Object(
        realm.array(parts.into_iter().take(limit).collect()),
    ))
}

/// `RegExp.prototype[Symbol.split]` core (ES2022 22.2.5.13): capture groups
/// spliced between segments; an empty match at the current segment start
/// advances one code unit instead of splitting, exactly like `SplitMatch`.
fn split_regex(realm: &mut Realm, value: &str, regexp: &RegExpData) -> JsResult<Vec<JsValue>> {
    let mut result = Vec::new();
    let mut p = 0;
    let mut q = 0;
    while q < value.len() {
        let Some(matched) = regexp.regex.find_from(value, q).next() else {
            break;
        };
        let range = matched.range();
        // Anchor: only a match starting exactly at `q` can split.
        if range.start > q {
            q = range.start;
            continue;
        }
        if range.end == p {
            // Empty match at the segment start: advance one code unit.
            q += value[q..]
                .chars()
                .next()
                .map(|ch| ch.len_utf8())
                .unwrap_or(1);
            continue;
        }
        result.push(JsValue::string(value[p..range.start].to_owned()));
        for group in 1..=matched.captures.len() {
            result.push(
                matched
                    .group(group)
                    .map(|group| JsValue::string(value[group].to_owned()))
                    .unwrap_or(JsValue::Undefined),
            );
        }
        p = range.end;
        q = range.end;
    }
    result.push(JsValue::string(value[p..].to_owned()));
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
            let global = regexp.flags.contains('g');
            // `replaceAll` requires the global flag (ES2022 22.1.3.18).
            if all && !global {
                return Err(realm.type_error("replaceAll must be called with a global RegExp"));
            }
            return replace_regex(realm, &value, &regexp, replacement, global);
        }
    }
    let needle = crate::vm::to_string(realm, search)?;
    let mut result = String::new();
    let mut offset = 0;
    while offset <= value.len()
        && let Some(index) = value[offset..].find(&needle)
    {
        let absolute = offset + index;
        let replacement_value = if replacement.is_function() {
            super::call(
                realm,
                replacement.clone(),
                JsValue::Undefined,
                &[
                    JsValue::string(needle.clone()),
                    JsValue::number(byte_to_utf16_offset(&value, absolute) as f64),
                    JsValue::string(value.clone()),
                ],
            )?
        } else {
            JsValue::string(expand_replacement(
                &crate::vm::to_string(realm, replacement.clone())?,
                &needle,
                &[],
                absolute,
                &value,
            ))
        };
        result.push_str(&value[offset..absolute]);
        result.push_str(&crate::vm::to_string(realm, replacement_value)?);
        offset = absolute + needle.len();
        if offset == absolute {
            // Empty needle: copy one code unit and continue.
            let skip = value[offset..]
                .chars()
                .next()
                .map(|ch| ch.len_utf8())
                .unwrap_or(0);
            if skip == 0 {
                break;
            }
            result.push_str(&value[offset..offset + skip]);
            offset += skip;
        }
        if !all {
            break;
        }
    }
    result.push_str(&value[offset.min(value.len())..]);
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
        let groups: Vec<Option<std::ops::Range<usize>>> = (1..=matched.captures.len())
            .map(|group| matched.group(group))
            .collect();
        let replacement_value = if replacement.is_function() {
            let mut arguments = vec![JsValue::string(matched_text.to_owned())];
            for group in &groups {
                arguments.push(
                    group
                        .as_ref()
                        .map(|range| JsValue::string(value[range.clone()].to_owned()))
                        .unwrap_or(JsValue::Undefined),
                );
            }
            arguments.push(JsValue::number(
                byte_to_utf16_offset(value, range.start) as f64
            ));
            arguments.push(JsValue::string(value.to_owned()));
            super::call(realm, replacement.clone(), JsValue::Undefined, &arguments)?
        } else {
            JsValue::string(expand_replacement(
                &crate::vm::to_string(realm, replacement.clone())?,
                matched_text,
                &groups,
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

/// `GetSubstitution` (ES2022 22.1.3.17.1): `$&`, `` $` ``, `$'`, `$$` and
/// `$n` group references, mirroring `js_string___GetSubstitution`
/// (quickjs.c:47600).
fn expand_replacement(
    replacement: &str,
    matched: &str,
    groups: &[Option<std::ops::Range<usize>>],
    index: usize,
    value: &str,
) -> String {
    let bytes = replacement.as_bytes();
    let mut result = String::with_capacity(replacement.len());
    let mut position = 0;
    while position < bytes.len() {
        if bytes[position] != b'$' || position + 1 >= bytes.len() {
            result.push(bytes[position] as char);
            position += 1;
            continue;
        }
        let token = bytes[position + 1] as char;
        match token {
            '$' => {
                result.push('$');
                position += 2;
            }
            '&' => {
                result.push_str(matched);
                position += 2;
            }
            '`' => {
                result.push_str(&value[..index]);
                position += 2;
            }
            '\'' => {
                result.push_str(&value[index + matched.len()..]);
                position += 2;
            }
            '1'..='9' => {
                // `$n` or `$nn` group reference.
                let mut digits = token.to_digit(10).unwrap() as usize;
                let mut consumed = 2;
                if position + 2 < bytes.len()
                    && (bytes[position + 2] as char).is_ascii_digit()
                    && digits * 10 + (bytes[position + 2] - b'0') as usize <= groups.len()
                {
                    digits = digits * 10 + (bytes[position + 2] - b'0') as usize;
                    consumed = 3;
                }
                if digits <= groups.len() {
                    if let Some(range) = &groups[digits - 1] {
                        result.push_str(&value[range.clone()]);
                    }
                    position += consumed;
                } else {
                    result.push('$');
                    position += 1;
                }
            }
            _ => {
                result.push('$');
                position += 1;
            }
        }
    }
    result
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
        // A global match resets `lastIndex` before and after, but only on
        // a real RegExp argument (ES2022 22.2.5.2 `Symbol.match`).
        let regexp_object = arg(arguments, 0);
        let is_object = regexp_data(&regexp_object).is_some();
        if is_object {
            set(realm, &regexp_object, "lastIndex", JsValue::number(0.0))?;
        }
        for matched in regexp.regex.find_iter(&value) {
            let range = matched.range();
            matches.push(JsValue::string(value[range].to_owned()));
        }
        if is_object {
            set(realm, &regexp_object, "lastIndex", JsValue::number(0.0))?;
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
    define_value(
        &result,
        "index",
        JsValue::number(byte_to_utf16_offset(&value, range.start) as f64),
    );
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
                .map(|matched| byte_to_utf16_offset(&value, matched.range().start) as f64)
                .unwrap_or(-1.0),
        ));
    }
    let pattern = crate::vm::to_string(realm, arg(arguments, 0))?;
    let regex = regress::Regex::new(&pattern)
        .map_err(|error| realm.syntax_error(format!("Invalid regular expression: {error}")))?;
    Ok(JsValue::number(
        regex
            .find(&value)
            .map(|matched| byte_to_utf16_offset(&value, matched.range().start) as f64)
            .unwrap_or(-1.0),
    ))
}

fn locale_compare(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    let other = crate::vm::to_string(realm, arg(arguments, 0))?;
    Ok(JsValue::number(
        match crate::coerce::utf16_cmp(&value, &other) {
            std::cmp::Ordering::Less => -1.0,
            std::cmp::Ordering::Equal => 0.0,
            std::cmp::Ordering::Greater => 1.0,
        },
    ))
}

fn iterator_values(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = this_string(realm, this)?;
    Ok(JsValue::Object(
        realm.iterator(
            value
                .encode_utf16()
                .map(|unit| JsValue::string(String::from_utf16_lossy(&[unit])))
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
    let mut units: Vec<u16> = Vec::with_capacity(arguments.len());
    for argument in arguments {
        let number = crate::vm::to_number(realm, argument.clone())?;
        units.push(crate::vm::to_uint32(number) as u16);
    }
    Ok(JsValue::string(String::from_utf16_lossy(&units)))
}

fn from_code_point(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let mut units: Vec<u16> = Vec::with_capacity(arguments.len());
    for argument in arguments {
        let value = crate::vm::to_number(realm, argument.clone())?;
        if !value.is_finite() || value.fract() != 0.0 || !(0.0..=0x10FFFF as f64).contains(&value) {
            return Err(realm.range_error("Invalid code point"));
        }
        let code = value as u32;
        match char::from_u32(code) {
            Some(ch) => {
                let mut buffer = [0u16; 2];
                units.extend_from_slice(ch.encode_utf16(&mut buffer));
            }
            // Lone surrogates are valid code points; the engine stores the
            // replacement character for them.
            None => units.push(0xFFFD),
        }
    }
    Ok(JsValue::string(String::from_utf16_lossy(&units)))
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
