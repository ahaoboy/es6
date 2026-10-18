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
                    let next = super::property::get_value_property(
                        realm,
                        &iterator,
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
                    return Ok(items);
                }
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
