//! Generator prototype and iterator methods.
//!
//! `%GeneratorPrototype%` (ES2022 27.5.2) plus the lazy Array iterator family
//! (ES2022 23.1.5), mirroring QuickJS' `JS_CLASS_ARRAY_ITERATOR` and its
//! iterator installs.

use super::*;
use crate::value::JsResult;

pub fn install(realm: &mut Realm) {
    install_generator_prototype(realm);
    install_array_iterators(realm);
    install_typed_array_iterator(realm);
}

fn install_generator_prototype(realm: &mut Realm) {
    let prototype = realm.generator_proto.clone();
    define_method(realm, &prototype, "next", 1, generator_next);
    define_method(realm, &prototype, "return", 1, generator_return);
    define_method(realm, &prototype, "throw", 1, generator_throw);
    // `Generator.prototype[Symbol.iterator]` returns the generator itself
    // (ES2022 27.5.2.5).
    let iterator = realm.well_known_symbol("iterator");
    define_method_symbol(
        realm,
        &prototype,
        iterator,
        "[Symbol.iterator]",
        0,
        generator_iterator,
    );
}

fn install_array_iterators(realm: &mut Realm) {
    let prototype = realm.array_proto.clone();
    define_method(realm, &prototype, "values", 0, array_iterator_values);
    define_method(realm, &prototype, "keys", 0, array_iterator_keys);
    define_method(realm, &prototype, "entries", 0, array_iterator_entries);
    // `Array.prototype[Symbol.iterator]` is `values` (ES2022 23.1.2.1).
    let iterator = realm.well_known_symbol("iterator");
    define_method_symbol(
        realm,
        &prototype,
        iterator,
        "values",
        0,
        array_iterator_values,
    );
}

fn install_typed_array_iterator(realm: &mut Realm) {
    let iterator = realm.well_known_symbol("iterator");
    let protos: Vec<_> = realm.typed_array_protos.values().cloned().collect();
    for prototype in protos {
        define_method_symbol(
            realm,
            &prototype,
            iterator,
            "values",
            0,
            typed_array_iterator_values,
        );
    }
}

fn generator_next(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    crate::vm::generator::generator_next(realm, this, arguments)
}

fn generator_return(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    crate::vm::generator::generator_return(realm, this, arguments)
}

fn generator_throw(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    crate::vm::generator::generator_throw(realm, this, arguments)
}

fn generator_iterator(
    _realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    Ok(this.clone())
}

/// `Array.prototype.values` (ES2022 23.1.3.34): a live iterator over the array.
fn array_iterator_values(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    let array = array_object(realm, this)?;
    Ok(JsValue::Object(realm.array_iterator(array.to_ref(), false)))
}

fn array_iterator_keys(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    let array = array_object(realm, this)?;
    let length = array.len();
    Ok(JsValue::Object(
        realm.iterator(
            (0..length)
                .map(|index| JsValue::number(index as f64))
                .collect(),
        ),
    ))
}

fn array_iterator_entries(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    let array = array_object(realm, this)?;
    Ok(JsValue::Object(realm.array_iterator(array.to_ref(), true)))
}

fn typed_array_iterator_values(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let items = match &object.to_ref().borrow().kind {
        crate::object::ObjectKind::TypedArray(typed) => typed.values(),
        _ => Vec::new(),
    };
    Ok(JsValue::Object(realm.iterator(items)))
}
