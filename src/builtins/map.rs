use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::object::{ObjectData, ObjectKind};
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicMapSet / js_map_* (quickjs.c).
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
    let object = Rc::new(RefCell::new(ObjectData::map(
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
                    // Set-semantics: SameValueZero replaces existing keys
                    // (ES2022 24.1.1.1).
                    if let Some(existing) = map
                        .iter_mut()
                        .find(|(key, _)| crate::vm::same_value_zero(key, &values[0]))
                    {
                        existing.1 = values[1].clone();
                    } else {
                        map.push((values[0].clone(), values[1].clone()));
                    }
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
    let key = arg(arguments, 0);
    let value = arg(arguments, 1);
    if let ObjectKind::Map(entries) = &mut data.kind {
        if let Some(entry) = entries
            .iter_mut()
            .find(|(entry_key, _)| crate::vm::same_value_zero(entry_key, &key))
        {
            entry.1 = value;
        } else {
            entries.push((key, value));
        }
    }
    Ok(this.clone())
}

fn get(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let key = arg(arguments, 0);
    let object = with_map(realm, this)?;
    let data = object.borrow();
    if let ObjectKind::Map(entries) = &data.kind {
        Ok(entries
            .iter()
            .find(|(entry_key, _)| crate::vm::same_value_zero(entry_key, &key))
            .map(|(_, value)| value.clone())
            .unwrap_or(JsValue::Undefined))
    } else {
        Ok(JsValue::Undefined)
    }
}

fn has(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let key = arg(arguments, 0);
    let object = with_map(realm, this)?;
    let data = object.borrow();
    if let ObjectKind::Map(entries) = &data.kind {
        Ok(JsValue::Bool(entries.iter().any(|(entry_key, _)| {
            crate::vm::same_value_zero(entry_key, &key)
        })))
    } else {
        Ok(JsValue::Bool(false))
    }
}

fn delete(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let key = arg(arguments, 0);
    let object = with_map(realm, this)?;
    let mut data = object.borrow_mut();
    if let ObjectKind::Map(entries) = &mut data.kind
        && let Some(index) = entries
            .iter()
            .position(|(entry_key, _)| crate::vm::same_value_zero(entry_key, &key))
    {
        entries.remove(index);
        return Ok(JsValue::Bool(true));
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
    let callback = arg(arguments, 0);
    let this_arg = arg(arguments, 1);
    let entries = {
        let object = with_map(realm, this)?;
        let data = object.borrow();
        match &data.kind {
            ObjectKind::Map(entries) => entries.clone(),
            _ => Vec::new(),
        }
    };
    for (key, value) in entries {
        super::call(
            realm,
            callback.clone(),
            this_arg.clone(),
            &[value, key, this.clone()],
        )?;
    }
    Ok(JsValue::Undefined)
}

fn keys(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let data = object.borrow();
    let keys = match &data.kind {
        ObjectKind::Map(entries) => entries.iter().map(|(key, _)| key.clone()).collect(),
        _ => Vec::new(),
    };
    Ok(JsValue::Object(realm.iterator(keys)))
}

fn values(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let data = object.borrow();
    let values = match &data.kind {
        ObjectKind::Map(entries) => entries.iter().map(|(_, value)| value.clone()).collect(),
        _ => Vec::new(),
    };
    Ok(JsValue::Object(realm.iterator(values)))
}

fn entries(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let data = object.borrow();
    let entries: Vec<JsValue> = match &data.kind {
        ObjectKind::Map(entries) => entries
            .iter()
            .map(|(key, value)| JsValue::Object(realm.array(vec![key.clone(), value.clone()])))
            .collect(),
        _ => Vec::new(),
    };
    Ok(JsValue::Object(realm.iterator(entries)))
}

fn size(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_map(realm, this)?;
    let data = object.borrow();
    Ok(JsValue::number(match &data.kind {
        ObjectKind::Map(entries) => entries.len() as f64,
        _ => 0.0,
    }))
}
