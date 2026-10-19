use boa_gc::{Gc, GcRefCell};

use super::*;
use crate::object::{MapKey, ObjectData, ObjectKind};
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicMapSet / js_map_* (quickjs.c). Keys use
// SameValueZero hashing, so lookup is O(1) and iteration preserves
// insertion order.
pub fn install(realm: &mut Realm) {
    let prototype = realm.map_proto.clone();
    install_constructor(realm, "Map", 0, constructor, &prototype);
    define_method(realm, &prototype, "set", 2, set);
    define_method(realm, &prototype, "get", 1, get);
    define_method(realm, &prototype, "has", 1, has);
    define_method(realm, &prototype, "delete", 1, delete);
    define_method(realm, &prototype, "clear", 0, clear);
    define_method(realm, &prototype, "forEach", 1, for_each);
    define_method(realm, &prototype, "keys", 0, keys);
    define_method(realm, &prototype, "values", 0, values);
    define_method(realm, &prototype, "entries", 0, entries);
    define_accessor(realm, &prototype, "size", size, None);
    let iterator = realm.well_known_symbol("iterator");
    define_method_symbol(realm, &prototype, iterator, "[Symbol.iterator]", 0, entries);
}

fn constructor(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = Gc::new(GcRefCell::new(ObjectData::map(
        Some(realm.map_proto.clone()),
        Vec::new(),
    )));
    let iterable = arg(arguments, 0);
    if !iterable.is_nullish() {
        let entries = crate::vm::iterable_items(realm, iterable)?;
        let mut data = object.borrow_mut();
        if let ObjectKind::Map(map) = &mut data.kind {
            for entry in entries {
                let values = crate::vm::iterable_items(realm, entry)?;
                if values.len() >= 2 {
                    map.insert(MapKey(values[0].clone()), values[1].clone());
                }
            }
        }
    }
    Ok(JsValue::Object(object))
}

fn with_map(realm: &Realm, this: &JsValue) -> JsResult<crate::object::ObjRef> {
    let object = this
        .as_object()
        .ok_or_else(|| realm.type_error("Map method called on incompatible receiver"))?;
    let reference = object.to_ref();
    if matches!(reference.borrow().kind, ObjectKind::Map(_)) {
        Ok(reference)
    } else {
        Err(realm.type_error("Map method called on incompatible receiver"))
    }
}

fn set(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let mut data = object.borrow_mut();
    let key = MapKey(arg(arguments, 0));
    let value = arg(arguments, 1);
    if let ObjectKind::Map(entries) = &mut data.kind {
        entries.insert(key, value);
    }
    drop(data);
    Ok(this.clone())
}

fn get(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let data = object.borrow();
    if let ObjectKind::Map(entries) = &data.kind {
        return Ok(entries
            .get(&MapKey(arg(arguments, 0)))
            .cloned()
            .unwrap_or(JsValue::Undefined));
    }
    Ok(JsValue::Undefined)
}

fn has(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let data = object.borrow();
    if let ObjectKind::Map(entries) = &data.kind {
        return Ok(JsValue::Bool(
            entries.contains_key(&MapKey(arg(arguments, 0))),
        ));
    }
    Ok(JsValue::Bool(false))
}

fn delete(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let mut data = object.borrow_mut();
    if let ObjectKind::Map(entries) = &mut data.kind {
        return Ok(JsValue::Bool(
            entries.shift_remove(&MapKey(arg(arguments, 0))).is_some(),
        ));
    }
    Ok(JsValue::Bool(false))
}

fn clear(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let mut data = object.borrow_mut();
    if let ObjectKind::Map(entries) = &mut data.kind {
        entries.clear();
    }
    Ok(JsValue::Undefined)
}

fn for_each(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let callback = arg(arguments, 0);
    if !callback.is_function() {
        return Err(realm.type_error("Map.prototype.forEach callback is not a function"));
    }
    let entries: Vec<(MapKey, JsValue)> = {
        let data = object.borrow();
        match &data.kind {
            ObjectKind::Map(entries) => entries
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            _ => Vec::new(),
        }
    };
    for (key, value) in entries {
        super::call(
            realm,
            callback.clone(),
            arg(arguments, 1),
            &[value, key.0, this.clone()],
        )?;
    }
    Ok(JsValue::Undefined)
}

fn keys(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let keys: Vec<JsValue> = {
        let data = object.borrow();
        match &data.kind {
            ObjectKind::Map(entries) => entries.keys().map(|key| key.0.clone()).collect(),
            _ => Vec::new(),
        }
    };
    Ok(JsValue::Object(realm.iterator(keys)))
}

fn values(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let values: Vec<JsValue> = {
        let data = object.borrow();
        match &data.kind {
            ObjectKind::Map(entries) => entries.values().cloned().collect(),
            _ => Vec::new(),
        }
    };
    Ok(JsValue::Object(realm.iterator(values)))
}

fn entries(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let entries: Vec<JsValue> = {
        let data = object.borrow();
        match &data.kind {
            ObjectKind::Map(entries) => entries
                .iter()
                .map(|(key, value)| {
                    JsValue::Object(realm.array(vec![key.0.clone(), value.clone()]))
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    Ok(JsValue::Object(realm.iterator(entries)))
}

fn size(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let data = object.borrow();
    if let ObjectKind::Map(entries) = &data.kind {
        return Ok(JsValue::number(entries.len() as f64));
    }
    Ok(JsValue::number(0.0))
}
