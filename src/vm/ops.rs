//! Opcode dispatch and operand-stack helpers.
//!
//! This is the Rust counterpart of the giant switch in QuickJS
//! `JS_CallInternal` (quickjs.c:18051); the fused operands mirror QuickJS'
//! short opcodes (`quickjs-opcode.h`).

use std::rc::Rc;

use super::frame::{Frame, Handler, Step};
use super::{ArgumentStack, ValueStack, constant_string, constant_strings};
use crate::bytecode::{CompareOp, Constant, FunctionDef, Op};
use crate::object::{Object, ObjectData, ObjectKind, PropKey, Property, RegExpData};
use crate::runtime::Realm;
use crate::value::JsValue;
use crate::value::{Env, EnvRef, JsResult};

use super::call::{
    call_value, call_value_owned, class_constant, construct_value, create_class, super_function,
};
use super::iter::{iterable_items, push_array_value};
use super::property::{
    get_element_value, get_value_property, pop_property_key, property_key_from_constant,
    set_element_value, set_value_property,
};

/// Opcode dispatch. This is the Rust counterpart of the giant switch in
/// QuickJS `JS_CallInternal` (quickjs.c:18051).
/// Hot opcodes, inlined into the dispatch loop.
///
/// Keeping this subset small shrinks `run_loop`'s stack frame and callee-save
/// set, which is paid on every JS call; everything else goes through the
/// out-of-line [`execute_op`]. `Ok(None)` means "not a hot opcode".
#[inline(always)]
pub(crate) fn execute_op(
    realm: &mut Realm,
    frame: &mut Frame,
    pc: &mut usize,
    op: Op,
) -> JsResult<Step> {
    match op {
        Op::Nop => {}
        Op::Undefined => frame.stack.push(JsValue::Undefined),
        Op::Null => frame.stack.push(JsValue::Null),
        Op::True => frame.stack.push(JsValue::Bool(true)),
        Op::False => frame.stack.push(JsValue::Bool(false)),
        Op::This => frame.stack.push(frame.this_value(realm)),
        Op::Callee => frame
            .stack
            .push(frame.callee.clone().unwrap_or(JsValue::Undefined)),
        Op::PushConst(index) => {
            let value = constant_value(realm, &frame.definition, index, frame.env.as_ref())?;
            frame.stack.push(value);
        }
        Op::Pop => {
            pop(&mut frame.stack)?;
        }
        Op::Dup => {
            let value = peek(&frame.stack)?.clone();
            frame.stack.push(value);
        }
        Op::Dup2 => {
            let length = frame.stack.len();
            if length < 2 {
                return Err(realm.type_error("Stack underflow"));
            }
            frame.stack.push(frame.stack[length - 2].clone());
            frame.stack.push(frame.stack[length - 1].clone());
        }
        Op::Swap => {
            let length = frame.stack.len();
            if length < 2 {
                return Err(realm.type_error("Stack underflow"));
            }
            frame.stack.swap(length - 1, length - 2);
        }
        Op::LoadArg(index) => {
            let value = frame
                .arguments
                .get(index as usize)
                .cloned()
                .unwrap_or(JsValue::Undefined);
            frame.stack.push(value);
        }
        Op::LoadArgInto { arg, depth, slot } => {
            let value = frame
                .arguments
                .get(arg as usize)
                .cloned()
                .unwrap_or(JsValue::Undefined);
            frame.set_var(depth, slot, value);
        }
        Op::GetVarPropConst {
            depth,
            slot,
            key,
            cache,
        } => {
            let object = frame.get_var(depth, slot);
            let value = match frame.definition.prop_caches.get(cache as usize) {
                Some(prop_cache) => super::property::cached_get_const(
                    realm,
                    &object,
                    &frame.definition,
                    key,
                    prop_cache,
                )?,
                None => get_value_property(realm, &object, &PropKey::from(""))?,
            };
            frame.stack.push(value);
        }
        Op::GetThisPropConst { key, cache } => {
            let object = frame.this_value(realm);
            let value = match frame.definition.prop_caches.get(cache as usize) {
                Some(prop_cache) => super::property::cached_get_const(
                    realm,
                    &object,
                    &frame.definition,
                    key,
                    prop_cache,
                )?,
                None => get_value_property(realm, &object, &PropKey::from(""))?,
            };
            frame.stack.push(value);
        }
        Op::SetVarPropConstKeep {
            depth,
            slot,
            key,
            cache,
        } => {
            let value = pop(&mut frame.stack)?;
            let object = frame.get_var(depth, slot);
            match frame.definition.prop_caches.get(cache as usize) {
                Some(prop_cache) => super::property::cached_set_const(
                    realm,
                    &object,
                    &frame.definition,
                    key,
                    &value,
                    prop_cache,
                )?,
                None => {
                    set_value_property(realm, &object, &PropKey::from(""), value.clone(), true)?
                }
            }
            frame.stack.push(value);
        }
        Op::SetThisPropConstKeep { key, cache } => {
            let value = pop(&mut frame.stack)?;
            let object = frame.this_value(realm);
            match frame.definition.prop_caches.get(cache as usize) {
                Some(prop_cache) => super::property::cached_set_const(
                    realm,
                    &object,
                    &frame.definition,
                    key,
                    &value,
                    prop_cache,
                )?,
                None => {
                    set_value_property(realm, &object, &PropKey::from(""), value.clone(), true)?
                }
            }
            frame.stack.push(value);
        }
        Op::CallVarMethodConst {
            depth,
            slot,
            key,
            cache,
            argc,
        } => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let object = frame.get_var(depth, slot);
            let function = match frame.definition.prop_caches.get(cache as usize) {
                Some(prop_cache) => super::property::cached_get_const(
                    realm,
                    &object,
                    &frame.definition,
                    key,
                    prop_cache,
                )?,
                None => get_value_property(realm, &object, &PropKey::from(""))?,
            };
            let value = call_value_owned(realm, function, object, arguments)?;
            frame.stack.push(value);
        }
        Op::CallVarMethodConstDiscard {
            depth,
            slot,
            key,
            cache,
            argc,
        } => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let object = frame.get_var(depth, slot);
            let function = match frame.definition.prop_caches.get(cache as usize) {
                Some(prop_cache) => super::property::cached_get_const(
                    realm,
                    &object,
                    &frame.definition,
                    key,
                    prop_cache,
                )?,
                None => get_value_property(realm, &object, &PropKey::from(""))?,
            };
            let _ = call_value_owned(realm, function, object, arguments)?;
        }
        Op::CallThisMethodConst { key, cache, argc } => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let object = frame.this_value(realm);
            let function = match frame.definition.prop_caches.get(cache as usize) {
                Some(prop_cache) => super::property::cached_get_const(
                    realm,
                    &object,
                    &frame.definition,
                    key,
                    prop_cache,
                )?,
                None => get_value_property(realm, &object, &PropKey::from(""))?,
            };
            let value = call_value_owned(realm, function, object, arguments)?;
            frame.stack.push(value);
        }
        Op::CallThisMethodConstDiscard { key, cache, argc } => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let object = frame.this_value(realm);
            let function = match frame.definition.prop_caches.get(cache as usize) {
                Some(prop_cache) => super::property::cached_get_const(
                    realm,
                    &object,
                    &frame.definition,
                    key,
                    prop_cache,
                )?,
                None => get_value_property(realm, &object, &PropKey::from(""))?,
            };
            let _ = call_value_owned(realm, function, object, arguments)?;
        }
        Op::Arguments => {
            frame
                .stack
                .push(JsValue::Object(realm.array(frame.arguments.to_vec())));
        }
        Op::RestArgs(index) => {
            let start = index as usize;
            let rest = if start < frame.arguments.len() {
                frame.arguments[start..].to_vec()
            } else {
                Vec::new()
            };
            frame.stack.push(JsValue::Object(realm.array(rest)));
        }
        Op::GetVar { depth, slot } => {
            let value = frame.get_var(depth, slot);
            frame.stack.push(value);
        }
        Op::SetVar { depth, slot } => {
            let value = pop(&mut frame.stack)?;
            frame.set_var(depth, slot, value);
        }
        Op::GetGlobal { name, cache } => {
            let cache = &frame.definition.prop_caches[cache as usize];
            match super::property::cached_global_get(realm, &frame.definition, name, cache)? {
                Some(value) => frame.stack.push(value),
                None => {
                    let name = constant_string(&frame.definition, name);
                    return Err(realm.reference_error(format!("{name} is not defined")));
                }
            }
        }
        Op::GetGlobalPropConst {
            name,
            key,
            global_cache,
            prop_cache,
        } => {
            let global_cache = &frame.definition.prop_caches[global_cache as usize];
            let object = match super::property::cached_global_get(
                realm,
                &frame.definition,
                name,
                global_cache,
            )? {
                Some(value) => value,
                None => {
                    let name = constant_string(&frame.definition, name);
                    return Err(realm.reference_error(format!("{name} is not defined")));
                }
            };
            let prop_cache = &frame.definition.prop_caches[prop_cache as usize];
            let value = super::property::cached_get_const(
                realm,
                &object,
                &frame.definition,
                key,
                prop_cache,
            )?;
            frame.stack.push(value);
        }
        Op::SetGlobal(index) => {
            let value = pop(&mut frame.stack)?;
            let name = constant_string(&frame.definition, index);
            Object::from_ref(realm.global.clone()).set_key(PropKey::Str(name), value);
        }
        Op::DeleteGlobal(index) => {
            let name = constant_string(&frame.definition, index);
            let deleted = Object::from_ref(realm.global.clone()).delete(&name);
            frame.stack.push(JsValue::Bool(deleted));
        }
        Op::GetProp => {
            let key = pop(&mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            let value = get_element_value(realm, &object, key)?;
            frame.stack.push(value);
        }
        Op::GetIndexVar {
            object_depth,
            object_slot,
            index_depth,
            index_slot,
        } => {
            let object = frame.get_var(object_depth, object_slot);
            let index = frame.get_var(index_depth, index_slot);
            let value = get_element_value(realm, &object, index)?;
            frame.stack.push(value);
        }
        Op::SetProp => {
            let value = pop(&mut frame.stack)?;
            let key = pop(&mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            set_element_value(realm, &object, key, value.clone())?;
            frame.stack.push(value);
        }
        Op::GetIndexUpdate {
            object_depth,
            object_slot,
            index_depth,
            index_slot,
            delta,
            prefix,
        } => {
            let object = frame.get_var(object_depth, object_slot);
            let old = frame.get_var(index_depth, index_slot);
            let number = super::to_numeric(realm, old.clone())?;
            let new = JsValue::number(number + delta as f64);
            frame.set_var(index_depth, index_slot, new.clone());
            let key = if prefix { new } else { old };
            let value = get_element_value(realm, &object, key)?;
            frame.stack.push(value);
        }
        Op::SetIndexVar {
            object_depth,
            object_slot,
            index_depth,
            index_slot,
        } => {
            let value = pop(&mut frame.stack)?;
            let object = frame.get_var(object_depth, object_slot);
            let index = frame.get_var(index_depth, index_slot);
            set_element_value(realm, &object, index, value.clone())?;
            frame.stack.push(value);
        }
        Op::SetPropKeep => {
            let value = pop(&mut frame.stack)?;
            let key = pop(&mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            set_element_value(realm, &object, key, value)?;
        }
        Op::DeleteProp => {
            let key = pop_property_key(realm, &mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            let deleted = match object {
                JsValue::Object(object) => object.borrow_mut().delete_property(&key),
                _ => true,
            };
            frame.stack.push(JsValue::Bool(deleted));
        }
        Op::GetPropConst { key, cache } => {
            let object = pop(&mut frame.stack)?;
            let value = match frame.definition.prop_caches.get(cache as usize) {
                Some(prop_cache) => super::property::cached_get_const(
                    realm,
                    &object,
                    &frame.definition,
                    key,
                    prop_cache,
                )?,
                None => {
                    let key = property_key_from_constant(&frame.definition, key)?;
                    get_value_property(realm, &object, &key)?
                }
            };
            frame.stack.push(value);
        }
        Op::SetPropConst { key, cache } => {
            let value = pop(&mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            match frame.definition.prop_caches.get(cache as usize) {
                Some(prop_cache) => super::property::cached_set_const(
                    realm,
                    &object,
                    &frame.definition,
                    key,
                    &value,
                    prop_cache,
                )?,
                None => {
                    let key = property_key_from_constant(&frame.definition, key)?;
                    set_value_property(realm, &object, &key, value.clone(), true)?;
                }
            }
            frame.stack.push(value);
        }
        Op::SetPropConstKeep { key, cache } => {
            let value = pop(&mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            match frame.definition.prop_caches.get(cache as usize) {
                Some(prop_cache) => super::property::cached_set_const(
                    realm,
                    &object,
                    &frame.definition,
                    key,
                    &value,
                    prop_cache,
                )?,
                None => {
                    let key = property_key_from_constant(&frame.definition, key)?;
                    set_value_property(realm, &object, &key, value, true)?;
                }
            }
            frame.stack.push(object);
        }
        Op::DefineAccessor { is_getter } => {
            let function = pop(&mut frame.stack)?;
            let key = pop_property_key(realm, &mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            if let JsValue::Object(object) = object {
                let mut data = object.borrow_mut();
                let get = is_getter.then(|| function.clone());
                let set = (!is_getter).then(|| function.clone());
                data.define_accessor(key, get, set, true);
            } else {
                return Err(realm.type_error("Cannot define accessor on primitive"));
            }
        }
        Op::NewObject => frame.stack.push(JsValue::Object(realm.object())),
        Op::NewObjectFrom { keys, count, cache } => {
            let count = count as usize;
            if frame.stack.len() < count {
                return Err(realm.type_error("Stack underflow"));
            }
            let start = frame.stack.len() - count;
            let object = realm.object();
            {
                // The per-site template is built once and cloned: shape and
                // property vector are shared/copied, so no key is hashed or
                // parsed per evaluation.
                let template = {
                    let mut entry = frame.definition.prop_caches[cache as usize].borrow_mut();
                    if entry.literal.is_none() {
                        let keys = constant_strings(&frame.definition, keys);
                        let keys: Vec<crate::object::PropKey> = keys
                            .iter()
                            .map(|key| crate::object::key_from_string(key.clone()))
                            .collect();
                        entry.literal = Some(crate::object::LiteralTemplate::new(&keys));
                    }
                    entry.literal.as_ref().expect("literal template").clone()
                };
                let mut data = object.borrow_mut();
                data.shape = template.shape;
                data.props = template.props;
                for (index, value) in frame.stack[start..].iter().enumerate() {
                    if let Some(property) = data.props.get_mut(index) {
                        property.value = value.clone();
                    }
                }
            }
            frame.stack.truncate(start);
            frame.stack.push(JsValue::Object(object));
        }
        Op::NewArray(count) => {
            let count = count as usize;
            if frame.stack.len() < count {
                return Err(realm.type_error("Stack underflow"));
            }
            let start = frame.stack.len() - count;
            let items = frame.stack[start..].to_vec();
            frame.stack.truncate(start);
            frame.stack.push(JsValue::Object(realm.array(items)));
        }
        Op::ArrayPush => {
            let value = pop(&mut frame.stack)?;
            let array = pop(&mut frame.stack)?;
            if let JsValue::Object(object) = &array {
                push_array_value(realm, object, value)?;
            }
            frame.stack.push(array);
        }
        Op::ArraySpread => {
            let iterable = pop(&mut frame.stack)?;
            let array = pop(&mut frame.stack)?;
            let items = iterable_items(realm, iterable)?;
            if let JsValue::Object(object) = &array {
                for item in items {
                    push_array_value(realm, object, item)?;
                }
            }
            frame.stack.push(array);
        }
        Op::ObjectSpread => {
            let source = pop(&mut frame.stack)?;
            let target = pop(&mut frame.stack)?;
            if let (JsValue::Object(target), JsValue::Object(source)) = (&target, &source) {
                super::iter::copy_own_properties(realm, target, source)?;
            }
            frame.stack.push(target);
        }
        Op::ArrayRest(index) => {
            let value = pop(&mut frame.stack)?;
            let items = iterable_items(realm, value.clone())?;
            let rest = items.into_iter().skip(index as usize).collect::<Vec<_>>();
            frame.stack.push(value);
            frame.stack.push(JsValue::Object(realm.array(rest)));
        }
        Op::ObjectRest(index) => {
            let excluded = constant_strings(&frame.definition, index);
            let value = pop(&mut frame.stack)?;
            let rest = realm.object();
            if let JsValue::Object(source) = &value {
                let keys = source.borrow().enumerable_keys();
                for key in keys {
                    if let Some(name) = key.to_rc_string()
                        && excluded
                            .iter()
                            .any(|excluded| excluded.as_ref() == name.as_ref())
                    {
                        continue;
                    }
                    let item = source.borrow().get_property(
                        realm,
                        &key,
                        &JsValue::Object(source.clone()),
                    )?;
                    rest.borrow_mut().define(key, Property::data(item));
                }
            }
            frame.stack.push(value);
            frame.stack.push(JsValue::Object(rest));
        }
        Op::Call(argc) => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let function = pop(&mut frame.stack)?;
            let value = call_value_owned(realm, function, JsValue::Undefined, arguments)?;
            frame.stack.push(value);
        }
        Op::CallDiscard(argc) => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let function = pop(&mut frame.stack)?;
            let _ = call_value_owned(realm, function, JsValue::Undefined, arguments)?;
        }
        Op::CallMethod(argc) => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let function = pop(&mut frame.stack)?;
            let this = pop(&mut frame.stack)?;
            let value = call_value_owned(realm, function, this, arguments)?;
            frame.stack.push(value);
        }
        Op::CallMethodDiscard(argc) => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let function = pop(&mut frame.stack)?;
            let this = pop(&mut frame.stack)?;
            let _ = call_value_owned(realm, function, this, arguments)?;
        }
        Op::CallSpread => {
            let arguments = pop_array(&mut frame.stack)?;
            let function = pop(&mut frame.stack)?;
            let value = call_value(realm, function, JsValue::Undefined, &arguments)?;
            frame.stack.push(value);
        }
        Op::CallMethodSpread => {
            let arguments = pop_array(&mut frame.stack)?;
            let function = pop(&mut frame.stack)?;
            let this = pop(&mut frame.stack)?;
            let value = call_value(realm, function, this, &arguments)?;
            frame.stack.push(value);
        }
        Op::CallSuperMethod(argc) => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let key = pop_property_key(realm, &mut frame.stack)?;
            let function = super_function(realm, frame, &key)?;
            let value = call_value_owned(realm, function, frame.this_value(realm), arguments)?;
            frame.stack.push(value);
        }
        Op::CallSuperMethodSpread => {
            let arguments = pop_array(&mut frame.stack)?;
            let key = pop_property_key(realm, &mut frame.stack)?;
            let function = super_function(realm, frame, &key)?;
            let value = call_value(realm, function, frame.this_value(realm), &arguments)?;
            frame.stack.push(value);
        }
        Op::SuperGetProp => {
            let key = pop_property_key(realm, &mut frame.stack)?;
            let function = super_function(realm, frame, &key)?;
            frame.stack.push(function);
        }
        Op::New(argc) => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let constructor = pop(&mut frame.stack)?;
            let value = construct_value(realm, constructor, &arguments)?;
            frame.stack.push(value);
        }
        Op::NewSpread => {
            let arguments = pop_array(&mut frame.stack)?;
            let constructor = pop(&mut frame.stack)?;
            let value = construct_value(realm, constructor, &arguments)?;
            frame.stack.push(value);
        }
        Op::Return => return Ok(Step::Return),
        Op::ReturnUndefined => {
            frame.stack.push(JsValue::Undefined);
            return Ok(Step::Return);
        }
        Op::Throw => {
            let value = pop(&mut frame.stack)?;
            return Err(value);
        }
        Op::Jump(target) => *pc = target as usize,
        Op::JumpIfFalse(target) => {
            let value = pop(&mut frame.stack)?;
            if !super::to_boolean(&value) {
                *pc = target as usize;
            }
        }
        Op::JumpIfTrue(target) => {
            let value = pop(&mut frame.stack)?;
            if super::to_boolean(&value) {
                *pc = target as usize;
            }
        }
        Op::JumpIfNotUndefined(target) => {
            let value = pop(&mut frame.stack)?;
            if !matches!(value, JsValue::Undefined) {
                *pc = target as usize;
            }
        }
        Op::JumpIfNullish(target) => {
            let value = pop(&mut frame.stack)?;
            if value.is_nullish() {
                *pc = target as usize;
            }
        }
        Op::JumpIfNullishKeep(target) => {
            let value = peek(&frame.stack)?;
            if value.is_nullish() {
                *pc = target as usize;
            } else {
                pop(&mut frame.stack)?;
            }
        }
        Op::AddVarDiscard {
            dst_depth,
            dst_slot,
            src_depth,
            src_slot,
        } => {
            let left = frame.get_var(dst_depth, dst_slot);
            let right = frame.get_var(src_depth, src_slot);
            let value = match add_ints(&left, &right) {
                Some(value) => value,
                None => super::add_values(realm, left, right)?,
            };
            frame.set_var(dst_depth, dst_slot, value);
        }
        Op::UpdateVarDiscard { depth, slot, delta } => {
            let value = frame.get_var(depth, slot);
            let next = match value {
                JsValue::Int(value) => match value.checked_add(delta as i32) {
                    Some(value) => JsValue::Int(value),
                    None => int_step_overflow(value, delta as i32),
                },
                value => numeric_step(realm, value, delta as f64)?,
            };
            frame.set_var(depth, slot, next);
        }
        Op::AddVar {
            dst_depth,
            dst_slot,
            src_depth,
            src_slot,
        } => {
            let left = frame.get_var(dst_depth, dst_slot);
            let right = frame.get_var(src_depth, src_slot);
            let value = match add_ints(&left, &right) {
                Some(value) => value,
                None => super::add_values(realm, left, right)?,
            };
            frame.set_var(dst_depth, dst_slot, value.clone());
            frame.stack.push(value);
        }
        Op::AddVars {
            left_depth,
            left_slot,
            right_depth,
            right_slot,
        } => {
            let left = frame.get_var(left_depth, left_slot);
            let right = frame.get_var(right_depth, right_slot);
            let value = match add_ints(&left, &right) {
                Some(value) => value,
                None => super::add_values(realm, left, right)?,
            };
            frame.stack.push(value);
        }
        Op::AddVarConst { depth, slot, value } => {
            let left = frame.get_var(depth, slot);
            let right = constant_value(realm, &frame.definition, value, frame.env.as_ref())?;
            let result = match add_ints(&left, &right) {
                Some(value) => value,
                None => super::add_values(realm, left, right)?,
            };
            frame.stack.push(result);
        }
        Op::CompareVarConst {
            op,
            depth,
            slot,
            value,
        } => {
            let left = frame.get_var(depth, slot);
            let right = constant_value(realm, &frame.definition, value, frame.env.as_ref())?;
            let result = compare_values(realm, &left, &right, op)?;
            frame.stack.push(JsValue::Bool(result));
        }
        Op::CompareVars {
            op,
            left_depth,
            left_slot,
            right_depth,
            right_slot,
        } => {
            let left = frame.get_var(left_depth, left_slot);
            let right = frame.get_var(right_depth, right_slot);
            let result = compare_values(realm, &left, &right, op)?;
            frame.stack.push(JsValue::Bool(result));
        }
        Op::CompareVarsJump {
            op,
            left_depth,
            left_slot,
            right_depth,
            right_slot,
            jump_if_true,
            target,
        } => {
            let left = frame.get_var(left_depth, left_slot);
            let right = frame.get_var(right_depth, right_slot);
            let result = if let (JsValue::Int(left), JsValue::Int(right)) = (&left, &right) {
                compare_ints(*left, *right, op)
            } else {
                compare_values(realm, &left, &right, op)?
            };
            if result == jump_if_true {
                *pc = target as usize;
            }
        }
        Op::CompareVarConstJump {
            op,
            depth,
            slot,
            value,
            jump_if_true,
            target,
        } => {
            let left = frame.get_var(depth, slot);
            let result = match left {
                JsValue::Int(left) => compare_ints(left, value, op),
                left => compare_values(realm, &left, &JsValue::Int(value), op)?,
            };
            if result == jump_if_true {
                *pc = target as usize;
            }
        }
        Op::UpdateVarJump {
            depth,
            slot,
            delta,
            target,
        } => {
            let value = frame.get_var(depth, slot);
            let next = match value {
                JsValue::Int(value) => match value.checked_add(delta as i32) {
                    Some(value) => JsValue::Int(value),
                    None => int_step_overflow(value, delta as i32),
                },
                value => numeric_step(realm, value, delta as f64)?,
            };
            frame.set_var(depth, slot, next);
            *pc = target as usize;
        }
        Op::ForNext {
            depth,
            slot,
            delta,
            limit_depth,
            limit_slot,
            op,
            body,
        } => {
            let value = frame.get_var(depth, slot);
            let next = match value {
                JsValue::Int(value) => match value.checked_add(delta as i32) {
                    Some(value) => JsValue::Int(value),
                    None => int_step_overflow(value, delta as i32),
                },
                value => numeric_step(realm, value, delta as f64)?,
            };
            frame.set_var(depth, slot, next.clone());
            let limit = frame.get_var(limit_depth, limit_slot);
            let result = if let (JsValue::Int(next), JsValue::Int(limit)) = (&next, &limit) {
                compare_ints(*next, *limit, op)
            } else {
                compare_values(realm, &next, &limit, op)?
            };
            if result {
                *pc = body as usize;
            }
        }
        Op::ForNextConst {
            depth,
            slot,
            delta,
            value,
            op,
            body,
        } => {
            let current = frame.get_var(depth, slot);
            let next = match current {
                JsValue::Int(current) => match current.checked_add(delta as i32) {
                    Some(value) => JsValue::Int(value),
                    None => int_step_overflow(current, delta as i32),
                },
                current => numeric_step(realm, current, delta as f64)?,
            };
            frame.set_var(depth, slot, next.clone());
            let result = match next {
                JsValue::Int(next) => compare_ints(next, value, op),
                next => compare_values(realm, &next, &JsValue::Int(value), op)?,
            };
            if result {
                *pc = body as usize;
            }
        }
        Op::BitAndVarConstJump {
            depth,
            slot,
            value,
            jump_if_true,
            target,
        } => {
            let left = frame.get_var(depth, slot);
            let bits = match left {
                JsValue::Int(left) => left,
                left => int32_value(realm, left)?,
            };
            if (bits & value != 0) == jump_if_true {
                *pc = target as usize;
            }
        }
        Op::Add => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            let value = match add_ints(&left, &right) {
                Some(value) => value,
                None => match (&left, &right) {
                    (JsValue::Float(left), JsValue::Float(right)) => JsValue::Float(left + right),
                    _ => super::add_values(realm, left, right)?,
                },
            };
            frame.stack.push(value);
        }
        Op::Sub => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            if let (JsValue::Int(left), JsValue::Int(right)) = (&left, &right) {
                let value = *left as i64 - *right as i64;
                if value >= i32::MIN as i64 && value <= i32::MAX as i64 {
                    frame.stack.push(JsValue::Int(value as i32));
                } else {
                    frame.stack.push(JsValue::Float(value as f64));
                }
            } else if let (JsValue::Float(left), JsValue::Float(right)) = (&left, &right) {
                frame.stack.push(JsValue::Float(*left - *right));
            } else {
                let value = super::to_numeric(realm, left)? - super::to_numeric(realm, right)?;
                frame.stack.push(JsValue::number(value));
            }
        }
        Op::Mul => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            if let (JsValue::Int(left), JsValue::Int(right)) = (&left, &right) {
                let value = *left as i64 * *right as i64;
                if value >= i32::MIN as i64 && value <= i32::MAX as i64 {
                    frame.stack.push(JsValue::Int(value as i32));
                } else {
                    frame.stack.push(JsValue::Float(value as f64));
                }
            } else if let (JsValue::Float(left), JsValue::Float(right)) = (&left, &right) {
                frame.stack.push(JsValue::Float(*left * *right));
            } else {
                let value = super::to_numeric(realm, left)? * super::to_numeric(realm, right)?;
                frame.stack.push(JsValue::number(value));
            }
        }
        Op::Div => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            if let (JsValue::Float(left), JsValue::Float(right)) = (&left, &right) {
                frame.stack.push(JsValue::Float(*left / *right));
            } else {
                let value = super::to_numeric(realm, left)? / super::to_numeric(realm, right)?;
                frame.stack.push(JsValue::number(value));
            }
        }
        Op::Mod => binary_number(realm, frame, |a, b| a % b)?,
        Op::Exp => binary_number(realm, frame, crate::coerce::js_pow)?,
        Op::BitAnd => binary_int(realm, frame, |a, b| a & b)?,
        Op::BitOr => binary_int(realm, frame, |a, b| a | b)?,
        Op::BitXor => binary_int(realm, frame, |a, b| a ^ b)?,
        Op::Shl => binary_int(realm, frame, |a, b| a.wrapping_shl((b & 31) as u32))?,
        Op::Shr => binary_int(realm, frame, |a, b| a.wrapping_shr((b & 31) as u32))?,
        Op::UShr => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            let value = super::to_uint32(super::to_numeric(realm, left)?)
                >> (super::to_uint32(super::to_numeric(realm, right)?) & 31);
            frame.stack.push(JsValue::number(value as f64));
        }
        Op::Eq => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::Bool(super::loose_equals(realm, &left, &right)?));
        }
        Op::Ne => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::Bool(!super::loose_equals(realm, &left, &right)?));
        }
        Op::StrictEq => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::Bool(super::strict_equals(&left, &right)));
        }
        Op::StrictNe => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::Bool(!super::strict_equals(&left, &right)));
        }
        Op::Lt => compare_op(realm, frame, |o| o < 0)?,
        Op::Le => compare_op(realm, frame, |o| o <= 0)?,
        Op::Gt => compare_op(realm, frame, |o| o > 0)?,
        Op::Ge => compare_op(realm, frame, |o| o >= 0)?,
        Op::In => {
            let object = pop(&mut frame.stack)?;
            let key = pop_property_key(realm, &mut frame.stack)?;
            // `in` on a primitive boxes it, like ToObject in ES2022 13.10.2.
            let result = match &object {
                JsValue::Object(object) => object.borrow().has_property(&key),
                JsValue::String(text) => match &key {
                    PropKey::Str(name) if name.as_ref() == "length" => true,
                    PropKey::Str(name) => crate::object::key_to_index(name)
                        .is_some_and(|index| index < crate::object::utf16_len(text)),
                    PropKey::Index(index) => *index < crate::object::utf16_len(text),
                    _ => false,
                },
                _ => false,
            };
            frame.stack.push(JsValue::Bool(result));
        }
        Op::InstanceOf => {
            let constructor = pop(&mut frame.stack)?;
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::Bool(crate::coerce::instance_of(
                realm,
                &value,
                &constructor,
            )?));
        }
        Op::Not => {
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::Bool(!super::to_boolean(&value)));
        }
        Op::Neg => {
            let value = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::number(-super::to_numeric(realm, value)?));
        }
        Op::Pos => {
            let value = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::number(super::to_numeric(realm, value)?));
        }
        Op::BitNot => {
            let value = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::Int(!super::to_int32(super::to_numeric(
                    realm, value,
                )?)));
        }
        Op::TypeOf => {
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::string(value.type_of()));
        }
        Op::TypeOfGlobal(index) => {
            let name = constant_string(&frame.definition, index);
            let value = super::property::global_get(realm, &name).ok().flatten();
            frame.stack.push(JsValue::string(match value {
                Some(value) => value.type_of(),
                None => "undefined",
            }));
        }
        Op::Void => {
            pop(&mut frame.stack)?;
            frame.stack.push(JsValue::Undefined);
        }
        Op::Await => return Ok(Step::Await),
        Op::Yield(resume_pc) => {
            *pc = resume_pc as usize;
            return Ok(Step::Yield);
        }
        Op::IncVar {
            depth,
            slot,
            prefix,
        } => {
            let old = frame.get_var(depth, slot);
            if let JsValue::Int(value) = old {
                let new = match value.checked_add(1) {
                    Some(value) => JsValue::Int(value),
                    None => JsValue::number(value as f64 + 1.0),
                };
                frame.set_var(depth, slot, new.clone());
                frame
                    .stack
                    .push(if prefix { new } else { JsValue::Int(value) });
            } else {
                let number = numeric_value(realm, old)?;
                let new = JsValue::number(number + 1.0);
                frame.set_var(depth, slot, new.clone());
                frame
                    .stack
                    .push(if prefix { new } else { JsValue::number(number) });
            }
        }
        Op::DecVar {
            depth,
            slot,
            prefix,
        } => {
            let old = frame.get_var(depth, slot);
            if let JsValue::Int(value) = old {
                let new = match value.checked_sub(1) {
                    Some(value) => JsValue::Int(value),
                    None => JsValue::number(value as f64 - 1.0),
                };
                frame.set_var(depth, slot, new.clone());
                frame
                    .stack
                    .push(if prefix { new } else { JsValue::Int(value) });
            } else {
                let number = numeric_value(realm, old)?;
                let new = JsValue::number(number - 1.0);
                frame.set_var(depth, slot, new.clone());
                frame
                    .stack
                    .push(if prefix { new } else { JsValue::number(number) });
            }
        }
        Op::IncGlobal(index, prefix) => {
            let name = constant_string(&frame.definition, index);
            let old = super::property::global_get(realm, &name)
                .ok()
                .flatten()
                .unwrap_or(JsValue::Undefined);
            let number = numeric_value(realm, old)?;
            let new = JsValue::number(number + 1.0);
            Object::from_ref(realm.global.clone()).set_key(PropKey::Str(name), new.clone());
            frame
                .stack
                .push(if prefix { new } else { JsValue::number(number) });
        }
        Op::DecGlobal(index, prefix) => {
            let name = constant_string(&frame.definition, index);
            let old = super::property::global_get(realm, &name)
                .ok()
                .flatten()
                .unwrap_or(JsValue::Undefined);
            let number = numeric_value(realm, old)?;
            let new = JsValue::number(number - 1.0);
            Object::from_ref(realm.global.clone()).set_key(PropKey::Str(name), new.clone());
            frame
                .stack
                .push(if prefix { new } else { JsValue::number(number) });
        }
        Op::UpdateProp { delta, prefix } => {
            let key = pop_property_key(realm, &mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            let old = get_value_property(realm, &object, &key)?;
            let number = numeric_value(realm, old)?;
            let new = JsValue::number(number + delta);
            set_value_property(realm, &object, &key, new.clone(), true)?;
            frame
                .stack
                .push(if prefix { new } else { JsValue::number(number) });
        }
        Op::PushEnv(size) => {
            let size = size as usize;

            let offset = frame.locals.len();
            frame.locals.resize(offset + size, JsValue::Undefined);
            frame.local_frames.push((offset, size));
            frame.local_offset = offset;
            frame.local_size = size;
            // Captured bindings of this scope live in a parallel environment
            // frame; flat storage still serves the uncaptured ones.
            if let Some(parent) = frame.env.clone() {
                frame.env = Some(Env::new(size, Some(parent)));
            }
        }
        Op::PopEnv => {
            if frame.local_frames.len() <= 1 {
                return Err(realm.type_error("Environment underflow"));
            }
            let (offset, _) = frame.local_frames.pop().unwrap();
            frame.locals.truncate(offset);
            match frame.local_frames.last() {
                Some((offset, size)) => {
                    frame.local_offset = *offset;
                    frame.local_size = *size;
                }
                None => {
                    frame.local_offset = 0;
                    frame.local_size = 0;
                }
            }
            if let Some(env) = &frame.env
                && let Some(parent) = env.parent.clone()
            {
                frame.env = Some(parent);
            }
        }
        Op::Closure(index) => {
            // A function that creates closures only has an environment when
            // one of them captures its bindings; otherwise an empty root
            // environment stands in (QuickJS passes `var_refs` the same way).
            let env = frame.env.clone().unwrap_or_else(|| Env::new(0, None));
            let value = closure_value(
                realm,
                &frame.definition,
                index,
                &env,
                frame.this_value(realm),
            )?;
            frame.stack.push(value);
        }
        Op::TryStart(catch_pc) => {
            frame.handlers.push(Handler {
                catch_pc: catch_pc as usize,
                stack_len: frame.stack.len(),
                env: frame.env.clone(),
                local_frames: frame.local_frames.len(),
                locals: frame.locals.len(),
                index: frame.handlers.len(),
            });
        }
        Op::TryEnd => {
            frame.handlers.pop();
        }
        Op::IterInit => {
            // `OP_for_of_start` (quickjs.c:19318): obtain the iterator without
            // draining it. Fast paths for engine-internal iterables avoid the
            // `Symbol.iterator` call, exactly like QuickJS' checks for its
            // own iterator classes.
            let value = pop(&mut frame.stack)?;
            let iterator = match &value {
                JsValue::String(text) => {
                    let text = text.clone();
                    JsValue::Object(realm.string_iterator(text))
                }
                JsValue::Object(object) => {
                    let data = object.borrow();
                    match &data.kind {
                        ObjectKind::Array(_) => {
                            let array = object.clone();
                            drop(data);
                            JsValue::Object(realm.array_iterator(array, false))
                        }
                        ObjectKind::TypedArray(typed) => {
                            let items = typed.values();
                            drop(data);
                            JsValue::Object(realm.iterator(items))
                        }
                        ObjectKind::Map(entries) => {
                            let entries: Vec<(JsValue, JsValue)> = entries
                                .iter()
                                .map(|(key, value)| (key.0.clone(), value.clone()))
                                .collect();
                            drop(data);
                            JsValue::Object(
                                realm.iterator(
                                    entries
                                        .into_iter()
                                        .map(|(key, value)| {
                                            JsValue::Object(realm.array(vec![key, value]))
                                        })
                                        .collect(),
                                ),
                            )
                        }
                        ObjectKind::Set(values) => {
                            let values: Vec<JsValue> =
                                values.iter().map(|key| key.0.clone()).collect();
                            drop(data);
                            JsValue::Object(realm.iterator(values))
                        }
                        ObjectKind::Iterator { .. } | ObjectKind::Generator(_) => value.clone(),
                        _ => {
                            drop(data);
                            super::iter::protocol_iterator(realm, &value)?
                        }
                    }
                }
                _ => {
                    return Err(realm.type_error("Value is not iterable"));
                }
            };
            frame.stack.push(iterator);
        }
        Op::ForInInit => {
            let value = pop(&mut frame.stack)?;
            let mut keys = Vec::new();
            if let JsValue::Object(object) = &value {
                let enumerable = object.borrow().enumerable_keys();
                for key in enumerable {
                    if let Some(text) = key.to_rc_string() {
                        keys.push(JsValue::String(text));
                    }
                }
            }
            frame.stack.push(JsValue::Object(realm.iterator(keys)));
        }
        Op::IterNext(exit) => {
            // `OP_for_of_next` (quickjs.c:19318): pull exactly one item. Fast
            // paths cover the engine-internal iterators; everything else
            // goes through the iterator protocol.
            let length = frame.stack.len();
            if length == 0 {
                return Err(realm.type_error("Missing iterator"));
            }
            let iterator = frame.stack[length - 1].clone();
            let next = super::iter::advance_iterator(realm, &iterator)?;
            match next {
                Some(value) => frame.stack.push(value),
                None => {
                    frame.stack.pop();
                    *pc = exit as usize;
                }
            }
        }
        Op::IterClose => {
            pop(&mut frame.stack)?;
        }
        Op::CreateClass(index) => {
            let parent = pop(&mut frame.stack)?;
            let class = class_constant(&frame.definition, index)?;
            let env = frame.env.clone().unwrap_or_else(|| Env::new(0, None));
            let value = create_class(realm, &class, &env, parent)?;
            frame.stack.push(value);
        }
        Op::ExportAll => {
            let source = pop(&mut frame.stack)?;
            let target = pop(&mut frame.stack)?;
            let (JsValue::Object(source), JsValue::Object(target)) = (source, target) else {
                return Ok(Step::Continue);
            };
            let names: Vec<PropKey> = source.borrow().shape.keys.clone();
            let receiver = JsValue::Object(target.clone());
            for key in names {
                // `export *` never re-exports `default`.
                if key == PropKey::from("default") {
                    continue;
                }
                let value = source
                    .borrow()
                    .get_own_value(&key)
                    .unwrap_or(JsValue::Undefined);
                target
                    .borrow_mut()
                    .put_property(realm, key, value, &receiver)?;
            }
        }
        Op::SuperCall(argc) => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let callee = frame
                .callee
                .clone()
                .ok_or_else(|| realm.reference_error("super is not available"))?;
            let parent = get_value_property(realm, &callee, &PropKey::from("__super__"))?;
            if parent.is_undefined() {
                frame.stack.push(JsValue::Undefined);
            } else {
                // A base-class parent initializes its fields before its body;
                // a derived parent does so in its own `super()` call.
                let parent_is_derived = parent
                    .as_object()
                    .and_then(|object| object.to_ref().borrow().function())
                    .is_some_and(|function| function.has_super);
                let receiver = frame.this_value(realm);
                if !parent_is_derived {
                    super::call::run_instance_initializer(realm, &parent, &receiver)?;
                }
                call_value(realm, parent.clone(), receiver.clone(), &arguments)?;
                // Fields declared by this class initialize after `super()`.
                super::call::run_instance_initializer(realm, &callee, &receiver)?;
                frame.stack.push(receiver);
            }
        }
    }
    Ok(Step::Continue)
}

/// Fast i32+i32; overflow falls back to the Number path in `add_values`.
#[inline]
fn add_ints(left: &JsValue, right: &JsValue) -> Option<JsValue> {
    if let (JsValue::Int(left), JsValue::Int(right)) = (left, right) {
        let sum = *left as i64 + *right as i64;
        if (i32::MIN as i64..=i32::MAX as i64).contains(&sum) {
            Some(JsValue::Int(sum as i32))
        } else {
            None
        }
    } else {
        None
    }
}

/// Rare numeric fallbacks live out of line so the inlined dispatch loop does
/// not need to preserve floating-point registers on every JS call.
#[inline(never)]
fn int_step_overflow(value: i32, delta: i32) -> JsValue {
    JsValue::number(value as f64 + delta as f64)
}

#[inline(never)]
fn numeric_value(realm: &mut Realm, value: JsValue) -> JsResult<f64> {
    super::to_numeric(realm, value)
}

#[inline(never)]
fn int32_value(realm: &mut Realm, value: JsValue) -> JsResult<i32> {
    Ok(super::to_int32(super::to_numeric(realm, value)?))
}

#[inline(never)]
fn numeric_step(realm: &mut Realm, value: JsValue, delta: f64) -> JsResult<JsValue> {
    Ok(JsValue::number(super::to_numeric(realm, value)? + delta))
}

fn binary_number(
    realm: &mut Realm,
    frame: &mut Frame,
    operation: impl Fn(f64, f64) -> f64,
) -> JsResult<()> {
    let right = pop(&mut frame.stack)?;
    let left = pop(&mut frame.stack)?;
    if let (JsValue::Int(left), JsValue::Int(right)) = (&left, &right) {
        let value = operation(*left as f64, *right as f64);
        frame.stack.push(JsValue::number(value));
        return Ok(());
    }
    let value = operation(
        super::to_numeric(realm, left)?,
        super::to_numeric(realm, right)?,
    );
    frame.stack.push(JsValue::number(value));
    Ok(())
}

#[inline]
fn binary_int(
    realm: &mut Realm,
    frame: &mut Frame,
    operation: impl Fn(i32, i32) -> i32,
) -> JsResult<()> {
    let right = pop(&mut frame.stack)?;
    let left = pop(&mut frame.stack)?;
    if let (JsValue::Int(left), JsValue::Int(right)) = (&left, &right) {
        frame.stack.push(JsValue::Int(operation(*left, *right)));
        return Ok(());
    }
    let value = operation(
        super::to_int32(super::to_numeric(realm, left)?),
        super::to_int32(super::to_numeric(realm, right)?),
    );
    frame.stack.push(JsValue::Int(value));
    Ok(())
}

#[inline(always)]
fn compare_ints(left: i32, right: i32, op: CompareOp) -> bool {
    match op {
        CompareOp::Lt => left < right,
        CompareOp::Le => left <= right,
        CompareOp::Gt => left > right,
        CompareOp::Ge => left >= right,
    }
}

#[inline(never)]
fn compare_values(
    realm: &mut Realm,
    left: &JsValue,
    right: &JsValue,
    op: CompareOp,
) -> JsResult<bool> {
    if let (JsValue::Int(left), JsValue::Int(right)) = (left, right) {
        return Ok(compare_ints(*left, *right, op));
    }
    let ordering = crate::coerce::abstract_compare(realm, left, right)?;
    Ok(ordering.is_some_and(|ordering| match op {
        CompareOp::Lt => ordering < 0,
        CompareOp::Le => ordering <= 0,
        CompareOp::Gt => ordering > 0,
        CompareOp::Ge => ordering >= 0,
    }))
}

#[inline]
fn compare_op(
    realm: &mut Realm,
    frame: &mut Frame,
    predicate: impl Fn(i32) -> bool,
) -> JsResult<()> {
    let right = pop(&mut frame.stack)?;
    let left = pop(&mut frame.stack)?;
    let ordering = if let (JsValue::Int(left), JsValue::Int(right)) = (&left, &right) {
        Some(left.cmp(right) as i32)
    } else {
        crate::coerce::abstract_compare(realm, &left, &right)?
    };
    frame
        .stack
        .push(JsValue::Bool(ordering.is_some_and(predicate)));
    Ok(())
}

#[inline(always)]
pub(crate) fn pop(stack: &mut ValueStack) -> JsResult<JsValue> {
    stack
        .pop()
        .ok_or_else(|| JsValue::string("InternalError: stack underflow"))
}

pub(crate) fn peek(stack: &[JsValue]) -> JsResult<&JsValue> {
    stack
        .last()
        .ok_or_else(|| JsValue::string("InternalError: stack underflow"))
}

#[inline(always)]
fn pop_arguments(stack: &mut ValueStack, count: usize) -> JsResult<ArgumentStack> {
    let len = stack.len();
    if len < count {
        return Err(JsValue::string("InternalError: stack underflow"));
    }
    if count == 0 {
        return Ok(ArgumentStack::new());
    }
    let mut values = ArgumentStack::with_capacity(count);
    for _ in 0..count {
        values.push(stack.pop().unwrap_or(JsValue::Undefined));
    }
    values.reverse();
    Ok(values)
}

fn pop_array(stack: &mut ValueStack) -> JsResult<Vec<JsValue>> {
    let value = pop(stack)?;
    match value {
        JsValue::Object(object) => {
            let data = object.borrow();
            Ok(data
                .array_items()
                .map(|items| items.to_vec())
                .unwrap_or_default())
        }
        _ => Ok(Vec::new()),
    }
}

#[inline]
fn constant_value(
    realm: &mut Realm,
    definition: &FunctionDef,
    index: u32,
    env: Option<&EnvRef>,
) -> JsResult<JsValue> {
    match definition.constants.get(index as usize) {
        Some(Constant::Int(value)) => Ok(JsValue::Int(*value)),
        Some(Constant::Number(value)) => Ok(JsValue::number(*value)),
        Some(Constant::String(value)) => Ok(JsValue::String(value.clone())),
        Some(Constant::Function(function)) => Ok(realm.make_js_function(
            function.clone(),
            env.expect("function constant requires an environment")
                .clone(),
            None,
        )),
        Some(Constant::Class(class)) => {
            let value = create_class(
                realm,
                class,
                env.expect("class constant requires an environment"),
                JsValue::Undefined,
            )?;
            Ok(value)
        }
        Some(Constant::Strings(_)) => Err(JsValue::string(
            "InternalError: invalid string-array constant",
        )),
        Some(Constant::Regex { source, flags }) => {
            let regex =
                regress::Regex::with_flags(source.as_ref(), flags.as_ref()).map_err(|error| {
                    realm.syntax_error(format!("Invalid regular expression: {error}"))
                })?;
            let data = Rc::new(RegExpData {
                source: source.to_string(),
                flags: flags.to_string(),
                regex: Rc::new(regex),
            });
            let object = ObjectData::regexp(data, Some(realm.regexp_proto.clone()));
            let object = Rc::new(std::cell::RefCell::new(object));
            Ok(JsValue::Object(object))
        }
        None => Err(JsValue::string("InternalError: invalid constant")),
    }
}

fn closure_value(
    realm: &mut Realm,
    _definition: &FunctionDef,
    index: u32,
    env: &EnvRef,
    this: JsValue,
) -> JsResult<JsValue> {
    let definition = _definition
        .constants
        .get(index as usize)
        .ok_or_else(|| realm.type_error("Invalid closure constant"))?;
    match definition {
        Constant::Function(function) => {
            Ok(realm.make_js_function(function.clone(), env.clone(), Some(this)))
        }
        _ => Err(realm.type_error("Invalid closure constant")),
    }
}
