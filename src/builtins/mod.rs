//! ECMAScript built-ins. Each module installs one constructor/prototype pair,
//! mirroring the split used by QuickJS' `JS_AddIntrinsic*` functions
//! (`quickjs.c`, around the `JS_NewContextRaw` initialization table).

mod array;
mod boolean;
mod buffer;
#[cfg(feature = "console")]
mod console;
mod date;
mod error;
mod function;
mod generator;
mod global;
pub(crate) mod json;
mod map;
mod math;
mod message;
mod number;
mod object;
mod promise;
mod regexp;
mod set;
mod string;
mod symbol;
mod weakmap;

use std::cell::RefCell;
use std::rc::Rc;

use crate::object::{
    JsFunction, NativeFn, ObjRef, Object, ObjectData, ObjectKind, PropKey, Property,
};
use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};

pub fn install(realm: &mut Realm) {
    object::install(realm);
    function::install(realm);
    array::install(realm);
    buffer::install(realm);
    string::install(realm);
    number::install(realm);
    boolean::install(realm);
    error::install(realm);
    math::install(realm);
    json::install(realm);
    regexp::install(realm);
    map::install(realm);
    set::install(realm);
    symbol::install(realm);
    promise::install(realm);
    date::install(realm);
    #[cfg(feature = "console")]
    console::install(realm);
    message::install(realm);
    global::install(realm);
    generator::install(realm);
    weakmap::install(realm);
    crate::modules::install(realm);
}

pub(crate) fn arg(arguments: &[JsValue], index: usize) -> JsValue {
    arguments.get(index).cloned().unwrap_or(JsValue::Undefined)
}

pub(crate) fn arg_number(realm: &mut Realm, arguments: &[JsValue], index: usize) -> JsResult<f64> {
    crate::vm::to_number(realm, arg(arguments, index))
}

/// `ToNumber` of a possibly-absent argument: missing or `undefined` yields
/// the spec default (unlike `arg_number`, where `undefined` is `NaN`).
pub(crate) fn arg_number_or(
    realm: &mut Realm,
    arguments: &[JsValue],
    index: usize,
    default: f64,
) -> JsResult<f64> {
    match arguments.get(index) {
        Some(JsValue::Undefined) | None => Ok(default),
        Some(value) => crate::vm::to_number(realm, value.clone()),
    }
}

pub(crate) fn define_value(target: &ObjRef, name: &str, value: impl Into<JsValue>) {
    target
        .borrow_mut()
        .define(name, Property::data(value.into()));
}

pub(crate) fn define_method(
    realm: &mut Realm,
    target: &ObjRef,
    name: &str,
    length: u32,
    native: impl Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue> + 'static,
) {
    let function = realm.native_function(name, length, native);
    target.borrow_mut().define(name, Property::hidden(function));
}

pub(crate) fn define_method_symbol(
    realm: &mut Realm,
    target: &ObjRef,
    symbol: u32,
    name: &str,
    length: u32,
    native: impl Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue> + 'static,
) {
    let function = realm.native_function(name, length, native);
    target
        .borrow_mut()
        .define(PropKey::Symbol(symbol), Property::hidden(function));
}

pub(crate) fn define_accessor(
    realm: &mut Realm,
    target: &ObjRef,
    name: &str,
    getter: impl Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue> + 'static,
    setter: Option<NativeFn>,
) {
    let getter = realm.native_function(&format!("get {name}"), 0, getter);
    let setter = setter.map(|setter| {
        realm.native_function(&format!("set {name}"), 1, move |realm, this, arguments| {
            setter(realm, this, arguments)
        })
    });
    target.borrow_mut().define(
        name,
        Property {
            value: JsValue::Undefined,
            get: Some(Box::new(getter)),
            set: setter.map(Box::new),
            writable: false,
            enumerable: false,
            configurable: true,
        },
    );
}

pub(crate) fn install_constructor(
    realm: &mut Realm,
    name: &str,
    length: u32,
    native: impl Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue> + 'static,
    prototype: &ObjRef,
) -> JsValue {
    let function = realm.native_function(name, length, native);
    if let Some(object) = function.as_object() {
        object.set("prototype", JsValue::Object(prototype.clone()));
    }
    prototype
        .borrow_mut()
        .define("constructor", Property::hidden(function.clone()));
    Object::from_ref(realm.global.clone()).set(name, function.clone());
    function
}

pub(crate) fn require_object(realm: &Realm, value: &JsValue) -> JsResult<Object> {
    value
        .as_object()
        .ok_or_else(|| realm.type_error("Expected an object"))
}

/// The `this` value as a string, sharing the atom allocation instead of
/// copying: string primitives and boxed strings hand back their `JsString`
/// (QuickJS does the same with its `JSString *` payload).
pub(crate) fn this_string(realm: &mut Realm, this: &JsValue) -> JsResult<crate::string::JsString> {
    match this {
        JsValue::String(value) => Ok(value.clone()),
        JsValue::Object(object) => {
            if let ObjectKind::BoxedString(value) = &object.borrow().kind {
                Ok(value.clone())
            } else {
                Ok(crate::string::JsString::from(crate::vm::to_string(
                    realm,
                    this.clone(),
                )?))
            }
        }
        _ => Ok(crate::string::JsString::from(crate::vm::to_string(
            realm,
            this.clone(),
        )?)),
    }
}

pub(crate) fn this_number(realm: &Realm, this: &JsValue) -> JsResult<f64> {
    match this {
        JsValue::Int(value) => Ok(*value as f64),
        JsValue::Float(value) => Ok(*value),
        JsValue::Object(object) => match &object.borrow().kind {
            ObjectKind::BoxedNumber(value) => Ok(*value),
            _ => Err(realm.type_error("Number.prototype method called on incompatible receiver")),
        },
        _ => Err(realm.type_error("Number.prototype method called on incompatible receiver")),
    }
}

pub(crate) fn this_boolean(realm: &Realm, this: &JsValue) -> JsResult<bool> {
    match this {
        JsValue::Bool(value) => Ok(*value),
        JsValue::Object(object) => match &object.borrow().kind {
            ObjectKind::BoxedBool(value) => Ok(*value),
            _ => Err(realm.type_error("Boolean.prototype method called on incompatible receiver")),
        },
        _ => Err(realm.type_error("Boolean.prototype method called on incompatible receiver")),
    }
}

pub(crate) fn array_object(realm: &Realm, this: &JsValue) -> JsResult<Object> {
    this.as_object()
        .ok_or_else(|| realm.type_error("Array method called on non-object"))
}

pub(crate) fn array_items(realm: &mut Realm, object: &Object) -> JsResult<Vec<JsValue>> {
    let data = object.to_ref();
    let data = data.borrow();
    if let ObjectKind::Array(array) = &data.kind {
        // Dense elements flattened; holes read as `undefined`.
        return Ok(array
            .items
            .iter()
            .map(|item| item.clone().unwrap_or(JsValue::Undefined))
            .collect());
    }
    drop(data);
    let length_value = crate::vm::get_value_property(
        realm,
        &JsValue::Object(object.to_ref()),
        &PropKey::from("length"),
    )?;
    let length = crate::vm::to_uint32(crate::vm::to_number(realm, length_value)?) as usize;
    let mut items = Vec::with_capacity(length);
    for index in 0..length {
        items.push(crate::vm::get_value_property(
            realm,
            &JsValue::Object(object.to_ref()),
            &PropKey::from(index.to_string()),
        )?);
    }
    Ok(items)
}

pub(crate) fn set_array_items(realm: &mut Realm, object: &Object, items: Vec<JsValue>) {
    let length = items.len();
    let reference = object.to_ref();
    let mut data = reference.borrow_mut();
    if let ObjectKind::Array(array) = &mut data.kind {
        array.items = items.into_iter().map(Some).collect();
        array.length = length;
        return;
    }
    drop(data);
    for (index, item) in items.into_iter().enumerate() {
        let _ = crate::vm::set_value_property(
            realm,
            &JsValue::Object(object.to_ref()),
            &PropKey::from(index.to_string()),
            item,
            true,
        );
    }
    let _ = crate::vm::set_value_property(
        realm,
        &JsValue::Object(object.to_ref()),
        &PropKey::from("length"),
        JsValue::number(length as f64),
        true,
    );
}

pub(crate) fn bound_function(
    realm: &Realm,
    target: JsValue,
    this: JsValue,
    prefix: Vec<JsValue>,
    name: &str,
    length: u32,
) -> JsValue {
    let function = Rc::new(JsFunction {
        name: Rc::from(name),
        length,
        native: None,
        native_data: None,
        data: Rc::new(RefCell::new(Vec::new())),
        def: None,
        env: None,
        bound: Some(crate::object::BoundFunction {
            target,
            this,
            arguments: prefix,
        }),
        lexical_this: None,
        instance_init: None,
        has_super: false,
        prototype: std::cell::RefCell::new(None),
    });
    JsValue::Object(realm.function_object(function))
}

pub(crate) fn define_builtin_object(realm: &mut Realm, name: &str, object: &ObjRef) {
    Object::from_ref(realm.global.clone()).set(name, JsValue::Object(object.clone()));
}

pub(crate) fn call(
    realm: &mut Realm,
    function: JsValue,
    this: JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    crate::vm::call_value(realm, function, this, arguments)
}

pub(crate) fn get(realm: &mut Realm, value: &JsValue, key: &str) -> JsResult<JsValue> {
    crate::vm::get_value_property(realm, value, &PropKey::from(key))
}

pub(crate) fn set(
    realm: &mut Realm,
    value: &JsValue,
    key: &str,
    new_value: JsValue,
) -> JsResult<()> {
    crate::vm::set_value_property(realm, value, &PropKey::from(key), new_value, true)
}

pub(crate) fn to_integer(value: f64) -> i64 {
    if value.is_nan() {
        0
    } else if value.is_infinite() {
        if value > 0.0 { i64::MAX } else { i64::MIN }
    } else {
        value.trunc() as i64
    }
}

pub(crate) fn normalize_index(index: f64, length: usize) -> usize {
    let length = length as i64;
    let mut index = to_integer(index);
    if index < 0 {
        index = (length + index).max(0);
    }
    index.min(length) as usize
}

pub(crate) fn object_kind_name(object: &ObjectData) -> &'static str {
    match &object.kind {
        ObjectKind::Array(_) => "Array",
        ObjectKind::Function(_) => "Function",
        ObjectKind::BoxedString(_) => "String",
        ObjectKind::BoxedNumber(_) => "Number",
        ObjectKind::BoxedBool(_) => "Boolean",
        ObjectKind::Date(_) => "Date",
        ObjectKind::RegExp(_) => "RegExp",
        ObjectKind::Map(_) => "Map",
        ObjectKind::Set(_) => "Set",
        ObjectKind::WeakMap(_) => "WeakMap",
        ObjectKind::WeakSet(_) => "WeakSet",
        ObjectKind::Promise(_) => "Promise",
        ObjectKind::ArrayBuffer(_) => "ArrayBuffer",
        ObjectKind::DataView(_) => "DataView",
        ObjectKind::TypedArray(data) => data.kind.name(),
        ObjectKind::Error => "Error",
        ObjectKind::Iterator { .. } => "Iterator",
        ObjectKind::ArrayIterator { .. } => "Array Iterator",
        ObjectKind::StringIterator { .. } => "String Iterator",
        ObjectKind::AsyncFrame(_) => "AsyncFunction",
        ObjectKind::Generator(_) => "Generator",
        ObjectKind::Ordinary => "Object",
    }
}
