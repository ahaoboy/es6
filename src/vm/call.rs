//! Call, construct and class-creation machinery.
//!
//! `call_value` is the bridge that `JS_CallInternal` (quickjs.c:18051)
//! provides in QuickJS: bound/native dispatch and bytecode invocation.
//! `construct_value` mirrors `JS_CallConstructorInternal` (quickjs.c:21078)
//! with receiver allocation like `JS_NewObjectFromShape` (quickjs.c:6136).

use crate::gc::{Gc, GcRefCell};
use std::rc::Rc;

use super::ArgumentStack;
use super::frame::{Frame, RunOutcome};
use crate::bytecode::{ClassDef, Constant, FunctionDef};
use crate::object::{ObjectData, ObjectKind, PropKey};
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

#[inline]
pub(crate) fn call_value_owned(
    realm: &mut Realm,
    callee: JsValue,
    this: JsValue,
    arguments: ArgumentStack,
) -> JsResult<JsValue> {
    // Probe the callee inside the borrow and copy only the fields of the
    // selected path. The common bytecode call therefore does not clone the
    // `JsFunction` handle at all.
    let (definition, parent_env, lexical_this) = {
        let data = match &callee {
            JsValue::Object(object) => object.borrow(),
            _ => return Err(realm.type_error("Value is not a function")),
        };
        let ObjectKind::Function(function) = &data.kind else {
            return Err(realm.type_error("Value is not a function"));
        };
        if let Some(bound) = &function.bound {
            let target = bound.target.clone();
            let this = bound.this.clone();
            let mut combined: ArgumentStack = bound.arguments.iter().cloned().collect();
            combined.extend(arguments);
            drop(data);
            return call_value_owned(realm, target, this, combined);
        }
        if let Some(native) = &function.native {
            let native = native.clone();
            drop(data);
            return native(realm, &this, &arguments);
        }
        if let Some(data_fn) = &function.native_data {
            let data_fn = data_fn.clone();
            let function_data = function.data.borrow().clone();
            drop(data);
            return data_fn(realm, &this, &arguments, &function_data);
        }
        let definition = match &function.def {
            Some(definition) => definition.clone(),
            None => return Err(realm.type_error("Invalid function")),
        };
        if definition.is_generator {
            // Calling a generator function creates a generator object without
            // running the body (ES2022 15.5.1.1), like QuickJS' generator call
            // path (quickjs.c:20871).
            let strict = definition.strict;
            let function = function.clone();
            drop(data);
            let this = if this.is_nullish() && !strict {
                JsValue::Object(realm.global.clone())
            } else {
                this
            };
            return Ok(JsValue::Object(realm.generator(
                function,
                this,
                arguments.to_vec(),
            )));
        }
        (
            definition,
            function.env.clone(),
            function.lexical_this.clone(),
        )
    };
    let env = definition.use_env.then(|| {
        let parent = parent_env.unwrap_or_else(|| Env::new(0, None));
        Env::new(definition.env_size as usize, Some(parent))
    });
    let trace = realm.trace_stack;
    if trace {
        super::error::push_call_stack(definition.name.clone());
    }
    let (this, global_this) = if definition.is_arrow {
        (lexical_this.unwrap_or(JsValue::Undefined), false)
    } else if this.is_nullish() && !definition.strict {
        (JsValue::Undefined, true)
    } else {
        (this, false)
    };
    let completion = definition.is_async.then(|| realm.promise());
    if realm.call_depth >= realm.max_call_depth {
        return Err(realm.range_error("Maximum call stack size exceeded"));
    }
    realm.call_depth += 1;
    let result = super::run_frame(
        realm,
        definition,
        env,
        this,
        global_this,
        Some(callee),
        arguments,
        completion.clone(),
    );
    realm.call_depth -= 1;
    if trace {
        super::error::pop_call_stack();
    }
    let Some(completion) = completion else {
        return match result? {
            RunOutcome::Return(value) => Ok(value),
            RunOutcome::Suspended | RunOutcome::Yield { .. } => Ok(JsValue::Undefined),
        };
    };
    match result {
        Ok(RunOutcome::Return(value)) => {
            crate::promise::resolve(realm, &completion, value)?;
        }
        Ok(RunOutcome::Suspended | RunOutcome::Yield { .. }) => {}
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
    // Read the own prototype property directly with a cached atom instead
    // of going through the generic property path (and allocating the name) on
    // every construction.
    let prototype = {
        let key = crate::object::prototype_key();
        let own = match &callee {
            JsValue::Object(object_ref) => {
                let data = object_ref.borrow();
                data.get_own(&key)
                    .map(|property| (property.is_accessor(), property.value.clone()))
            }
            _ => None,
        };
        match own {
            Some((false, JsValue::Object(prototype))) => prototype,
            Some((false, _)) => realm.object_proto.clone(),
            Some((true, _)) => {
                // An accessor prototype: follow the generic getter path.
                match super::property::get_value_property(realm, &callee, &key)? {
                    JsValue::Object(prototype) => prototype,
                    _ => realm.object_proto.clone(),
                }
            }
            None => {
                if function.prototype.borrow().is_some() {
                    // A deleted prototype falls back to %Object.prototype%.
                    realm.object_proto.clone()
                } else {
                    let JsValue::Object(object_ref) = &callee else {
                        unreachable!()
                    };
                    match realm.function_prototype(object_ref, &function) {
                        JsValue::Object(prototype) => prototype,
                        _ => realm.object_proto.clone(),
                    }
                }
            }
        }
    };
    if function.native.is_some() || function.native_data.is_some() {
        let this = JsValue::Object(Gc::new(GcRefCell::new(ObjectData::ordinary(Some(
            prototype,
        )))));
        let result = call_value(realm, callee.clone(), this.clone(), arguments)?;
        return Ok(if result.is_object() { result } else { this });
    }
    let this_object = ObjectData::ordinary(Some(prototype));
    let this = JsValue::Object(Gc::new(GcRefCell::new(this_object)));
    // Base class fields initialize before the constructor body runs; derived
    // class fields initialize in the `super()` call (see `Op::SuperCall`).
    if !function.has_super {
        run_instance_initializer(realm, &callee, &this)?;
    }
    let result = call_value(realm, callee.clone(), this.clone(), arguments)?;
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
pub(crate) fn super_function(realm: &mut Realm, frame: &Frame, key: &PropKey) -> JsResult<JsValue> {
    let callee = frame
        .callee
        .clone()
        .ok_or_else(|| realm.reference_error("super is not available"))?;
    let prototype =
        super::property::get_value_property(realm, &callee, &PropKey::from("__super_proto__"))?;
    if prototype.is_undefined() {
        return Err(realm.reference_error("super is not available"));
    }
    let receiver = frame.this_value(realm);
    get_property_receiver(realm, &prototype, key, &receiver)
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
    let constructor = realm.make_class_constructor(
        constructor_def,
        env.clone(),
        class.instance_init.clone(),
        class.has_super,
    );
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
            let get = method.is_getter.then(|| function.clone());
            let set = (!method.is_getter).then(|| function.clone());
            data.define_accessor(PropKey::from(method.name.as_ref()), get, set, false);
        } else {
            target.set_key(PropKey::from(method.name.as_ref()), function);
        }
    }
    Ok(constructor)
}
