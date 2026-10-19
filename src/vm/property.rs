//! Property access: prototype-chain lookup, inline caches and elements.
//!
//! `get_value_property`/`set_value_property` correspond to
//! `JS_GetPropertyInternal` (quickjs.c:9110) and `JS_SetPropertyInternal`
//! (quickjs.c:10818); the caches mirror `JSPropCacheEntry`.

use super::ValueStack;
use super::call::call_value;
use super::ops::pop;
use crate::bytecode::{Constant, FunctionDef};
use crate::object::{PropCache, PropKey, Property, ResolvedProperty};
use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};

/// QuickJS `JS_GetPropertyInternal` (quickjs.c:9110).
pub fn get_value_property(realm: &mut Realm, value: &JsValue, key: &PropKey) -> JsResult<JsValue> {
    match value {
        JsValue::Object(object) => {
            let (resolved, needs_prototype) = {
                let data = object.borrow();
                let resolved = data.resolve_property(key);
                // Constructable functions materialize `.prototype` on first
                // access instead of at creation.
                let needs_prototype = matches!(resolved, ResolvedProperty::Missing)
                    && matches!(key, PropKey::Str(name) if name.as_ref() == "prototype")
                    && matches!(
                        &data.kind,
                        crate::object::ObjectKind::Function(function)
                            if function.is_constructable() && function.prototype.borrow().is_none()
                    );
                (resolved, needs_prototype)
            };
            if needs_prototype {
                let function = match &object.borrow().kind {
                    crate::object::ObjectKind::Function(function) => function.clone(),
                    _ => return Ok(JsValue::Undefined),
                };
                realm.function_prototype(object, &function);
                return get_value_property(realm, value, key);
            }
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
            // Walk the primitive's prototype directly; no temporary wrapper
            // object is allocated, like QuickJS' exotic string handling in
            // `JS_GetPropertyInternal` (quickjs.c:9110).
            let prototype = realm.string_proto.clone();
            super::call::get_property_receiver(realm, &JsValue::Object(prototype), key, value)
        }
        JsValue::Bool(_) => {
            let prototype = realm.boolean_proto.clone();
            super::call::get_property_receiver(realm, &JsValue::Object(prototype), key, value)
        }
        JsValue::Int(_) | JsValue::Float(_) => {
            let prototype = realm.number_proto.clone();
            super::call::get_property_receiver(realm, &JsValue::Object(prototype), key, value)
        }
        JsValue::Symbol(_) => {
            let prototype = realm.symbol_proto.clone();
            super::call::get_property_receiver(realm, &JsValue::Object(prototype), key, value)
        }
        JsValue::Undefined | JsValue::Null => Err(realm.type_error(format!(
            "Cannot read properties of {} (reading '{}')",
            if value.is_null() { "null" } else { "undefined" },
            display_key(key)
        ))),
    }
}

/// Constant-key get with a single cache borrow: the site key lives in the
/// cache, so a hit never rebuilds or clones it. QuickJS keeps the same
/// pre-interned atom plus property offset in JSPropCacheEntry.
#[inline]
pub(crate) fn cached_get_const(
    realm: &mut Realm,
    object: &JsValue,
    definition: &crate::bytecode::FunctionDef,
    index: u32,
    cache: &std::cell::RefCell<PropCache>,
) -> JsResult<JsValue> {
    if let JsValue::Object(object_ref) = object {
        let data = object_ref.borrow();
        let entry = cache.borrow();
        let shape = data.shape_id();
        let cached = if let Some(slot) = entry.poly_own(shape) {
            data.props
                .get(slot as usize)
                .map(|property| (property.value.clone(), property.get.as_deref().cloned()))
        } else if entry.valid
            && !entry.own
            && (entry.object_shape == shape || entry.poly_proto(shape))
            && entry.prototype_generation == crate::object::prototype_generation()
            && data.prototype.as_ref().map(|proto| {
                proto.as_ref() as *const boa_gc::GcRefCell<crate::object::ObjectData> as usize
            }) == entry.receiver_proto
        {
            read_proto_holder(
                &data,
                entry.holder_depth,
                entry.holder_shape,
                entry.index,
                None,
            )
        } else if entry.valid && entry.own && entry.object_shape == shape {
            data.props
                .get(entry.index as usize)
                .map(|property| (property.value.clone(), property.get.as_deref().cloned()))
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
    let key = property_key_from_constant(definition, index)?;
    cache.borrow_mut().key = Some(key.clone());
    cached_get_property(realm, object, &key, cache)
}

/// Constant-key set with the same single-borrow fast path.
#[inline]
pub(crate) fn cached_set_const(
    realm: &mut Realm,
    object: &JsValue,
    definition: &crate::bytecode::FunctionDef,
    index: u32,
    value: &JsValue,
    cache: &std::cell::RefCell<PropCache>,
) -> JsResult<()> {
    if let JsValue::Object(object_ref) = object {
        let mut data = object_ref.borrow_mut();
        let entry = cache.borrow();
        let shape = data.shape_id();
        let slot = if let Some(slot) = entry.poly_own(shape) {
            Some(slot)
        } else if entry.valid && entry.own && entry.object_shape == shape {
            Some(entry.index)
        } else {
            None
        };
        if let Some(slot) = slot
            && let Some(property) = data.props.get_mut(slot as usize)
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
    }
    let key = property_key_from_constant(definition, index)?;
    cache.borrow_mut().key = Some(key.clone());
    cached_set_property(realm, object, &key, value, cache)
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
        if entry.valid && entry.object_shape == data.shape_id() {
            let cached = if entry.own {
                if data.shape.keys.get(entry.index as usize) == Some(key) {
                    data.props
                        .get(entry.index as usize)
                        .map(|property| (property.value.clone(), property.get.as_deref().cloned()))
                } else {
                    None
                }
            } else if entry.prototype_generation == crate::object::prototype_generation()
                && data.prototype.as_ref().map(|proto| {
                    proto.as_ref() as *const boa_gc::GcRefCell<crate::object::ObjectData> as usize
                }) == entry.receiver_proto
            {
                read_proto_holder(
                    &data,
                    entry.holder_depth,
                    entry.holder_shape,
                    entry.index,
                    Some(key),
                )
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
        if let Some(index) = receiver.lookup(key) {
            let mut entry = cache.borrow_mut();
            entry.object_shape = receiver.shape_id();
            entry.index = index;
            entry.own = true;
            entry.valid = true;
            entry.remember_own(receiver.shape_id(), index);
        } else {
            let mut current = receiver.prototype.clone();
            let mut depth = 0;
            while let Some(holder) = current {
                let (shape, index, next) = {
                    let data = holder.borrow();
                    (data.shape_id(), data.lookup(key), data.prototype.clone())
                };
                holder.borrow_mut().is_prototype = true;
                if let Some(index) = index {
                    let mut entry = cache.borrow_mut();
                    entry.object_shape = receiver.shape_id();
                    entry.holder_shape = shape;
                    entry.holder_depth = depth;
                    entry.receiver_proto = receiver.prototype.as_ref().map(|proto| {
                        proto.as_ref() as *const boa_gc::GcRefCell<crate::object::ObjectData>
                            as usize
                    });
                    entry.prototype_generation = crate::object::prototype_generation();
                    entry.index = index;
                    entry.own = false;
                    entry.valid = true;
                    entry.remember_proto(receiver.shape_id());
                    break;
                }
                current = next;
                depth += 1;
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
            if entry.own && entry.object_shape == data.shape_id() {
                let index = entry.index as usize;
                if data.shape.keys.get(index) == Some(key)
                    && let Some(property) = data.props.get_mut(index)
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
                && entry.insert_shape == data.shape_id()
                && entry.prototype_generation == crate::object::prototype_generation()
                && let Some(target) = entry.insert_target.clone()
            {
                // Reuse the interned shape transition prepared by the first
                // insertion at this site instead of hashing the key again.
                let index = entry.insert_index;
                if index as usize == data.props.len()
                    && target.keys.len() == data.props.len() + 1
                    && target.keys.get(index as usize) == Some(key)
                {
                    data.shape = target;
                    if data.is_prototype {
                        crate::object::bump_prototype_generation();
                    }
                    data.props.push(Property::data(value.clone()));
                    let shape = data.shape_id();
                    drop(entry);
                    let mut entry = cache.borrow_mut();
                    entry.object_shape = shape;
                    entry.index = index;
                    entry.own = true;
                    entry.valid = true;
                    entry.remember_own(shape, index);
                    return Ok(());
                }
            }
        }
        drop(entry);
        drop(data);
    }
    let (shape_before, had_own) = match object {
        JsValue::Object(object_ref) => {
            let data = object_ref.borrow();
            (data.shape_id(), data.lookup(key).is_some())
        }
        _ => (0, false),
    };
    set_value_property(realm, object, key, value.clone(), true)?;
    if let JsValue::Object(object_ref) = object {
        let data = object_ref.borrow();
        if let Some(index) = data.lookup(key) {
            let mut entry = cache.borrow_mut();
            entry.object_shape = data.shape_id();
            entry.index = index;
            entry.insert_shape = shape_before;
            entry.insert_target = (!had_own).then(|| data.shape.clone());
            entry.insert_index = index;
            entry.prototype_generation = crate::object::prototype_generation();
            entry.can_insert = !had_own;
            entry.own = true;
            entry.valid = true;
            entry.remember_own(data.shape_id(), index);
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
            if let Some(slot) = data.lookup(&canonical) {
                if data.props[slot as usize].is_accessor() {
                    let setter = data.props[slot as usize].set.as_deref().cloned();
                    drop(data);
                    if let Some(setter) = setter {
                        call_value(realm, setter, value.clone(), &[new_value])?;
                    }
                    return Ok(());
                }
                if data.props[slot as usize].writable {
                    data.props[slot as usize].value = new_value;
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
    definition: &FunctionDef,
    name_index: u32,
    cache: &std::cell::RefCell<PropCache>,
) -> JsResult<Option<JsValue>> {
    // The site's name is fixed by the bytecode and the shape id guarantees
    // the same slot layout, so a hit needs neither the name constant nor a
    // key comparison (QuickJS' global var cache does the same).
    {
        let global = realm.global.borrow();
        let entry = cache.borrow();
        if entry.valid
            && entry.object_shape == global.shape_id()
            && let Some(property) = global.props.get(entry.index as usize)
            && !property.is_accessor()
        {
            return Ok(Some(property.value.clone()));
        }
    }
    let name = super::constant_string(definition, name_index);
    let value = global_get(realm, &name)?;
    if value.is_some() {
        let global = realm.global.borrow();
        let key = PropKey::Str(name);
        if let Some(index) = global.lookup(&key) {
            let mut entry = cache.borrow_mut();
            entry.object_shape = global.shape_id();
            entry.index = index;
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

pub(crate) fn property_key_from_constant(
    definition: &FunctionDef,
    index: u32,
) -> JsResult<PropKey> {
    match definition.constants.get(index as usize) {
        Some(Constant::String(value)) => Ok(crate::object::key_from_string(value.clone())),
        Some(Constant::Int(value)) => Ok(PropKey::from(value.to_string())),
        Some(Constant::Number(value)) => Ok(PropKey::from(super::format_number(*value))),
        _ => Err(JsValue::string("InternalError: invalid property constant")),
    }
}

fn read_proto_holder(
    receiver: &crate::object::ObjectData,
    depth: u32,
    holder_shape: u64,
    index: u32,
    key: Option<&crate::object::PropKey>,
) -> Option<(JsValue, Option<JsValue>)> {
    fn walk(
        cell: &boa_gc::GcRefCell<crate::object::ObjectData>,
        depth: u32,
        holder_shape: u64,
        index: u32,
        key: Option<&crate::object::PropKey>,
    ) -> Option<(JsValue, Option<JsValue>)> {
        let data = cell.borrow();
        if depth == 0 {
            let key_matches =
                key.is_none_or(|key| data.shape.keys.get(index as usize) == Some(key));
            return (data.shape_id() == holder_shape && key_matches)
                .then(|| {
                    data.props
                        .get(index as usize)
                        .map(|property| (property.value.clone(), property.get.as_deref().cloned()))
                })
                .flatten();
        }
        let next = data.prototype.as_ref()?;
        walk(next.as_ref(), depth - 1, holder_shape, index, key)
    }
    let first = receiver.prototype.as_ref()?;
    walk(first.as_ref(), depth, holder_shape, index, key)
}
