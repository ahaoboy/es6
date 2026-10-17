use super::*;
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicSymbol / js_symbol_* (quickjs.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.symbol_proto.clone();
    let constructor = install_constructor(realm, "Symbol", 0, constructor, &prototype);
    let target = constructor.as_object().unwrap().to_ref();
    for name in [
        "iterator",
        "asyncIterator",
        "toStringTag",
        "hasInstance",
        "species",
        "match",
        "replace",
        "search",
        "split",
        "isConcatSpreadable",
        "unscopables",
        "toPrimitive",
    ] {
        let id = realm.well_known_symbol(name);
        define_value(&target, name, JsValue::Symbol(id));
    }
    define_method(realm, &target, "for", 1, symbol_for);
    define_method(realm, &target, "keyFor", 1, key_for);
    define_method(realm, &prototype, "toString", 0, to_string);
    define_method(realm, &prototype, "valueOf", 0, value_of);
    define_accessor(realm, &prototype, "description", description, None);
}

fn constructor(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let description = if arg(arguments, 0).is_undefined() {
        String::new()
    } else {
        crate::vm::to_string(realm, arg(arguments, 0))?
    };
    Ok(realm.symbol(&description))
}

fn symbol_for(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let key = crate::vm::to_string(realm, arg(arguments, 0))?;
    if let Some(id) = realm.symbol_registry.get(&key) {
        return Ok(JsValue::Symbol(*id));
    }
    let id = realm.next_symbol(&key);
    realm.symbol_registry.insert(key, id);
    Ok(JsValue::Symbol(id))
}

fn key_for(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let JsValue::Symbol(id) = arg(arguments, 0) else {
        return Err(realm.type_error("Symbol.keyFor requires a symbol"));
    };
    Ok(realm
        .symbol_registry
        .iter()
        .find(|(_, value)| **value == id)
        .map(|(key, _)| JsValue::string(key.clone()))
        .unwrap_or(JsValue::Undefined))
}

fn description(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let JsValue::Symbol(id) = this else {
        return Err(realm.type_error("Not a symbol"));
    };
    Ok(JsValue::string(realm.symbol_description(*id).to_string()))
}

fn to_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let JsValue::Symbol(id) = this else {
        return Err(realm.type_error("Not a symbol"));
    };
    let description = realm.symbol_description(*id);
    Ok(JsValue::string(if description.is_empty() {
        "Symbol()".into()
    } else {
        format!("Symbol({description})")
    }))
}

fn value_of(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    if this.is_symbol() {
        Ok(this.clone())
    } else {
        Err(realm.type_error("Not a symbol"))
    }
}
