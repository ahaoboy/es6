//! Weak collections: `WeakMap` and `WeakSet` (ES2022 25.3/25.4).
//!
//! Keys are ephemeron keys in Boa's collector: an entry's value is reachable
//! only while the key is reachable, matching the weak-map semantics QuickJS
//! implements with its weak reference lists.

use boa_gc::{Gc, GcRefCell};

use super::*;
use crate::object::{ObjectData, ObjectKind, WeakObjectMap, WeakObjectSet};
use crate::value::JsValue;

pub fn install(realm: &mut Realm) {
    let prototype = realm.weak_map_proto.clone();
    let constructor = install_constructor(realm, "WeakMap", 0, weak_map, &prototype);
    let _ = constructor;
    define_method(realm, &prototype, "get", 1, get);
    define_method(realm, &prototype, "set", 2, set);
    define_method(realm, &prototype, "has", 1, has);
    define_method(realm, &prototype, "delete", 1, delete);

    let set_prototype = realm.weak_set_proto.clone();
    let set_constructor = install_constructor(realm, "WeakSet", 0, weak_set, &set_prototype);
    let _ = set_constructor;
    define_method(realm, &set_prototype, "add", 1, set_add);
    define_method(realm, &set_prototype, "has", 1, set_has);
    define_method(realm, &set_prototype, "delete", 1, set_delete);
}

fn weak_map(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = Gc::new(GcRefCell::new(ObjectData {
        props: Default::default(),
        prototype: Some(realm.weak_map_proto.clone()),
        kind: ObjectKind::WeakMap(WeakObjectMap::new()),
        extensible: true,
        class_name: "WeakMap",
        shape: crate::object::empty_shape(),
        is_prototype: false,
    }));
    let iterable = arg(arguments, 0);
    if !iterable.is_nullish() {
        let entries = crate::vm::iterable_items(realm, iterable)?;
        let mut data = object.borrow_mut();
        if let ObjectKind::WeakMap(map) = &mut data.kind {
            for entry in entries {
                let pair = crate::vm::iterable_items(realm, entry)?;
                if pair.len() >= 2
                    && let JsValue::Object(key) = &pair[0]
                {
                    map.insert(key, pair[1].clone());
                }
            }
        }
    }
    Ok(JsValue::Object(object))
}

fn weak_set(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = Gc::new(GcRefCell::new(ObjectData {
        props: Default::default(),
        prototype: Some(realm.weak_set_proto.clone()),
        kind: ObjectKind::WeakSet(WeakObjectSet::new()),
        extensible: true,
        class_name: "WeakSet",
        shape: crate::object::empty_shape(),
        is_prototype: false,
    }));
    let iterable = arg(arguments, 0);
    if !iterable.is_nullish() {
        let values = crate::vm::iterable_items(realm, iterable)?;
        let mut data = object.borrow_mut();
        if let ObjectKind::WeakSet(set) = &mut data.kind {
            for value in values {
                if let JsValue::Object(key) = value {
                    set.insert(&key, ());
                }
            }
        }
    }
    Ok(JsValue::Object(object))
}

fn require_key(realm: &Realm, value: &JsValue) -> JsResult<crate::object::ObjRef> {
    match value {
        JsValue::Object(object) => Ok(object.clone()),
        _ => Err(realm.type_error("Invalid value used as weak collection key")),
    }
}

fn get(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = this
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| realm.type_error("WeakMap method called on incompatible receiver"))?;
    let key = require_key(realm, &arg(arguments, 0))?;
    let data = object.borrow();
    match &data.kind {
        ObjectKind::WeakMap(map) => Ok(map
            .get(&key)
            .and_then(|entry| entry.value().map(|value| (*value).clone()))
            .unwrap_or(JsValue::Undefined)),
        _ => Err(realm.type_error("WeakMap method called on incompatible receiver")),
    }
}

fn set(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = this
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| realm.type_error("WeakMap method called on incompatible receiver"))?;
    let key = require_key(realm, &arg(arguments, 0))?;
    let value = arg(arguments, 1);
    let mut data = object.borrow_mut();
    match &mut data.kind {
        ObjectKind::WeakMap(map) => {
            map.insert(&key, value);
            drop(data);
            Ok(this.clone())
        }
        _ => Err(realm.type_error("WeakMap method called on incompatible receiver")),
    }
}

fn has(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = this
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| {
            realm.type_error("Weak collection method called on incompatible receiver")
        })?;
    let key = require_key(realm, &arg(arguments, 0))?;
    let data = object.borrow();
    let found = match &data.kind {
        ObjectKind::WeakMap(map) => map.contains_key(&key),
        ObjectKind::WeakSet(set) => set.contains_key(&key),
        _ => {
            return Err(realm.type_error("Weak collection method called on incompatible receiver"));
        }
    };
    Ok(JsValue::Bool(found))
}

fn delete(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = this
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| {
            realm.type_error("Weak collection method called on incompatible receiver")
        })?;
    let key = require_key(realm, &arg(arguments, 0))?;
    let mut data = object.borrow_mut();
    let removed = match &mut data.kind {
        ObjectKind::WeakMap(map) => map.remove(&key),
        ObjectKind::WeakSet(set) => set.remove(&key),
        _ => {
            return Err(realm.type_error("Weak collection method called on incompatible receiver"));
        }
    };
    Ok(JsValue::Bool(removed))
}

fn set_add(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = this
        .as_object()
        .map(|object| object.into_ref())
        .ok_or_else(|| realm.type_error("WeakSet method called on incompatible receiver"))?;
    let key = require_key(realm, &arg(arguments, 0))?;
    let mut data = object.borrow_mut();
    match &mut data.kind {
        ObjectKind::WeakSet(set) => {
            set.insert(&key, ());
            drop(data);
            Ok(this.clone())
        }
        _ => Err(realm.type_error("WeakSet method called on incompatible receiver")),
    }
}

fn set_has(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    has(realm, this, arguments)
}

fn set_delete(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    delete(realm, this, arguments)
}
