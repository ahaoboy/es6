use crate::gc::{Gc, GcRefCell};
use std::rc::Rc;

use super::*;
use crate::object::{PropKey, Property};
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicObject / js_object_constructor (quickjs.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.object_proto.clone();
    install_constructor(realm, "Object", 1, constructor, &prototype);
    define_method(realm, &prototype, "toString", 0, to_string);
    define_method(realm, &prototype, "toLocaleString", 0, to_string);
    define_method(realm, &prototype, "valueOf", 0, value_of);
    define_method(realm, &prototype, "hasOwnProperty", 1, has_own_property);
    define_method(realm, &prototype, "isPrototypeOf", 1, is_prototype_of);
    define_method(
        realm,
        &prototype,
        "propertyIsEnumerable",
        1,
        property_is_enumerable,
    );
    define_accessor(
        realm,
        &prototype,
        "__proto__",
        get_proto,
        Some(Rc::new(set_proto)),
    );

    let constructor = realm.global_get("Object").as_object().unwrap();
    let target = constructor.to_ref();
    define_method(realm, &target, "keys", 1, keys);
    define_method(realm, &target, "values", 1, values);
    define_method(realm, &target, "entries", 1, entries);
    define_method(realm, &target, "assign", 2, assign);
    define_method(realm, &target, "create", 2, create);
    define_method(realm, &target, "getPrototypeOf", 1, get_proto_of);
    define_method(realm, &target, "setPrototypeOf", 2, set_proto_of);
    define_method(realm, &target, "defineProperty", 3, define_property);
    define_method(realm, &target, "defineProperties", 2, define_properties);
    define_method(
        realm,
        &target,
        "getOwnPropertyDescriptor",
        2,
        get_own_property_descriptor,
    );
    define_method(
        realm,
        &target,
        "getOwnPropertyDescriptors",
        1,
        get_own_property_descriptors,
    );
    define_method(
        realm,
        &target,
        "getOwnPropertyNames",
        1,
        get_own_property_names,
    );
    define_method(
        realm,
        &target,
        "getOwnPropertySymbols",
        1,
        get_own_property_symbols,
    );
    define_method(realm, &target, "freeze", 1, freeze);
    define_method(realm, &target, "isFrozen", 1, is_frozen);
    define_method(realm, &target, "seal", 1, seal);
    define_method(realm, &target, "isSealed", 1, is_sealed);
    define_method(realm, &target, "preventExtensions", 1, prevent_extensions);
    define_method(realm, &target, "isExtensible", 1, is_extensible);
    define_method(realm, &target, "is", 2, is);
    define_method(realm, &target, "fromEntries", 1, from_entries);
    define_method(realm, &target, "hasOwn", 2, has_own);
}

fn constructor(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    match arg(arguments, 0) {
        JsValue::Undefined | JsValue::Null => Ok(JsValue::Object(realm.object())),
        JsValue::Object(value) => Ok(JsValue::Object(value)),
        JsValue::String(value) => Ok(JsValue::Object(realm.boxed_string(value))),
        JsValue::Int(_) | JsValue::Float(_) => {
            let value = crate::vm::to_number(realm, arg(arguments, 0))?;
            Ok(JsValue::Object(realm.boxed_number(value)))
        }
        JsValue::Bool(value) => Ok(JsValue::Object(realm.boxed_bool(value))),
        JsValue::Symbol(value) => Ok(JsValue::Symbol(value)),
    }
}

fn to_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    if this.is_nullish() {
        return Ok(JsValue::string("[object Undefined]"));
    }
    if let JsValue::Object(object) = this {
        let data = object.borrow();
        let name = super::object_kind_name(&data);
        return Ok(JsValue::string(format!("[object {name}]")));
    }
    let _ = realm;
    Ok(JsValue::string(format!(
        "[object {}]",
        match this {
            JsValue::String(_) => "String",
            JsValue::Int(_) | JsValue::Float(_) => "Number",
            JsValue::Bool(_) => "Boolean",
            _ => "Object",
        }
    )))
}

fn value_of(_realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(this.clone())
}

fn has_own_property(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = require_object(realm, this)?;
    let key = crate::vm::to_property_key(realm, arg(arguments, 0))?;
    Ok(JsValue::Bool(object.to_ref().borrow().has_own(&key)))
}

fn is_prototype_of(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let target = match arg(arguments, 0) {
        JsValue::Object(object) => object,
        _ => return Ok(JsValue::Bool(false)),
    };
    let Some(prototype) = this.as_object() else {
        return Ok(JsValue::Bool(false));
    };
    let mut current = target.borrow().prototype.clone();
    while let Some(object) = current {
        if Gc::ptr_eq(&object, &prototype.to_ref()) {
            return Ok(JsValue::Bool(true));
        }
        current = object.borrow().prototype.clone();
    }
    let _ = realm;
    Ok(JsValue::Bool(false))
}

fn property_is_enumerable(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let object = require_object(realm, this)?;
    let key = crate::vm::to_property_key(realm, arg(arguments, 0))?;
    let data = object.to_ref();
    let data = data.borrow();
    let enumerable = data
        .get_own(&key)
        .map(|property| property.enumerable)
        .unwrap_or_else(|| {
            if let crate::object::ObjectKind::Array(array) = &data.kind {
                key.index().is_some_and(|index| index < array.length)
            } else {
                false
            }
        });
    Ok(JsValue::Bool(enumerable))
}

fn get_proto(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = require_object(realm, this)?;
    let proto = object.to_ref().borrow().prototype.clone();
    Ok(proto.map(JsValue::Object).unwrap_or(JsValue::Undefined))
}

fn set_proto(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = require_object(realm, this)?;
    match arg(arguments, 0) {
        JsValue::Object(proto) => {
            if crate::object::would_create_cycle(&object.to_ref(), &proto) {
                return Err(realm.type_error("Cyclic prototype chain"));
            }
            object.to_ref().borrow_mut().set_proto(Some(proto));
        }
        JsValue::Null => object.to_ref().borrow_mut().set_proto(None),
        _ => {
            return Err(realm.type_error("Object prototype may only be an Object or null"));
        }
    }
    Ok(this.clone())
}

/// `ToObject` (ES2022 7.1.18): primitives box into wrapper objects, so
/// `Object.keys("ab")` works like in every engine.
fn to_object_value(realm: &mut Realm, value: &JsValue) -> JsResult<JsValue> {
    match value {
        JsValue::Object(_) => Ok(value.clone()),
        JsValue::String(text) => {
            let object = realm.boxed_string(text.clone());
            Ok(JsValue::Object(object))
        }
        JsValue::Int(_) | JsValue::Float(_) => {
            let number = value.as_number().unwrap_or(f64::NAN);
            let object = realm.boxed_number(number);
            Ok(JsValue::Object(object))
        }
        JsValue::Bool(flag) => {
            let object = realm.boxed_bool(*flag);
            Ok(JsValue::Object(object))
        }
        JsValue::Symbol(_) | JsValue::Undefined | JsValue::Null => {
            Err(realm.type_error("Cannot convert undefined or null to object"))
        }
    }
}

fn keys(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let source = to_object_value(realm, &arg(arguments, 0))?;
    let object = require_object(realm, &source)?;
    let keys = object
        .to_ref()
        .borrow()
        .enumerable_keys()
        .into_iter()
        .filter_map(|key| key.to_rc_string().map(JsValue::String))
        .collect();
    Ok(JsValue::Object(realm.array(keys)))
}

fn values(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let source = to_object_value(realm, &arg(arguments, 0))?;
    let object = require_object(realm, &source)?;
    let mut result = Vec::new();
    let keys = object.to_ref().borrow().enumerable_keys();
    for key in keys {
        result.push(crate::vm::get_value_property(realm, &source, &key)?);
    }
    Ok(JsValue::Object(realm.array(result)))
}

fn entries(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let source = to_object_value(realm, &arg(arguments, 0))?;
    let object = require_object(realm, &source)?;
    let mut result = Vec::new();
    let keys = object.to_ref().borrow().enumerable_keys();
    for key in keys {
        let value = crate::vm::get_value_property(realm, &source, &key)?;
        let entry = realm.array(vec![
            JsValue::String(key.to_rc_string().unwrap_or_default()),
            value,
        ]);
        result.push(JsValue::Object(entry));
    }
    Ok(JsValue::Object(realm.array(result)))
}

fn assign(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let target = arg(arguments, 0);
    if target.is_nullish() {
        return Err(realm.type_error("Cannot convert undefined or null to object"));
    }
    for source in arguments.iter().skip(1) {
        if source.is_nullish() {
            continue;
        }
        let object = require_object(realm, source)?;
        let keys = object.to_ref().borrow().enumerable_keys();
        for key in keys {
            let value = crate::vm::get_value_property(realm, source, &key)?;
            crate::vm::set_value_property(realm, &target, &key, value, true)?;
        }
    }
    Ok(target)
}

fn create(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let proto = match arg(arguments, 0) {
        JsValue::Object(object) => Some(object),
        JsValue::Null => None,
        _ => {
            return Err(realm.type_error("Object prototype may only be an Object or null"));
        }
    };
    let object = Gc::new(GcRefCell::new(crate::object::ObjectData::ordinary(proto)));
    let value = JsValue::Object(object);
    let properties = arg(arguments, 1);
    if properties.is_object() {
        define_properties(realm, &JsValue::Undefined, &[value.clone(), properties])?;
    }
    Ok(value)
}

fn get_proto_of(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = to_object(realm, arg(arguments, 0))?;
    Ok(object
        .to_ref()
        .borrow()
        .prototype
        .clone()
        .map(JsValue::Object)
        .unwrap_or(JsValue::Null))
}

fn set_proto_of(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = require_object(realm, &arg(arguments, 0))?;
    match arg(arguments, 1) {
        JsValue::Object(prototype) => {
            if crate::object::would_create_cycle(&object.to_ref(), &prototype) {
                return Err(realm.type_error("Cyclic prototype chain"));
            }
            object.to_ref().borrow_mut().set_proto(Some(prototype));
        }
        JsValue::Null => object.to_ref().borrow_mut().set_proto(None),
        _ => return Err(realm.type_error("Object prototype may only be an Object or null")),
    }
    Ok(arg(arguments, 0))
}

fn define_property(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = require_object(realm, &arg(arguments, 0))?;
    let key = crate::vm::to_property_key(realm, arg(arguments, 1))?;
    let descriptor = require_object(realm, &arg(arguments, 2))?;
    let descriptor_value = JsValue::Object(descriptor.to_ref());
    let existing = object.to_ref().borrow().get_own(&key).cloned();
    let property = descriptor_from_object(realm, &descriptor_value, existing)?;
    object.to_ref().borrow_mut().define(key, property);
    Ok(arg(arguments, 0))
}

fn define_properties(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let object = require_object(realm, &arg(arguments, 0))?;
    let descriptors = require_object(realm, &arg(arguments, 1))?;
    let keys = descriptors.to_ref().borrow().enumerable_keys();
    for key in keys {
        let descriptor =
            crate::vm::get_value_property(realm, &JsValue::Object(descriptors.to_ref()), &key)?;
        let descriptor = require_object(realm, &descriptor)?;
        let existing = object.to_ref().borrow().get_own(&key).cloned();
        let property =
            descriptor_from_object(realm, &JsValue::Object(descriptor.to_ref()), existing)?;
        object.to_ref().borrow_mut().define(key, property);
    }
    Ok(arg(arguments, 0))
}

fn descriptor_from_object(
    realm: &mut Realm,
    descriptor: &JsValue,
    existing: Option<Property>,
) -> JsResult<Property> {
    let object = descriptor.as_object().unwrap();
    let has = |name: &str| object.to_ref().borrow().has_own(&PropKey::from(name));
    let enumerable = if has("enumerable") {
        let value = crate::vm::get_value_property(realm, descriptor, &PropKey::from("enumerable"))?;
        crate::vm::to_boolean(&value)
    } else {
        existing
            .as_ref()
            .is_some_and(|property| property.enumerable)
    };
    let configurable = if has("configurable") {
        let value =
            crate::vm::get_value_property(realm, descriptor, &PropKey::from("configurable"))?;
        crate::vm::to_boolean(&value)
    } else {
        existing
            .as_ref()
            .is_some_and(|property| property.configurable)
    };
    let accessor = has("get") || has("set") || existing.as_ref().is_some_and(Property::is_accessor);
    if accessor {
        let get = if has("get") {
            let get = crate::vm::get_value_property(realm, descriptor, &PropKey::from("get"))?;
            if !get.is_undefined() && !get.is_function() {
                return Err(realm.type_error("Getter must be a function"));
            }
            (!get.is_undefined()).then_some(Box::new(get))
        } else {
            existing.as_ref().and_then(|property| property.get.clone())
        };
        let set = if has("set") {
            let set = crate::vm::get_value_property(realm, descriptor, &PropKey::from("set"))?;
            if !set.is_undefined() && !set.is_function() {
                return Err(realm.type_error("Setter must be a function"));
            }
            (!set.is_undefined()).then_some(Box::new(set))
        } else {
            existing.as_ref().and_then(|property| property.set.clone())
        };
        return Ok(Property {
            value: JsValue::Undefined,
            get,
            set,
            writable: false,
            enumerable,
            configurable,
        });
    }
    let existing = existing.filter(|property| !property.is_accessor());
    let value = if has("value") {
        crate::vm::get_value_property(realm, descriptor, &PropKey::from("value"))?
    } else {
        existing
            .as_ref()
            .map(|property| property.value.clone())
            .unwrap_or(JsValue::Undefined)
    };
    let writable = if has("writable") {
        let value = crate::vm::get_value_property(realm, descriptor, &PropKey::from("writable"))?;
        crate::vm::to_boolean(&value)
    } else {
        existing.as_ref().is_some_and(|property| property.writable)
    };
    Ok(Property {
        value,
        get: None,
        set: None,
        writable,
        enumerable,
        configurable,
    })
}

fn to_object(realm: &mut Realm, value: JsValue) -> JsResult<Object> {
    match value {
        JsValue::Object(object) => Ok(Object::from_ref(object)),
        JsValue::String(value) => Ok(Object::from_ref(realm.boxed_string(value))),
        JsValue::Int(value) => Ok(Object::from_ref(realm.boxed_number(value as f64))),
        JsValue::Float(value) => Ok(Object::from_ref(realm.boxed_number(value))),
        JsValue::Bool(value) => Ok(Object::from_ref(realm.boxed_bool(value))),
        JsValue::Null | JsValue::Undefined => {
            Err(realm.type_error("Cannot convert undefined or null to object"))
        }
        JsValue::Symbol(_) => Err(realm.type_error("Cannot convert a Symbol to an object")),
    }
}

fn get_own_property_descriptor(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let object = to_object(realm, arg(arguments, 0))?;
    let key = crate::vm::to_property_key(realm, arg(arguments, 1))?;
    let value = match object.to_ref().borrow().get_own_value(&key) {
        Some(value) => value,
        None => return Ok(JsValue::Undefined),
    };
    let (writable, enumerable, configurable, get, set, accessor) = {
        let data = object.to_ref();
        let data = data.borrow();
        match data.get_own(&key) {
            Some(property) => (
                property.writable,
                property.enumerable,
                property.configurable,
                property.get.as_deref().cloned(),
                property.set.as_deref().cloned(),
                property.is_accessor(),
            ),
            // Virtual properties: dense elements and exotic built-in
            // fields, with the attribute sets of ES2022 10.4.
            None => match (&data.kind, key.index(), key.as_str()) {
                (ObjectKind::Array(_), _, Some("length")) => {
                    (true, false, false, None, None, false)
                }
                (ObjectKind::Array(_), Some(_), _) => (true, true, true, None, None, false),
                (ObjectKind::BoxedString(_), Some(_), _) => (false, true, false, None, None, false),
                (ObjectKind::BoxedString(_), _, Some("length")) => {
                    (false, false, false, None, None, false)
                }
                (
                    ObjectKind::TypedArray(_),
                    _,
                    Some("length" | "byteLength" | "byteOffset" | "buffer"),
                ) => (false, false, false, None, None, false),
                (ObjectKind::TypedArray(_), Some(_), _) => (true, true, false, None, None, false),
                _ => (true, true, true, None, None, false),
            },
        }
    };
    let descriptor = realm.object();
    if accessor {
        // Accessor descriptors always expose both `get` and `set`; absent
        // functions are `undefined` (ES2022 6.2.5.1).
        define_value(&descriptor, "get", get.unwrap_or(JsValue::Undefined));
        define_value(&descriptor, "set", set.unwrap_or(JsValue::Undefined));
    } else {
        define_value(&descriptor, "value", value);
        define_value(&descriptor, "writable", JsValue::Bool(writable));
    }
    define_value(&descriptor, "enumerable", JsValue::Bool(enumerable));
    define_value(&descriptor, "configurable", JsValue::Bool(configurable));
    Ok(JsValue::Object(descriptor))
}

fn get_own_property_descriptors(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let object = to_object(realm, arg(arguments, 0))?;
    let result = realm.object();
    let keys = object.to_ref().borrow().own_keys();
    for key in keys {
        if let JsValue::Object(descriptor) = get_own_property_descriptor(
            realm,
            &JsValue::Undefined,
            &[
                JsValue::Object(object.to_ref()),
                JsValue::String(key.to_rc_string().unwrap_or_default()),
            ],
        )? {
            result
                .borrow_mut()
                .define(key, Property::data(JsValue::Object(descriptor)));
        }
    }
    Ok(JsValue::Object(result))
}

fn get_own_property_names(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let object = to_object(realm, arg(arguments, 0))?;
    let keys = object
        .to_ref()
        .borrow()
        .own_keys()
        .into_iter()
        .filter_map(|key| key.to_rc_string().map(JsValue::String))
        .collect();
    Ok(JsValue::Object(realm.array(keys)))
}

fn get_own_property_symbols(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let object = to_object(realm, arg(arguments, 0))?;
    let symbols = object
        .to_ref()
        .borrow()
        .own_keys()
        .into_iter()
        .filter_map(|key| match key {
            PropKey::Symbol(id) => Some(JsValue::Symbol(id)),
            PropKey::Str(_) | PropKey::Index(_) => None,
        })
        .collect();
    Ok(JsValue::Object(realm.array(symbols)))
}

fn freeze(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = require_object(realm, &arg(arguments, 0))?;
    let reference = object.to_ref();
    let mut data = reference.borrow_mut();
    data.extensible = false;
    for property in data.props.iter_mut() {
        property.writable = false;
        property.configurable = false;
    }
    let _ = realm;
    Ok(JsValue::Object(object.to_ref()))
}

fn is_frozen(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = require_object(realm, &arg(arguments, 0))?;
    let data = object.to_ref();
    let data = data.borrow();
    let frozen = !data.extensible
        && data
            .props
            .iter()
            .all(|property| !property.writable && !property.configurable);
    let _ = realm;
    Ok(JsValue::Bool(frozen))
}

fn seal(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = require_object(realm, &arg(arguments, 0))?;
    let reference = object.to_ref();
    let mut data = reference.borrow_mut();
    data.extensible = false;
    for property in data.props.iter_mut() {
        property.configurable = false;
    }
    let _ = realm;
    Ok(JsValue::Object(object.to_ref()))
}

fn is_sealed(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = require_object(realm, &arg(arguments, 0))?;
    let data = object.to_ref();
    let data = data.borrow();
    let sealed = !data.extensible && data.props.iter().all(|property| !property.configurable);
    let _ = realm;
    Ok(JsValue::Bool(sealed))
}

fn prevent_extensions(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let object = require_object(realm, &arg(arguments, 0))?;
    object.to_ref().borrow_mut().extensible = false;
    let _ = realm;
    Ok(JsValue::Object(object.to_ref()))
}

fn is_extensible(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = require_object(realm, &arg(arguments, 0))?;
    let extensible = object.to_ref().borrow().extensible;
    let _ = realm;
    Ok(JsValue::Bool(extensible))
}

fn is(_realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Bool(crate::vm::same_value(
        &arg(arguments, 0),
        &arg(arguments, 1),
    )))
}

fn from_entries(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let entries = crate::vm::iterable_items(realm, arg(arguments, 0))?;
    let object = realm.object();
    for entry in entries {
        let values = crate::vm::iterable_items(realm, entry)?;
        if values.len() >= 2 {
            let key = crate::vm::to_property_key(realm, values[0].clone())?;
            object
                .borrow_mut()
                .define(key, Property::data(values[1].clone()));
        }
    }
    Ok(JsValue::Object(object))
}

fn has_own(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = require_object(realm, &arg(arguments, 0))?;
    let key = crate::vm::to_property_key(realm, arg(arguments, 1))?;
    Ok(JsValue::Bool(object.to_ref().borrow().has_own(&key)))
}
