//! Property access: prototype-chain lookup, inline caches and elements.
//!
//! `get_value_property`/`set_value_property` correspond to
//! `JS_GetPropertyInternal` (quickjs.c:9110) and `JS_SetPropertyInternal`
//! (quickjs.c:10818); the caches mirror `JSPropCacheEntry`.

use std::rc::Rc;

use super::ValueStack;
use super::call::call_value;
use super::ops::pop;
use crate::object::{ObjectData, PropCache, PropKey, Property, ResolvedProperty};
use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};

/// QuickJS `JS_GetPropertyInternal` (quickjs.c:9110).
pub fn get_value_property(realm: &mut Realm, value: &JsValue, key: &PropKey) -> JsResult<JsValue> {
    match value {
        JsValue::Object(object) => {
            let resolved = object.borrow().resolve_property(key);
            match resolved {
                crate::object::ResolvedProperty::Data { value, .. } => Ok(value),
                crate::object::ResolvedProperty::Accessor {
                    get: Some(getter), ..
                } => call_value(realm, getter, value.clone(), &[]),
                crate::object::ResolvedProperty::Accessor { get: None, .. } => {
                    Ok(JsValue::Undefined)
                }
                crate::object::ResolvedProperty::Missing => Ok(JsValue::Undefined),
            }
        }
        JsValue::String(text_value) => {
            let text = text_value.as_ref();
            let index = match key {
                PropKey::Str(name) if name.as_ref() == "length" => {
                    return Ok(JsValue::number(crate::object::utf16_len(text) as f64));
                }
                // Element reads reach here as canonical `Index` keys as well
                // as numeric strings, like QuickJS' tagged-int atoms.
                other => other.index(),
            };
            if let Some(index) = index
                && let Some(ch) = crate::object::utf16_char(text, index)
            {
                return Ok(JsValue::string(ch));
            }
            let prototype = ObjectData::ordinary(Some(realm.string_proto.clone()));
            prototype.get_property(realm, key, value)
        }
        JsValue::Bool(_) => {
            let prototype = ObjectData::ordinary(Some(realm.boolean_proto.clone()));
            prototype.get_property(realm, key, value)
        }
        JsValue::Int(_) | JsValue::Float(_) => {
            let prototype = ObjectData::ordinary(Some(realm.number_proto.clone()));
            prototype.get_property(realm, key, value)
        }
        JsValue::Symbol(_) => {
            let prototype = ObjectData::ordinary(Some(realm.symbol_proto.clone()));
            prototype.get_property(realm, key, value)
        }
        JsValue::Undefined | JsValue::Null => Err(realm.type_error(format!(
            "Cannot read properties of {} (reading '{}')",
            if value.is_null() { "null" } else { "undefined" },
            display_key(key)
        ))),
    }
}

/// Monomorphic property get. Shape checks plus a weak holder reference make
/// the cached `IndexMap` slot valid without hashing the property name again.
#[inline]
pub(crate) fn cached_get_property(
    realm: &mut Realm,
    object: &JsValue,
    key: &PropKey,
    cache: &std::cell::RefCell<PropCache>,
) -> JsResult<JsValue> {
    if let JsValue::Object(object_ref) = object {
        let data = object_ref.borrow();
        let entry = cache.borrow();
        if entry.valid && entry.object_shape == data.shape {
            let cached = if entry.own {
                data.props
                    .get_index(entry.index as usize)
                    .filter(|(cached_key, _)| *cached_key == key)
                    .map(|(_, property)| (property.value.clone(), property.get.as_deref().cloned()))
            } else if entry.prototype_generation == crate::object::prototype_generation()
                && entry
                    .receiver_proto
                    .as_ref()
                    .and_then(std::rc::Weak::upgrade)
                    .is_some_and(|proto| {
                        data.prototype
                            .as_ref()
                            .is_some_and(|value| Rc::ptr_eq(value, &proto))
                    })
                && !data.props.contains_key(key)
            {
                entry
                    .holder
                    .as_ref()
                    .and_then(std::rc::Weak::upgrade)
                    .and_then(|holder| {
                        let holder_data = holder.borrow();
                        if holder_data.shape == entry.holder_shape {
                            holder_data
                                .props
                                .get_index(entry.index as usize)
                                .filter(|(cached_key, _)| *cached_key == key)
                                .map(|(_, property)| {
                                    (property.value.clone(), property.get.as_deref().cloned())
                                })
                        } else {
                            None
                        }
                    })
            } else {
                None
            };
            if let Some((value, getter)) = cached {
                drop(entry);
                drop(data);
                return match getter {
                    Some(getter) => call_value(realm, getter, object.clone(), &[]),
                    None => Ok(value),
                };
            }
        }
        drop(entry);
        drop(data);
    }
    let value = get_value_property(realm, object, key)?;
    if let JsValue::Object(object_ref) = object {
        let receiver = object_ref.borrow();
        if let Some(index) = receiver.props.get_index_of(key) {
            let mut entry = cache.borrow_mut();
            entry.object_shape = receiver.shape;
            entry.holder = None;
            entry.receiver_proto = None;
            entry.index = index as u32;
            entry.own = true;
            entry.valid = true;
        } else {
            let mut current = receiver.prototype.clone();
            while let Some(holder) = current {
                let (shape, index, next) = {
                    let data = holder.borrow();
                    (
                        data.shape,
                        data.props.get_index_of(key),
                        data.prototype.clone(),
                    )
                };
                holder.borrow_mut().is_prototype = true;
                if let Some(index) = index {
                    let mut entry = cache.borrow_mut();
                    entry.object_shape = receiver.shape;
                    entry.holder = Some(Rc::downgrade(&holder));
                    entry.holder_shape = shape;
                    entry.receiver_proto = receiver.prototype.as_ref().map(Rc::downgrade);
                    entry.prototype_generation = crate::object::prototype_generation();
                    entry.index = index as u32;
                    entry.own = false;
                    entry.valid = true;
                    break;
                }
                current = next;
            }
        }
    }
    Ok(value)
}

#[inline]
pub(crate) fn cached_set_property(
    realm: &mut Realm,
    object: &JsValue,
    key: &PropKey,
    value: &JsValue,
    cache: &std::cell::RefCell<PropCache>,
) -> JsResult<()> {
    if let JsValue::Object(object_ref) = object {
        let mut data = object_ref.borrow_mut();
        let entry = cache.borrow();
        if entry.valid {
            if entry.own && entry.object_shape == data.shape {
                let index = entry.index as usize;
                if data
                    .props
                    .get_index(index)
                    .is_some_and(|(cached_key, _)| cached_key == key)
                    && let Some((_, property)) = data.props.get_index_mut(index)
                {
                    if property.is_accessor() {
                        let setter = property.set.as_deref().cloned();
                        drop(entry);
                        drop(data);
                        if let Some(setter) = setter {
                            call_value(realm, setter, object.clone(), std::slice::from_ref(value))?;
                        }
                        return Ok(());
                    }
                    if property.writable {
                        property.value = value.clone();
                    }
                    return Ok(());
                }
            } else if entry.can_insert
                && data.extensible
                && entry.insert_shape == data.shape
                && !data.has_property(key)
            {
                data.define(key.clone(), Property::data(value.clone()));
                drop(entry);
                if let Some(index) = data.props.get_index_of(key) {
                    let mut entry = cache.borrow_mut();
                    entry.object_shape = data.shape;
                    entry.index = index as u32;
                    entry.own = true;
                    entry.valid = true;
                }
                return Ok(());
            }
        }
        drop(entry);
        drop(data);
    }
    let (shape_before, had_own) = match object {
        JsValue::Object(object_ref) => {
            let data = object_ref.borrow();
            (data.shape, data.props.contains_key(key))
        }
        _ => (0, false),
    };
    set_value_property(realm, object, key, value.clone(), true)?;
    if let JsValue::Object(object_ref) = object {
        let data = object_ref.borrow();
        if let Some(index) = data.props.get_index_of(key) {
            let mut entry = cache.borrow_mut();
            entry.object_shape = data.shape;
            entry.holder = None;
            entry.receiver_proto = None;
            entry.index = index as u32;
            entry.insert_shape = shape_before;
            entry.can_insert = !had_own;
            entry.own = true;
            entry.valid = true;
        }
    }
    Ok(())
}

/// Element access with the integer fast path used by `obj[index]`, like
/// `js_get_fast_array_element` (quickjs.c:9969).
#[inline]
pub(crate) fn get_element_value(
    realm: &mut Realm,
    object: &JsValue,
    key: JsValue,
) -> JsResult<JsValue> {
    let fast = match (object, &key) {
        (JsValue::Object(object), JsValue::Int(index)) => object.borrow().get_index_value(*index),
        _ => None,
    };
    match fast {
        Some(value) => Ok(value),
        None => {
            let key = super::to_property_key(realm, key)?;
            get_value_property(realm, object, &key)
        }
    }
}

/// Indexed write fast path, like `JS_SetPropertyValue` (quickjs.c:10830).
#[inline]
pub(crate) fn set_element_value(
    realm: &mut Realm,
    object: &JsValue,
    key: JsValue,
    value: JsValue,
) -> JsResult<()> {
    if let (JsValue::Object(object), JsValue::Int(index)) = (object, &key)
        && *index >= 0
        && object.borrow_mut().set_index_value(realm, *index, &value)
    {
        return Ok(());
    }
    let key = super::to_property_key(realm, key)?;
    set_value_property(realm, object, &key, value, true)
}

/// QuickJS `JS_SetPropertyInternal` (quickjs.c:10818).
pub fn set_value_property(
    realm: &mut Realm,
    value: &JsValue,
    key: &PropKey,
    new_value: JsValue,
    _strict: bool,
) -> JsResult<()> {
    match value {
        JsValue::Object(object) => {
            let canonical = key.canonical();
            let prototype = object.borrow().prototype.clone();
            let mut data = object.borrow_mut();
            if data.put_array_value(realm, key, &new_value)?.is_some() {
                return Ok(());
            }
            if let Some(property) = data.props.get_mut(&canonical) {
                if property.is_accessor() {
                    let setter = property.set.as_deref().cloned();
                    drop(data);
                    if let Some(setter) = setter {
                        call_value(realm, setter, value.clone(), &[new_value])?;
                    }
                    return Ok(());
                }
                if property.writable {
                    property.value = new_value;
                }
                return Ok(());
            }
            let extensible = data.extensible;
            drop(data);

            if let Some(prototype) = prototype
                && prototype.borrow().has_property(&canonical)
            {
                let resolved = prototype.borrow().resolve_property(&canonical);
                match resolved {
                    crate::object::ResolvedProperty::Accessor {
                        set: Some(setter), ..
                    } => {
                        call_value(realm, setter, value.clone(), &[new_value])?;
                    }
                    crate::object::ResolvedProperty::Accessor { set: None, .. } => {}
                    crate::object::ResolvedProperty::Data { writable, .. } => {
                        if writable && extensible {
                            object
                                .borrow_mut()
                                .define(canonical.clone(), Property::data(new_value));
                        }
                    }
                    crate::object::ResolvedProperty::Missing => {}
                }
                return Ok(());
            }
            if extensible {
                object
                    .borrow_mut()
                    .define(canonical, Property::data(new_value));
            }
            Ok(())
        }
        JsValue::Undefined | JsValue::Null => Err(realm.type_error(format!(
            "Cannot set properties of {} (setting '{}')",
            if value.is_null() { "null" } else { "undefined" },
            display_key(key)
        ))),
        JsValue::Symbol(_) => Ok(()),
        _ => Ok(()),
    }
}

pub(crate) fn display_key(key: &PropKey) -> String {
    match key {
        PropKey::Str(value) => value.to_string(),
        PropKey::Index(value) => value.to_string(),
        PropKey::Symbol(value) => format!("Symbol({value})"),
    }
}

/// Global lookup through the realm global object, like the
/// `OP_get_var` global fallback in `JS_CallInternal` (quickjs.c:18051).
/// Monomorphic global read cache, the counterpart of QuickJS' global
/// `JSPropCacheEntry` (`OP_get_var` resolves through the global object).
///
/// `None` still means "not defined", so callers can raise `ReferenceError`;
/// undefined values are cached like any other data property.
pub(crate) fn cached_global_get(
    realm: &mut Realm,
    name: &crate::string::JsString,
    cache: &std::cell::RefCell<PropCache>,
) -> JsResult<Option<JsValue>> {
    let key = PropKey::Str(name.clone());
    {
        let global = realm.global.borrow();
        let entry = cache.borrow();
        if entry.valid
            && entry.object_shape == global.shape
            && let Some((cached_key, property)) = global.props.get_index(entry.index as usize)
            && cached_key == &key
            && !property.is_accessor()
        {
            return Ok(Some(property.value.clone()));
        }
    }
    let value = global_get(realm, name)?;
    if value.is_some() {
        let global = realm.global.borrow();
        if let Some(index) = global.props.get_index_of(&key) {
            let mut entry = cache.borrow_mut();
            entry.object_shape = global.shape;
            entry.index = index as u32;
            entry.valid = true;
        }
    }
    Ok(value)
}

/// Global variable read (`OP_get_var` against the global object in
/// QuickJS). The name is the bytecode constant's `JsString`, so a hot global
/// read does not allocate or re-hash the text; only an own/prototype lookup
/// remains. Missing names return `None` so the caller can raise
/// `ReferenceError` (or return `undefined` for `typeof`).
pub(crate) fn global_get(
    realm: &mut Realm,
    name: &crate::string::JsString,
) -> JsResult<Option<JsValue>> {
    let key = PropKey::Str(name.clone());
    let resolved = { realm.global.borrow().resolve_property(&key) };
    match resolved {
        ResolvedProperty::Data { value, .. } => Ok(Some(value)),
        ResolvedProperty::Accessor { .. } => {
            let global = JsValue::Object(realm.global.clone());
            get_value_property(realm, &global, &key).map(Some)
        }
        ResolvedProperty::Missing => Ok(None),
    }
}

pub(crate) fn pop_property_key(realm: &mut Realm, stack: &mut ValueStack) -> JsResult<PropKey> {
    let value = pop(stack)?;
    super::to_property_key(realm, value)
}
