//! Call, construct and class-creation machinery.
//!
//! `call_value` is the bridge that `JS_CallInternal` (quickjs.c:18051)
//! provides in QuickJS: bound/native dispatch and bytecode invocation.
//! `construct_value` mirrors `JS_CallConstructorInternal` (quickjs.c:21078)
//! with receiver allocation like `JS_NewObjectFromShape` (quickjs.c:6136).

use std::cell::RefCell;
use std::rc::Rc;

use super::frame::{Frame, RunOutcome};
use super::{ArgumentStack};
use crate::bytecode::{ClassDef, Constant, FunctionDef};
use crate::object::{ObjectData, ObjectKind, Property, PropKey};
use crate::runtime::Realm;
use crate::value::{Env, EnvRef, JsResult, JsValue};

/// QuickJS `JS_CallInternal` (quickjs.c:18051) entry: bound and native
/// dispatch around the bytecode VM.
pub fn call_value(
    realm: &mut Realm,
    callee: JsValue,
    this: JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    call_value_owned(realm, callee, this, arguments.iter().cloned().collect())
}

#[inline(never)]
pub(crate) fn call_value_owned(
    realm: &mut Realm,
    callee: JsValue,
    this: JsValue,
    arguments: ArgumentStack,
) -> JsResult<JsValue> {
    let function = match &callee {
        JsValue::Object(object) => match &object.borrow().kind {
            ObjectKind::Function(function) => Some(function.clone()),
            _ => None,
        },
        _ => None,
    };
    let Some(function) = function else {
        return Err(realm.type_error("Value is not a function"));
    };
    if let Some(bound) = &function.bound {
        let mut combined: ArgumentStack = bound.arguments.iter().cloned().collect();
        combined.extend(arguments);
        return call_value_owned(realm, bound.target.clone(), bound.this.clone(), combined);
    }
    if let Some(native) = function.native.clone() {
        return native(realm, &this, &arguments);
    }
    if let Some(data_fn) = function.native_data.clone() {
        let data = function.data.borrow().clone();
        return data_fn(realm, &this, &arguments, &data);
    }
    let definition = function
        .def
        .clone()
        .ok_or_else(|| realm.type_error("Invalid function"))?;
    let env = definition.use_env.then(|| {
        let parent = function.env.clone().unwrap_or_else(|| Env::new(0, None));
        Env::new(definition.env_size as usize, Some(parent))
    });
    let trace = super::error::stack_trace_enabled();
    if trace {
        super::error::push_call_stack(definition.name.clone());
    }
    let this = if definition.is_arrow {
        function.lexical_this.clone().unwrap_or(JsValue::Undefined)
    } else if this.is_nullish() && !definition.strict {
        JsValue::Object(realm.global.clone())
    } else {
        this
    };
    let completion = definition.is_async.then(|| realm.promise());
    let result = super::run_frame(
        realm,
        definition.clone(),
        env,
        this,
        Some(callee),
        arguments,
        completion.clone(),
    );
    if trace {
        super::error::pop_call_stack();
    }
    let Some(completion) = completion else {
        return match result? {
            RunOutcome::Return(value) => Ok(value),
            RunOutcome::Suspended => Ok(JsValue::Undefined),
        };
    };
    match result {
        Ok(RunOutcome::Return(value)) => {
            crate::promise::resolve(realm, &completion, value)?;
        }
        Ok(RunOutcome::Suspended) => {}
        Err(error) => {
            crate::promise::reject(realm, &completion, error)?;
        }
    }
    Ok(JsValue::Object(completion))
}

/// QuickJS `JS_CallConstructorInternal`; allocates the receiver from
/// `Function.prototype` exactly like `JS_NewObjectFromShape` (quickjs.c:6136).
pub fn construct_value(
    realm: &mut Realm,
    callee: JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let Some(function) = callee
        .as_object()
        .and_then(|object| object.to_ref().borrow().function())
    else {
        return Err(realm.type_error("Value is not a constructor"));
    };
    if let Some(bound) = &function.bound {
        let mut combined = bound.arguments.clone();
        combined.extend_from_slice(arguments);
        return construct_value(realm, bound.target.clone(), &combined);
    }
    if function
        .def
        .as_ref()
        .is_some_and(|definition| definition.is_arrow || definition.is_async)
    {
        return Err(realm.type_error("Value is not a constructor"));
    }
    let prototype = super::property::get_value_property(
        realm,
        &callee,
        &PropKey::from("prototype"),
    )?;
    let prototype = match prototype {
        JsValue::Object(object) => object,
        _ => realm.object_proto.clone(),
    };
    if function.native.is_some() || function.native_data.is_some() {
        let this = JsValue::Object(Rc::new(RefCell::new(ObjectData::ordinary(Some(prototype)))));
        let result = call_value(realm, callee.clone(), this.clone(), arguments)?;
        return Ok(if result.is_object() { result } else { this });
    }
    let this_object = ObjectData::ordinary(Some(prototype));
    let this = JsValue::Object(Rc::new(RefCell::new(this_object)));
    let result = call_value(realm, callee.clone(), this.clone(), arguments)?;
    run_instance_initializer(realm, &callee, &this)?;
    Ok(if result.is_object() { result } else { this })
}

pub(crate) fn run_instance_initializer(
    realm: &mut Realm,
    constructor: &JsValue,
    this: &JsValue,
) -> JsResult<()> {
    let function = constructor
        .as_object()
        .and_then(|object| object.to_ref().borrow().function());
    let Some(function) = function else {
        return Ok(());
    };
    let Some(initializer) = function.instance_init.clone() else {
        return Ok(());
    };
    let initializer = realm.make_js_function(
        initializer,
        function.env.clone().unwrap_or_else(|| Env::new(0, None)),
        None,
    );
    call_value(realm, initializer, this.clone(), &[])?;
    Ok(())
}

pub(crate) fn copy_this_fields(this: &JsValue, env: &EnvRef, size: usize) {
    let Some(object) = this.as_object() else {
        return;
    };
    for slot in 0..size {
        let value = object.get(&slot.to_string());
        env.slots.borrow_mut()[slot] = value;
    }
}

/// Super property lookup on the parent prototype, like the
/// `OP_get_field_ic`/`OP_array_from` super path in `JS_CallInternal`
/// (quickjs.c:18051): home-object prototype plus receiver-bound get.
pub(crate) fn super_function(
    realm: &mut Realm,
    frame: &Frame,
    key: &PropKey,
) -> JsResult<JsValue> {
    let callee = frame
        .callee
        .clone()
        .ok_or_else(|| realm.reference_error("super is not available"))?;
    let prototype = super::property::get_value_property(
        realm,
        &callee,
        &PropKey::from("__super_proto__"),
    )?;
    if prototype.is_undefined() {
        return Err(realm.reference_error("super is not available"));
    }
    get_property_receiver(realm, &prototype, key, &frame.this)
}

/// Resolve a property with a receiver, so getter calls bind to the original
/// receiver like `JS_GetPropertyInternal` (quickjs.c:9110).
pub(crate) fn get_property_receiver(
    realm: &mut Realm,
    value: &JsValue,
    key: &PropKey,
    receiver: &JsValue,
) -> JsResult<JsValue> {
    if let JsValue::Object(object) = value {
        let resolved = object.borrow().resolve_property(key);
        match resolved {
            crate::object::ResolvedProperty::Data { value, .. } => Ok(value),
            crate::object::ResolvedProperty::Accessor {
                get: Some(getter), ..
            } => call_value(realm, getter, receiver.clone(), &[]),
            crate::object::ResolvedProperty::Accessor { get: None, .. } => Ok(JsValue::Undefined),
            crate::object::ResolvedProperty::Missing => Ok(JsValue::Undefined),
        }
    } else {
        super::property::get_value_property(realm, value, key)
    }
}

pub(crate) fn class_constant(definition: &FunctionDef, index: u32) -> JsResult<Rc<ClassDef>> {
    match definition.constants.get(index as usize) {
        Some(Constant::Class(class)) => Ok(class.clone()),
        _ => Err(JsValue::string("InternalError: invalid class constant")),
    }
}

/// Class wiring: constructor, prototype chain, methods, accessors and the
/// static initializer. Corresponds to `js_parse_class` emission
/// (quickjs.c:25571) executed at runtime.
pub(crate) fn create_class(
    realm: &mut Realm,
    class: &Rc<ClassDef>,
    env: &EnvRef,
    parent: JsValue,
) -> JsResult<JsValue> {
    let constructor_def = class
        .constructor
        .clone()
        .ok_or_else(|| realm.type_error("Missing class constructor"))?;
    let constructor =
        realm.make_class_constructor(constructor_def, env.clone(), class.instance_init.clone());
    let constructor_object = constructor
        .as_object()
        .ok_or_else(|| realm.type_error("Invalid constructor"))?;
    let prototype = constructor_object.get("prototype").as_object();
    if let Some(parent_object) = parent.as_object() {
        constructor_object.to_ref().borrow_mut().prototype = Some(parent_object.to_ref());
        let mut parent_proto_object = None;
        if let Some(prototype) = &prototype {
            let parent_proto = parent_object.get("prototype").as_object();
            if let Some(parent_proto) = parent_proto {
                prototype.to_ref().borrow_mut().prototype = Some(parent_proto.to_ref());
                parent_proto_object = Some(parent_proto);
            }
        }
        constructor_object.set("__super__", parent.clone());
        if let Some(parent_proto) = parent_proto_object {
            constructor_object.set("__super_proto__", JsValue::Object(parent_proto.to_ref()));
        }
    }
    if let Some(initializer) = &class.static_init {
        let initializer = realm.make_js_function(initializer.clone(), env.clone(), None);
        call_value(realm, initializer, constructor.clone(), &[])?;
    }
    for method in &class.methods {
        let function = realm.make_js_function(method.function.clone(), env.clone(), None);
        if let Some(parent_object) = parent.as_object()
            && let Some(parent_proto) = parent_object.get("prototype").as_object()
            && let Some(function_object) = function.as_object()
        {
            function_object.set("__super_proto__", JsValue::Object(parent_proto.to_ref()));
        }
        let target = if method.is_static {
            constructor_object.clone()
        } else if let Some(prototype) = &prototype {
            prototype.clone()
        } else {
            constructor_object.clone()
        };
        if method.is_getter || method.is_setter {
            let target_reference = target.to_ref();
            let mut data = target_reference.borrow_mut();
            let property = data
                .props
                .entry(PropKey::Str(method.name.clone()))
                .or_insert_with(|| Property {
                    value: JsValue::Undefined,
                    get: None,
                    set: None,
                    writable: false,
                    enumerable: false,
                    configurable: true,
                });
            if method.is_getter {
                property.get = Some(Box::new(function));
            } else {
                property.set = Some(Box::new(function));
            }
        } else {
            target.set_key(PropKey::Str(method.name.clone()), function);
        }
    }
    Ok(constructor)
}
