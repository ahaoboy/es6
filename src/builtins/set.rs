use boa_gc::{Gc, GcRefCell};

use super::*;
use crate::object::{MapKey, ObjectData, ObjectKind};
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicMapSet / js_set_* (quickjs.c). Values use
// SameValueZero hashing, giving O(1) membership and insertion-order
// iteration.
pub fn install(realm: &mut Realm) {
    let prototype = realm.set_proto.clone();
    install_constructor(realm, "Set", 0, constructor, &prototype);
    define_method(realm, &prototype, "add", 1, add);
    define_method(realm, &prototype, "has", 1, has);
    define_method(realm, &prototype, "delete", 1, delete);
    define_method(realm, &prototype, "clear", 0, clear);
    define_method(realm, &prototype, "forEach", 1, for_each);
    define_method(realm, &prototype, "values", 0, values);
    define_method(realm, &prototype, "keys", 0, values);
    define_method(realm, &prototype, "entries", 0, entries);
    define_accessor(realm, &prototype, "size", size, None);
    let iterator = realm.well_known_symbol("iterator");
    define_method_symbol(realm, &prototype, iterator, "[Symbol.iterator]", 0, values);
}

fn constructor(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = Gc::new(GcRefCell::new(ObjectData::set(
        Some(realm.set_proto.clone()),
        Vec::new(),
    )));
    let iterable = arg(arguments, 0);
    if !iterable.is_nullish() {
        let values = crate::vm::iterable_items(realm, iterable)?;
        if let ObjectKind::Set(set) = &mut object.borrow_mut().kind {
            for value in values {
                set.insert(MapKey(value));
            }
        }
    }
    Ok(JsValue::Object(object))
}

fn with_set(realm: &Realm, this: &JsValue) -> JsResult<crate::object::ObjRef> {
    let object = this
        .as_object()
        .ok_or_else(|| realm.type_error("Set method called on incompatible receiver"))?;
    let reference = object.to_ref();
    if matches!(reference.borrow().kind, ObjectKind::Set(_)) {
        Ok(reference)
    } else {
        Err(realm.type_error("Set method called on incompatible receiver"))
    }
}

fn add(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_set(realm, this)?;
    let mut data = object.borrow_mut();
    if let ObjectKind::Set(values) = &mut data.kind {
        values.insert(MapKey(arg(arguments, 0)));
    }
    drop(data);
    Ok(this.clone())
}

fn has(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_set(realm, this)?;
    let data = object.borrow();
    if let ObjectKind::Set(values) = &data.kind {
        return Ok(JsValue::Bool(values.contains(&MapKey(arg(arguments, 0)))));
    }
    Ok(JsValue::Bool(false))
}

fn delete(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_set(realm, this)?;
    let mut data = object.borrow_mut();
    if let ObjectKind::Set(values) = &mut data.kind {
        return Ok(JsValue::Bool(
            values.shift_remove(&MapKey(arg(arguments, 0))),
        ));
    }
    Ok(JsValue::Bool(false))
}

fn clear(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_set(realm, this)?;
    let mut data = object.borrow_mut();
    if let ObjectKind::Set(values) = &mut data.kind {
        values.clear();
    }
    Ok(JsValue::Undefined)
}

fn for_each(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let callback = arg(arguments, 0);
    let this_arg = arg(arguments, 1);
    let values: Vec<JsValue> = {
        let object = with_set(realm, this)?;
        let data = object.borrow();
        match &data.kind {
            ObjectKind::Set(values) => values.iter().map(|key| key.0.clone()).collect(),
            _ => Vec::new(),
        }
    };
    for value in values {
        super::call(
            realm,
            callback.clone(),
            this_arg.clone(),
            &[value.clone(), value, this.clone()],
        )?;
    }
    Ok(JsValue::Undefined)
}

fn values(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_set(realm, this)?;
    let values: Vec<JsValue> = {
        let data = object.borrow();
        match &data.kind {
            ObjectKind::Set(values) => values.iter().map(|key| key.0.clone()).collect(),
            _ => Vec::new(),
        }
    };
    Ok(JsValue::Object(realm.iterator(values)))
}

fn entries(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_set(realm, this)?;
    let entries: Vec<JsValue> = {
        let data = object.borrow();
        match &data.kind {
            ObjectKind::Set(values) => values
                .iter()
                .map(|key| JsValue::Object(realm.array(vec![key.0.clone(), key.0.clone()])))
                .collect(),
            _ => Vec::new(),
        }
    };
    Ok(JsValue::Object(realm.iterator(entries)))
}

fn size(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = with_set(realm, this)?;
    let data = object.borrow();
    Ok(JsValue::number(match &data.kind {
        ObjectKind::Set(values) => values.len() as f64,
        _ => 0.0,
    }))
}
