//! Iteration protocol helpers.
//!
//! `iterable_items` drains an iterable eagerly for spread calls and array
//! literals; QuickJS instead drives `Symbol.iterator` lazily with
//! `OP_for_of_start`/`OP_for_of_next` (quickjs.c:19318) for loops.
use std::rc::Rc;

use super::call::call_value;
use crate::object::{ObjectKind, PropKey};
use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};

/// Fully drain an iterable. Used by spread arguments, array literals and
/// destructuring rest elements. Corresponds to
/// `js_append_enumerate`-style eager consumption; `for-of` loops use the
/// lazy `IterNext` opcode instead (quickjs.c:19318).
pub fn iterable_items(realm: &mut Realm, value: JsValue) -> JsResult<Vec<JsValue>> {
    match &value {
        JsValue::String(text) => Ok(text.chars().map(JsValue::string).collect()),
        JsValue::Object(object) => {
            if let Some(items) = object.borrow().array_items().map(|items| items.to_vec()) {
                return Ok(items);
            }
            let data = object.borrow();
            if let ObjectKind::Iterator { items, .. } = &data.kind {
                return Ok(items.clone());
            }
            if let ObjectKind::Map(entries) = &data.kind {
                let entries = entries.clone();
                drop(data);
                return Ok(entries
                    .into_iter()
                    .map(|(key, value)| JsValue::Object(realm.array(vec![key, value])))
                    .collect());
            }
            if let ObjectKind::Set(values) = &data.kind {
                return Ok(values.clone());
            }
            if let ObjectKind::TypedArray(values) = &data.kind {
                return Ok(values.values());
            }
            drop(data);
            let iterator_key = PropKey::Symbol(realm.well_known_symbol("iterator"));
            if object.borrow().has_property(&iterator_key) {
                let method =
                    super::property::get_value_property(realm, &value, &iterator_key)?;
                if method.is_function() {
                    let iterator = call_value(realm, method, value.clone(), &[])?;
                    return drain_protocol_iterator(realm, &iterator);
                }
            }
            // Spread accepts a bare iterator too: any object with a callable
            // `next` (ES2022 13.2.8.5 IteratorToList).
            let next = super::property::get_value_property(
                realm,
                &value,
                &PropKey::from("next"),
            )?;
            if next.is_function() {
                return drain_protocol_iterator(realm, &value);
            }
            let length = super::property::get_value_property(
                realm,
                &value,
                &PropKey::from("length"),
            )?;
            if length.is_undefined() {
                return Err(realm.type_error("Value is not iterable"));
            }
            let length = super::to_uint32(super::to_number(realm, length)?) as usize;
            let mut items = Vec::with_capacity(length);
            for index in 0..length {
                items.push(super::property::get_value_property(
                    realm,
                    &value,
                    &PropKey::Str(Rc::from(index.to_string())),
                )?);
            }
            Ok(items)
        }
        _ => Err(realm.type_error("Value is not iterable")),
    }
}

/// Drain an iterator through its `next` method until done
/// (ES2022 13.2.8.5 `IteratorToList`).
fn drain_protocol_iterator(realm: &mut Realm, iterator: &JsValue) -> JsResult<Vec<JsValue>> {
    let next = super::property::get_value_property(
        realm,
        iterator,
        &PropKey::from("next"),
    )?;
    if !next.is_function() {
        return Err(realm.type_error("Iterator has no next method"));
    }
    let mut items = Vec::new();
    loop {
        let result = call_value(realm, next.clone(), iterator.clone(), &[])?;
        let done = super::property::get_value_property(
            realm,
            &result,
            &PropKey::from("done"),
        )?;
        if super::to_boolean(&done) {
            break;
        }
        items.push(super::property::get_value_property(
            realm,
            &result,
            &PropKey::from("value"),
        )?);
    }
    Ok(items)
}

/// `GetIterator` (ES2022 7.4.1): call `Symbol.iterator` and require a
/// callable `next`.
pub(crate) fn protocol_iterator(realm: &mut Realm, value: &JsValue) -> JsResult<JsValue> {
    let iterator_key = PropKey::Symbol(realm.well_known_symbol("iterator"));
    let method =
        super::property::get_value_property(realm, value, &iterator_key)?;
    if !method.is_function() {
        return Err(realm.type_error("Value is not iterable"));
    }
    let iterator = super::call::call_value(realm, method, value.clone(), &[])?;
    let next = super::property::get_value_property(
        realm,
        &iterator,
        &PropKey::from("next"),
    )?;
    if !next.is_function() {
        return Err(realm.type_error("Iterator has no next method"));
    }
    Ok(iterator)
}

/// `IteratorStep` + `IteratorValue` (ES2022 7.4.2/7.4.4): pull one result.
/// Engine-internal iterators take the fast path, like QuickJS' checks for
/// its own iterator classes; everything else runs the protocol.
pub(crate) fn advance_iterator(realm: &mut Realm, iterator: &JsValue) -> JsResult<Option<JsValue>> {
    if let JsValue::Object(object) = iterator {
        // Fast paths: no user code can run.
        let result = {
            let mut data = object.borrow_mut();
            match &mut data.kind {
                ObjectKind::Iterator { items, index } => {
                    if *index < items.len() {
                        let value = items[*index].clone();
                        *index += 1;
                        Some(Some(value))
                    } else {
                        Some(None)
                    }
                }
                ObjectKind::ArrayIterator { array, index, entries } => {
                    let array = array.clone();
                    let current = *index;
                    let entries = *entries;
                    Some(array_iterator_step(realm, &array, current, entries, index))
                }
                ObjectKind::StringIterator { text, index } => {
                    let units = crate::coerce::utf16_length(text);
                    if *index < units {
                        let unit = crate::object::utf16_char(text, *index);
                        *index += 1;
                        Some(Some(JsValue::string(unit.unwrap_or_default())))
                    } else {
                        Some(None)
                    }
                }
                ObjectKind::Generator(_) => None,
                _ => None,
            }
        };
        if let Some(value) = result {
            return Ok(value);
        }
        let is_generator = matches!(object.borrow().kind, ObjectKind::Generator(_));
        if is_generator {
            drop(object);
            // Generators resume through `next`; the result object feeds the
            // same `done`/`value` protocol.
            let result = super::generator::generator_next(realm, iterator, &[])?;
            let done = super::property::get_value_property(
                realm,
                &result,
                &PropKey::from("done"),
            )?;
            if super::to_boolean(&done) {
                return Ok(None);
            }
            return super::property::get_value_property(
                realm,
                &result,
                &PropKey::from("value"),
            )
            .map(Some);
        }
    }
    // Protocol path: `iterator.next()` then `done`/`value`.
    let next = super::property::get_value_property(
        realm,
        iterator,
        &PropKey::from("next"),
    )?;
    if !next.is_function() {
        return Err(realm.type_error("Iterator has no next method"));
    }
    let result = super::call::call_value(realm, next, iterator.clone(), &[])?;
    let done = super::property::get_value_property(realm, &result, &PropKey::from("done"))?;
    if super::to_boolean(&done) {
        return Ok(None);
    }
    super::property::get_value_property(realm, &result, &PropKey::from("value"))
        .map(Some)
}

/// One step of the live array iterator (ES2022 23.1.5.1.1). Advances the
/// index in place and returns the next value.
fn array_iterator_step(
    realm: &mut Realm,
    array: &crate::object::ObjRef,
    current: usize,
    entries: bool,
    index: &mut usize,
) -> Option<JsValue> {
    let length = array.borrow().array_len().unwrap_or(0);
    if current >= length {
        return None;
    }
    let value = array
        .borrow()
        .get_index_value(current as i32)
        .unwrap_or(JsValue::Undefined);
    *index += 1;
    if entries {
        Some(JsValue::Object(
            realm.array(vec![JsValue::number(current as f64), value]),
        ))
    } else {
        Some(value)
    }
}

pub(crate) fn push_array_value(
    realm: &mut Realm,
    array: &crate::object::ObjRef,
    value: JsValue,
) -> JsResult<()> {
    let mut data = array.borrow_mut();
    if let ObjectKind::Array(items) = &mut data.kind {
        items.items.push(Some(value));
        items.length = items.items.len();
        Ok(())
    } else {
        drop(data);
        Err(realm.type_error("Array expected"))
    }
}

pub(crate) fn copy_own_properties(
    realm: &mut Realm,
    target: &crate::object::ObjRef,
    source: &crate::object::ObjRef,
) -> JsResult<()> {
    let keys = source.borrow().enumerable_keys();
    for key in keys {
        let value = source
            .borrow()
            .get_property(realm, &key, &JsValue::Object(source.clone()))?;
        target
            .borrow_mut()
            .define(key, crate::object::Property::data(value));
    }
    Ok(())
}
