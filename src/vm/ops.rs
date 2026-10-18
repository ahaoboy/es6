//! Opcode dispatch and operand-stack helpers.
//!
//! This is the Rust counterpart of the giant switch in QuickJS
//! `JS_CallInternal` (quickjs.c:18051); the fused operands mirror QuickJS'
//! short opcodes (`quickjs-opcode.h`).

use std::rc::Rc;

use super::frame::{EventLoop, Frame, Handler, Step};
use super::{ArgumentStack, ValueStack, constant_string, constant_strings};
use crate::bytecode::{CompareOp, Constant, FunctionDef, Op};
use crate::object::{
    Object, ObjectData, ObjectKind, Property, PropKey, RegExpData,
};
use crate::value::JsValue;
use crate::runtime::Realm;
use crate::value::{Env, EnvRef, JsResult};

use super::call::{
    call_value, call_value_owned, class_constant, construct_value, create_class, super_function,
};
use super::iter::{iterable_items, push_array_value};
use super::property::{
    cached_get_property, cached_set_property, get_element_value, get_value_property,
    pop_property_key, set_element_value, set_value_property,
};

/// Opcode dispatch. This is the Rust counterpart of the giant switch in
/// QuickJS `JS_CallInternal` (quickjs.c:18051).
#[inline(always)]
pub(crate) fn execute_op(realm: &mut Realm, frame: &mut Frame, op: Op) -> JsResult<Step> {
    match op {
        Op::Nop => {}
        Op::Undefined => frame.stack.push(JsValue::Undefined),
        Op::Null => frame.stack.push(JsValue::Null),
        Op::True => frame.stack.push(JsValue::Bool(true)),
        Op::False => frame.stack.push(JsValue::Bool(false)),
        Op::This => frame.stack.push(frame.this.clone()),
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
        Op::GetGlobal(index) => {
            let name = constant_string(&frame.definition, index);
            match super::property::global_get(realm, &name) {
                Some(value) => frame.stack.push(value),
                None => return Err(realm.reference_error(format!("{name} is not defined"))),
            }
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
            let key = property_key_from_constant(&frame.definition, key)?;
            let object = pop(&mut frame.stack)?;
            let value = match frame.definition.prop_caches.get(cache as usize) {
                Some(cache) => cached_get_property(realm, &object, &key, cache)?,
                None => get_value_property(realm, &object, &key)?,
            };
            frame.stack.push(value);
        }
        Op::SetPropConst { key, cache } => {
            let value = pop(&mut frame.stack)?;
            let key = property_key_from_constant(&frame.definition, key)?;
            let object = pop(&mut frame.stack)?;
            match frame.definition.prop_caches.get(cache as usize) {
                Some(cache) => cached_set_property(realm, &object, &key, &value, cache)?,
                None => set_value_property(realm, &object, &key, value.clone(), true)?,
            }
            frame.stack.push(value);
        }
        Op::SetPropConstKeep { key, cache } => {
            let value = pop(&mut frame.stack)?;
            let key = property_key_from_constant(&frame.definition, key)?;
            let object = pop(&mut frame.stack)?;
            match frame.definition.prop_caches.get(cache as usize) {
                Some(cache) => cached_set_property(realm, &object, &key, &value, cache)?,
                None => set_value_property(realm, &object, &key, value, true)?,
            }
            frame.stack.push(object);
        }
        Op::DefineAccessor { is_getter } => {
            let function = pop(&mut frame.stack)?;
            let key = pop_property_key(realm, &mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            if let JsValue::Object(object) = object {
                let mut data = object.borrow_mut();
                data.shape = data.shape.wrapping_add(1);
                if data.is_prototype {
                    crate::object::bump_prototype_generation();
                }
                let property = data.props.entry(key).or_insert_with(|| Property {
                    value: JsValue::Undefined,
                    get: None,
                    set: None,
                    writable: false,
                    enumerable: true,
                    configurable: true,
                });
                if is_getter {
                    property.get = Some(Box::new(function));
                } else {
                    property.set = Some(Box::new(function));
                }
            } else {
                return Err(realm.type_error("Cannot define accessor on primitive"));
            }
        }
        Op::NewObject => frame.stack.push(JsValue::Object(realm.object())),
        Op::NewObjectFrom { keys, count } => {
            let count = count as usize;
            if frame.stack.len() < count {
                return Err(realm.type_error("Stack underflow"));
            }
            let keys = constant_strings(&frame.definition, keys);
            let start = frame.stack.len() - count;
            let values = frame.stack[start..].to_vec();
            frame.stack.truncate(start);
            let object = realm.object();
            {
                let mut data = object.borrow_mut();
                data.props = crate::object::PropertyMap::with_capacity_and_hasher(
                    count,
                    rustc_hash::FxBuildHasher,
                );
                for (key, value) in keys.iter().zip(values) {
                    data.props.insert(
                        crate::object::key_from_string(key.clone()),
                        Property::data(value),
                    );
                }
                data.shape = 1;
            }
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
        Op::CallMethod(argc) => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let function = pop(&mut frame.stack)?;
            let this = pop(&mut frame.stack)?;
            let value = call_value_owned(realm, function, this, arguments)?;
            frame.stack.push(value);
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
            let value = call_value_owned(realm, function, frame.this.clone(), arguments)?;
            frame.stack.push(value);
        }
        Op::CallSuperMethodSpread => {
            let arguments = pop_array(&mut frame.stack)?;
            let key = pop_property_key(realm, &mut frame.stack)?;
            let function = super_function(realm, frame, &key)?;
            let value = call_value(realm, function, frame.this.clone(), &arguments)?;
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
        Op::Return => {
            let value = pop(&mut frame.stack)?;
            return Ok(Step::Return(value));
        }
        Op::ReturnUndefined => return Ok(Step::Return(JsValue::Undefined)),
        Op::Throw => {
            let value = pop(&mut frame.stack)?;
            return Err(value);
        }
        Op::Jump(target) => frame.pc = target,
        Op::JumpIfFalse(target) => {
            let value = pop(&mut frame.stack)?;
            if !super::to_boolean(&value) {
                frame.pc = target;
            }
        }
        Op::JumpIfTrue(target) => {
            let value = pop(&mut frame.stack)?;
            if super::to_boolean(&value) {
                frame.pc = target;
            }
        }
        Op::JumpIfNotUndefined(target) => {
            let value = pop(&mut frame.stack)?;
            if !matches!(value, JsValue::Undefined) {
                frame.pc = target;
            }
        }
        Op::JumpIfNullish(target) => {
            let value = pop(&mut frame.stack)?;
            if value.is_nullish() {
                frame.pc = target;
            }
        }
        Op::JumpIfNullishKeep(target) => {
            let value = peek(&frame.stack)?;
            if value.is_nullish() {
                frame.pc = target;
            } else {
                pop(&mut frame.stack)?;
            }
        }
        Op::AddVar {
            dst_depth,
            dst_slot,
            src_depth,
            src_slot,
        } => {
            let left = frame.get_var(dst_depth, dst_slot);
            let right = frame.get_var(src_depth, src_slot);
            let value = add_ints(&left, &right).unwrap_or_else(|| JsValue::Undefined);
            let value = match value {
                JsValue::Undefined => super::add_values(realm, left, right)?,
                value => value,
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
        Op::UpdateVarJump {
            depth,
            slot,
            delta,
            target,
        } => {
            let value = frame.get_var(depth, slot);
            let number = super::to_numeric(realm, value)?;
            frame.set_var(depth, slot, JsValue::number(number + delta as f64));
            frame.pc = target;
        }
        Op::Add => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            let value = match add_ints(&left, &right) {
                Some(value) => value,
                None => super::add_values(realm, left, right)?,
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
            } else {
                let value = super::to_numeric(realm, left)? * super::to_numeric(realm, right)?;
                frame.stack.push(JsValue::number(value));
            }
        }
        Op::Div => binary_number(realm, frame, |a, b| a / b)?,
        Op::Mod => binary_number(realm, frame, |a, b| a % b)?,
        Op::Exp => binary_number(realm, frame, |a, b| a.powf(b))?,
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
            let result = match object {
                JsValue::Object(object) => object.borrow().has_property(&key),
                _ => return Err(realm.type_error("Right-hand side of 'in' is not an object")),
            };
            frame.stack.push(JsValue::Bool(result));
        }
        Op::InstanceOf => {
            let constructor = pop(&mut frame.stack)?;
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::Bool(
                crate::coerce::instance_of(realm, &value, &constructor)?,
            ));
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
            frame.stack.push(JsValue::number(super::to_numeric(realm, value)?));
        }
        Op::BitNot => {
            let value = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::Int(!super::to_int32(super::to_numeric(realm, value)?)));
        }
        Op::TypeOf => {
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::string(value.type_of()));
        }
        Op::TypeOfGlobal(index) => {
            let name = constant_string(&frame.definition, index);
            let value = super::property::global_get(realm, &name);
            frame.stack.push(JsValue::string(match value {
                Some(value) => value.type_of(),
                None => "undefined",
            }));
        }
        Op::Void => {
            pop(&mut frame.stack)?;
            frame.stack.push(JsValue::Undefined);
        }
        Op::Await => {
            let value = pop(&mut frame.stack)?;
            return Ok(Step::Await(value));
        }
        Op::IncVar {
            depth,
            slot,
            prefix,
        } => {
            let old = frame.get_var(depth, slot);
            if let JsValue::Int(value) = old {
                let new = JsValue::number(value as f64 + 1.0);
                frame.set_var(depth, slot, new.clone());
                frame
                    .stack
                    .push(if prefix { new } else { JsValue::Int(value) });
            } else {
                let number = super::to_numeric(realm, old)?;
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
                let new = JsValue::number(value as f64 - 1.0);
                frame.set_var(depth, slot, new.clone());
                frame
                    .stack
                    .push(if prefix { new } else { JsValue::Int(value) });
            } else {
                let number = super::to_numeric(realm, old)?;
                let new = JsValue::number(number - 1.0);
                frame.set_var(depth, slot, new.clone());
                frame
                    .stack
                    .push(if prefix { new } else { JsValue::number(number) });
            }
        }
        Op::IncGlobal(index, prefix) => {
            let name = constant_string(&frame.definition, index);
            let old = super::property::global_get(realm, &name).unwrap_or(JsValue::Undefined);
            let number = super::to_numeric(realm, old)?;
            let new = JsValue::number(number + 1.0);
            Object::from_ref(realm.global.clone()).set_key(PropKey::Str(name), new.clone());
            frame
                .stack
                .push(if prefix { new } else { JsValue::number(number) });
        }
        Op::DecGlobal(index, prefix) => {
            let name = constant_string(&frame.definition, index);
            let old = super::property::global_get(realm, &name).unwrap_or(JsValue::Undefined);
            let number = super::to_numeric(realm, old)?;
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
            let number = super::to_numeric(realm, old)?;
            let new = JsValue::number(number + delta);
            set_value_property(realm, &object, &key, new.clone(), true)?;
            frame
                .stack
                .push(if prefix { new } else { JsValue::number(number) });
        }
        Op::PushEnv(size) => match frame.event_loop {
            EventLoop::Env => {
                frame.env = Some(Env::new(size as usize, Some(frame.env().clone())));
            }
            EventLoop::Locals => {
                let offset = frame.locals.len();
                let size = size as usize;
                frame.locals.resize(offset + size, JsValue::Undefined);
                frame.local_frames.push((offset, size));
            }
        },
        Op::PopEnv => {
            if frame.event_loop == EventLoop::Env {
                let parent = frame.env().parent.clone();
                match parent {
                    Some(parent) => frame.env = Some(parent),
                    None => return Err(realm.type_error("Environment underflow")),
                }
            } else if frame.local_frames.len() > 1 {
                let (offset, _) = frame.local_frames.pop().unwrap();
                frame.locals.truncate(offset);
            } else {
                return Err(realm.type_error("Environment underflow"));
            }
        }
        Op::Closure(index) => {
            let value = closure_value(
                realm,
                &frame.definition,
                index,
                frame.env(),
                frame.this.clone(),
            )?;
            frame.stack.push(value);
        }
        Op::TryStart(catch_pc) => {
            frame.handlers.push(Handler {
                catch_pc,
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
            let value = pop(&mut frame.stack)?;
            let items = iterable_items(realm, value)?;
            frame.stack.push(JsValue::Object(realm.iterator(items)));
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
            let length = frame.stack.len();
            if length == 0 {
                return Err(realm.type_error("Missing iterator"));
            }
            let value = {
                let JsValue::Object(object) = &frame.stack[length - 1] else {
                    return Err(realm.type_error("Value is not an iterator"));
                };
                let mut data = object.borrow_mut();
                match &mut data.kind {
                    ObjectKind::Iterator { items, index } => {
                        if *index < items.len() {
                            let value = items[*index].clone();
                            *index += 1;
                            Some(value)
                        } else {
                            None
                        }
                    }
                    _ => return Err(realm.type_error("Value is not an iterator")),
                }
            };
            match value {
                Some(value) => frame.stack.push(value),
                None => {
                    frame.stack.pop();
                    frame.pc = exit;
                }
            }
        }
        Op::IterClose => {
            pop(&mut frame.stack)?;
        }
        Op::CreateClass(index) => {
            let parent = pop(&mut frame.stack)?;
            let class = class_constant(&frame.definition, index)?;
            let value = create_class(realm, &class, frame.env(), parent)?;
            frame.stack.push(value);
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
                call_value(realm, parent.clone(), frame.this.clone(), &arguments)?;
                super::call::run_instance_initializer(realm, &parent, &frame.this)?;
                frame.stack.push(frame.this.clone());
            }
        }
    }
    Ok(Step::Continue)
}

/// Fast i32+i32 with overflow promotion, shared by the fused add opcodes.
#[inline]
fn add_ints(left: &JsValue, right: &JsValue) -> Option<JsValue> {
    if let (JsValue::Int(left), JsValue::Int(right)) = (left, right) {
        let sum = *left as i64 + *right as i64;
        if (i32::MIN as i64..=i32::MAX as i64).contains(&sum) {
            Some(JsValue::Int(sum as i32))
        } else {
            Some(JsValue::Float(sum as f64))
        }
    } else {
        None
    }
}

fn binary_number(
    realm: &mut Realm,
    frame: &mut Frame,
    operation: impl Fn(f64, f64) -> f64,
) -> JsResult<()> {
    let right = pop(&mut frame.stack)?;
    let left = pop(&mut frame.stack)?;
    let value = operation(super::to_numeric(realm, left)?, super::to_numeric(realm, right)?);
    frame.stack.push(JsValue::number(value));
    Ok(())
}

fn binary_int(
    realm: &mut Realm,
    frame: &mut Frame,
    operation: impl Fn(i32, i32) -> i32,
) -> JsResult<()> {
    let right = pop(&mut frame.stack)?;
    let left = pop(&mut frame.stack)?;
    let value = operation(
        super::to_int32(super::to_numeric(realm, left)?),
        super::to_int32(super::to_numeric(realm, right)?),
    );
    frame.stack.push(JsValue::Int(value));
    Ok(())
}

#[inline]
fn compare_values(
    realm: &mut Realm,
    left: &JsValue,
    right: &JsValue,
    op: CompareOp,
) -> JsResult<bool> {
    let ordering = if let (JsValue::Int(left), JsValue::Int(right)) = (left, right) {
        Some(left.cmp(right) as i32)
    } else {
        crate::coerce::abstract_compare(realm, left, right)?
    };
    Ok(ordering.is_some_and(|ordering| match op {
        CompareOp::Lt => ordering < 0,
        CompareOp::Le => ordering <= 0,
        CompareOp::Gt => ordering > 0,
        CompareOp::Ge => ordering >= 0,
    }))
}

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

fn pop_arguments(stack: &mut ValueStack, count: usize) -> JsResult<ArgumentStack> {
    if stack.len() < count {
        return Err(JsValue::string("InternalError: stack underflow"));
    }
    let start = stack.len() - count;
    let values = stack.drain(start..).collect();
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

fn property_key_from_constant(definition: &FunctionDef, index: u32) -> JsResult<PropKey> {
    match definition.constants.get(index as usize) {
        Some(Constant::String(value)) => Ok(crate::object::key_from_string(value.clone())),
        Some(Constant::Number(value)) => Ok(PropKey::Str(Rc::from(
            super::format_number(*value),
        ))),
        _ => Err(JsValue::string("InternalError: invalid property constant")),
    }
}

fn constant_value(
    realm: &mut Realm,
    definition: &FunctionDef,
    index: u32,
    env: Option<&EnvRef>,
) -> JsResult<JsValue> {
    match definition.constants.get(index as usize) {
        Some(Constant::Number(value)) => Ok(JsValue::number(*value)),
        Some(Constant::String(value)) => Ok(JsValue::String(value.clone())),
        Some(Constant::Function(function)) => Ok(realm.make_js_function(
            function.clone(),
            env.expect("function constant requires an environment").clone(),
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
