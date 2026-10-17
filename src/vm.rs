use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::bytecode::{ClassDef, Constant, FunctionDef, Op};
use crate::object::{
    ObjRef, Object, ObjectData, ObjectKind, PropKey, Property, RegExpData, key_to_index,
};
use crate::runtime::Realm;
use crate::value::{Env, EnvRef, JsResult, JsValue};

enum Step {
    Continue,
    Return(JsValue),
}

struct Handler {
    catch_pc: usize,
    stack_len: usize,
    env: EnvRef,
    index: usize,
}

/// Activation record. QuickJS' equivalent is `JSStackFrame` (quickjs.c:900
/// area) with `var_refs`, `sp` and the operand stack in `JS_CallInternal`
/// (quickjs.c:963).
struct Frame<'a> {
    definition: &'a FunctionDef,
    env: EnvRef,
    this: JsValue,
    callee: Option<JsValue>,
    stack: Vec<JsValue>,
    arguments: Vec<JsValue>,
    handlers: Vec<Handler>,
    pc: usize,
}

thread_local! {
    /// Byte offset of the innermost uncaught VM exception. The VM keeps this
    /// out of `JsValue` so thrown values remain ordinary JavaScript values.
    static ERROR_OFFSET: Cell<Option<usize>> = const { Cell::new(None) };
}

pub fn take_error_offset() -> Option<usize> {
    ERROR_OFFSET.with(Cell::take)
}

pub fn clear_error_offset() {
    ERROR_OFFSET.with(|offset| offset.set(None));
}

pub fn execute(
    realm: &mut Realm,
    definition: &FunctionDef,
    env: EnvRef,
    this: JsValue,
) -> JsResult<JsValue> {
    ERROR_OFFSET.with(|offset| offset.set(None));
    run_frame(realm, definition, env, this, None, Vec::new())
}

fn run_frame(
    realm: &mut Realm,
    definition: &FunctionDef,
    env: EnvRef,
    this: JsValue,
    callee: Option<JsValue>,
    arguments: Vec<JsValue>,
) -> JsResult<JsValue> {
    let mut frame = Frame {
        definition,
        env,
        this,
        callee,
        stack: Vec::with_capacity(16),
        arguments,
        handlers: Vec::new(),
        pc: 0,
    };
    loop {
        if frame.pc >= definition.code.len() {
            return Ok(JsValue::Undefined);
        }
        let op = definition.code[frame.pc].clone();
        frame.pc += 1;
        let result = execute_op(realm, &mut frame, op);
        match result {
            Ok(Step::Continue) => {}
            Ok(Step::Return(value)) => return Ok(value),
            Err(exception) => {
                if let Some(handler) = frame.handlers.pop() {
                    ERROR_OFFSET.with(|offset| offset.set(None));
                    frame.handlers.truncate(handler.index);
                    frame.stack.truncate(handler.stack_len);
                    frame.env = handler.env;
                    frame.stack.push(exception);
                    frame.pc = handler.catch_pc;
                } else {
                    if let Some(offset) = definition.positions.get(frame.pc - 1) {
                        ERROR_OFFSET.with(|current| {
                            if current.get().is_none() {
                                current.set(Some(*offset as usize));
                            }
                        });
                    }
                    return Err(exception);
                }
            }
        }
    }
}

/// Opcode dispatch. This is the Rust counterpart of the giant switch in
/// QuickJS `JS_CallInternal` (quickjs.c:963).
fn execute_op(realm: &mut Realm, frame: &mut Frame, op: Op) -> JsResult<Step> {
    match op {
        Op::Nop => {}
        Op::Undefined => frame.stack.push(JsValue::Undefined),
        Op::Null => frame.stack.push(JsValue::Null),
        Op::True => frame.stack.push(JsValue::Bool(true)),
        Op::False => frame.stack.push(JsValue::Bool(false)),
        Op::This => frame.stack.push(frame.this.clone()),
        Op::PushConst(index) => {
            let value = constant_value(realm, frame.definition, index, &frame.env)?;
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
                .push(JsValue::Object(realm.array(frame.arguments.clone())));
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
            let value = frame.env.get(depth as usize, slot as usize);
            frame.stack.push(value);
        }
        Op::SetVar { depth, slot } => {
            let value = pop(&mut frame.stack)?;
            frame.env.set(depth as usize, slot as usize, value);
        }
        Op::GetGlobal(name) => {
            let value = global_get(realm, &name);
            match value {
                Some(value) => frame.stack.push(value),
                None => return Err(realm.reference_error(format!("{name} is not defined"))),
            }
        }
        Op::SetGlobal(name) => {
            let value = pop(&mut frame.stack)?;
            Object::from_ref(realm.global.clone()).set_key(PropKey::Str(name), value);
        }
        Op::DeleteGlobal(name) => {
            let deleted = Object::from_ref(realm.global.clone()).delete(&name);
            frame.stack.push(JsValue::Bool(deleted));
        }
        Op::GetProp => {
            let key = pop_property_key(realm, &mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            let value = get_value_property(realm, &object, &key)?;
            frame.stack.push(value);
        }
        Op::SetProp => {
            let value = pop(&mut frame.stack)?;
            let key = pop_property_key(realm, &mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            set_value_property(realm, &object, &key, value.clone(), true)?;
            frame.stack.push(value);
        }
        Op::SetPropKeep => {
            let value = pop(&mut frame.stack)?;
            let key = pop_property_key(realm, &mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            set_value_property(realm, &object, &key, value, true)?;
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
        Op::GetPropConst(index) => {
            let key = property_key_from_constant(frame.definition, index)?;
            let object = pop(&mut frame.stack)?;
            let value = get_value_property(realm, &object, &key)?;
            frame.stack.push(value);
        }
        Op::SetPropConst(index) => {
            let value = pop(&mut frame.stack)?;
            let key = property_key_from_constant(frame.definition, index)?;
            let object = pop(&mut frame.stack)?;
            set_value_property(realm, &object, &key, value.clone(), true)?;
            frame.stack.push(value);
        }
        Op::SetPropConstKeep(index) => {
            let value = pop(&mut frame.stack)?;
            let key = property_key_from_constant(frame.definition, index)?;
            let object = pop(&mut frame.stack)?;
            set_value_property(realm, &object, &key, value, true)?;
            frame.stack.push(object);
        }
        Op::DefineAccessor { is_getter } => {
            let function = pop(&mut frame.stack)?;
            let key = pop_property_key(realm, &mut frame.stack)?;
            let object = pop(&mut frame.stack)?;
            if let JsValue::Object(object) = object {
                let mut data = object.borrow_mut();
                let property = data.props.entry(key).or_insert_with(|| Property {
                    value: JsValue::Undefined,
                    get: None,
                    set: None,
                    writable: false,
                    enumerable: true,
                    configurable: true,
                });
                if is_getter {
                    property.get = Some(function);
                } else {
                    property.set = Some(function);
                }
            } else {
                return Err(realm.type_error("Cannot define accessor on primitive"));
            }
        }
        Op::NewObject => frame.stack.push(JsValue::Object(realm.object())),
        Op::NewArray(count) => {
            let count = count as usize;
            if frame.stack.len() < count {
                return Err(realm.type_error("Stack underflow"));
            }
            let items = frame.stack.split_off(frame.stack.len() - count);
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
                copy_own_properties(realm, target, source)?;
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
        Op::ObjectRest(excluded) => {
            let value = pop(&mut frame.stack)?;
            let rest = realm.object();
            if let JsValue::Object(source) = &value {
                let keys = source.borrow().enumerable_keys();
                for key in keys {
                    if let Some(name) = key.as_str()
                        && excluded.iter().any(|excluded| excluded.as_ref() == name)
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
            let value = call_value(realm, function, JsValue::Undefined, &arguments)?;
            frame.stack.push(value);
        }
        Op::CallMethod(argc) => {
            let arguments = pop_arguments(&mut frame.stack, argc as usize)?;
            let function = pop(&mut frame.stack)?;
            let this = pop(&mut frame.stack)?;
            let value = call_value(realm, function, this, &arguments)?;
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
            let value = call_value(realm, function, frame.this.clone(), &arguments)?;
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
            if !to_boolean(&value) {
                frame.pc = target;
            }
        }
        Op::JumpIfTrue(target) => {
            let value = pop(&mut frame.stack)?;
            if to_boolean(&value) {
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
        Op::Add => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            frame.stack.push(add_values(realm, left, right)?);
        }
        Op::Sub => binary_number(realm, frame, |a, b| a - b)?,
        Op::Mul => binary_number(realm, frame, |a, b| a * b)?,
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
            let value = to_uint32(to_number(left)?) >> (to_uint32(to_number(right)?) & 31);
            frame.stack.push(JsValue::number(value as f64));
        }
        Op::Eq => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::Bool(loose_equals(realm, &left, &right)?));
        }
        Op::Ne => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::Bool(!loose_equals(realm, &left, &right)?));
        }
        Op::StrictEq => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::Bool(strict_equals(&left, &right)));
        }
        Op::StrictNe => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::Bool(!strict_equals(&left, &right)));
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
            frame
                .stack
                .push(JsValue::Bool(instance_of(realm, &value, &constructor)?));
        }
        Op::Not => {
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::Bool(!to_boolean(&value)));
        }
        Op::Neg => {
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::number(-to_number(value)?));
        }
        Op::Pos => {
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::number(to_number(value)?));
        }
        Op::BitNot => {
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::Int(!to_int32(to_number(value)?)));
        }
        Op::TypeOf => {
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::string(value.type_of()));
        }
        Op::TypeOfGlobal(name) => {
            let value = global_get(realm, &name);
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
            let value = if let JsValue::Object(object) = &value {
                let promise = {
                    let data = object.borrow();
                    match &data.kind {
                        ObjectKind::Promise(promise) => Some(promise.clone()),
                        _ => None,
                    }
                };
                if let Some(promise) = promise {
                    match promise.state {
                        crate::object::PromiseState::Fulfilled => promise.value,
                        crate::object::PromiseState::Rejected => return Err(promise.value),
                        crate::object::PromiseState::Pending => JsValue::Undefined,
                    }
                } else {
                    value
                }
            } else {
                value
            };
            frame.stack.push(value);
        }
        Op::IncVar {
            depth,
            slot,
            prefix,
        } => {
            let _ = &op;
            let old = frame.env.get(depth as usize, slot as usize);
            let number = to_number(old)?;
            let new = JsValue::number(number + 1.0);
            frame.env.set(depth as usize, slot as usize, new.clone());
            frame
                .stack
                .push(if prefix { new } else { JsValue::number(number) });
        }
        Op::DecVar {
            depth,
            slot,
            prefix,
        } => {
            let old = frame.env.get(depth as usize, slot as usize);
            let number = to_number(old)?;
            let new = JsValue::number(number - 1.0);
            frame.env.set(depth as usize, slot as usize, new.clone());
            frame
                .stack
                .push(if prefix { new } else { JsValue::number(number) });
        }
        Op::IncGlobal(name, prefix) => {
            let old = global_get(realm, &name).unwrap_or(JsValue::Undefined);
            let number = to_number(old)?;
            let new = JsValue::number(number + 1.0);
            Object::from_ref(realm.global.clone()).set_key(PropKey::Str(name), new.clone());
            frame
                .stack
                .push(if prefix { new } else { JsValue::number(number) });
        }
        Op::DecGlobal(name, prefix) => {
            let old = global_get(realm, &name).unwrap_or(JsValue::Undefined);
            let number = to_number(old)?;
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
            let number = to_number(old)?;
            let new = JsValue::number(number + delta);
            set_value_property(realm, &object, &key, new.clone(), true)?;
            frame
                .stack
                .push(if prefix { new } else { JsValue::number(number) });
        }
        Op::PushEnv(size) => {
            frame.env = Env::new(size as usize, Some(frame.env.clone()));
        }
        Op::PopEnv => {
            let parent = frame.env.parent.clone();
            match parent {
                Some(parent) => frame.env = parent,
                None => return Err(realm.type_error("Environment underflow")),
            }
        }
        Op::Closure(index) => {
            let value = closure_value(
                realm,
                frame.definition,
                index,
                &frame.env,
                frame.this.clone(),
            )?;
            frame.stack.push(value);
        }
        Op::TryStart(catch_pc) => {
            frame.handlers.push(Handler {
                catch_pc,
                stack_len: frame.stack.len(),
                env: frame.env.clone(),
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
                    if let Some(text) = key.as_str() {
                        keys.push(JsValue::string(text.to_owned()));
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
            let class = class_constant(frame.definition, index)?;
            let value = create_class(realm, &class, &frame.env, parent)?;
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
                run_instance_initializer(realm, &parent, &frame.this)?;
                frame.stack.push(frame.this.clone());
            }
        }
    }
    Ok(Step::Continue)
}

fn get_property_receiver(
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
        get_value_property(realm, value, key)
    }
}

fn super_function(realm: &mut Realm, frame: &Frame, key: &PropKey) -> JsResult<JsValue> {
    let callee = frame
        .callee
        .clone()
        .ok_or_else(|| realm.reference_error("super is not available"))?;
    let prototype = get_value_property(realm, &callee, &PropKey::from("__super_proto__"))?;
    if prototype.is_undefined() {
        return Err(realm.reference_error("super is not available"));
    }
    get_property_receiver(realm, &prototype, key, &frame.this)
}

fn binary_number(
    realm: &mut Realm,
    frame: &mut Frame,
    operation: impl Fn(f64, f64) -> f64,
) -> JsResult<()> {
    let right = pop(&mut frame.stack)?;
    let left = pop(&mut frame.stack)?;
    let value = operation(to_number(left)?, to_number(right)?);
    frame.stack.push(JsValue::number(value));
    let _ = realm;
    Ok(())
}

fn binary_int(
    realm: &mut Realm,
    frame: &mut Frame,
    operation: impl Fn(i32, i32) -> i32,
) -> JsResult<()> {
    let right = pop(&mut frame.stack)?;
    let left = pop(&mut frame.stack)?;
    let value = operation(to_int32(to_number(left)?), to_int32(to_number(right)?));
    frame.stack.push(JsValue::Int(value));
    let _ = realm;
    Ok(())
}

fn compare_op(
    realm: &mut Realm,
    frame: &mut Frame,
    predicate: impl Fn(i32) -> bool,
) -> JsResult<()> {
    let right = pop(&mut frame.stack)?;
    let left = pop(&mut frame.stack)?;
    let ordering = abstract_compare(realm, &left, &right)?;
    frame
        .stack
        .push(JsValue::Bool(ordering.is_some_and(predicate)));
    Ok(())
}

fn pop(stack: &mut Vec<JsValue>) -> JsResult<JsValue> {
    stack
        .pop()
        .ok_or_else(|| JsValue::string("InternalError: stack underflow"))
}

fn peek(stack: &[JsValue]) -> JsResult<&JsValue> {
    stack
        .last()
        .ok_or_else(|| JsValue::string("InternalError: stack underflow"))
}

fn pop_arguments(stack: &mut Vec<JsValue>, count: usize) -> JsResult<Vec<JsValue>> {
    if stack.len() < count {
        return Err(JsValue::string("InternalError: stack underflow"));
    }
    Ok(stack.split_off(stack.len() - count))
}

fn pop_array(stack: &mut Vec<JsValue>) -> JsResult<Vec<JsValue>> {
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
        Some(Constant::String(value)) => Ok(PropKey::Str(value.clone())),
        Some(Constant::Number(value)) => Ok(PropKey::Str(Rc::from(format_number(*value)))),
        _ => Err(JsValue::string("InternalError: invalid property constant")),
    }
}

fn constant_value(
    realm: &mut Realm,
    definition: &FunctionDef,
    index: u32,
    env: &EnvRef,
) -> JsResult<JsValue> {
    match definition.constants.get(index as usize) {
        Some(Constant::Number(value)) => Ok(JsValue::number(*value)),
        Some(Constant::String(value)) => Ok(JsValue::String(value.clone())),
        Some(Constant::Function(function)) => {
            Ok(realm.make_js_function(function.clone(), env.clone(), None))
        }
        Some(Constant::Class(class)) => {
            let value = create_class(realm, class, env, JsValue::Undefined)?;
            Ok(value)
        }
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
            let object = Rc::new(RefCell::new(object));
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

fn class_constant(definition: &FunctionDef, index: u32) -> JsResult<Rc<ClassDef>> {
    match definition.constants.get(index as usize) {
        Some(Constant::Class(class)) => Ok(class.clone()),
        _ => Err(JsValue::string("InternalError: invalid class constant")),
    }
}

fn create_class(
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
                property.get = Some(function);
            } else {
                property.set = Some(function);
            }
        } else {
            target.set_key(PropKey::Str(method.name.clone()), function);
        }
    }
    let _ = class.has_super;
    Ok(constructor)
}

/// QuickJS `JS_CallInternal` (quickjs.c:963) and the native call bridge in
/// `JS_CallFree`/`js_call_c_function`.
pub fn call_value(
    realm: &mut Realm,
    callee: JsValue,
    this: JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let function = callee
        .as_object()
        .and_then(|object| object.to_ref().borrow().function());
    let Some(function) = function else {
        return Err(realm.type_error("Value is not a function"));
    };
    if let Some(bound) = &function.bound {
        let mut combined = bound.arguments.clone();
        combined.extend_from_slice(arguments);
        return call_value(realm, bound.target.clone(), bound.this.clone(), &combined);
    }
    if let Some(native) = function.native {
        return native(realm, &this, arguments);
    }
    let definition = function
        .def
        .clone()
        .ok_or_else(|| realm.type_error("Invalid function"))?;
    let parent = function.env.clone().unwrap_or_else(|| Env::new(0, None));
    let env = Env::new(definition.env_size as usize, Some(parent));
    if definition.name.as_ref() == "<class_fields>" {
        copy_this_fields(&this, &env, definition.env_size as usize);
    }
    let this = if definition.is_arrow {
        function.lexical_this.clone().unwrap_or(JsValue::Undefined)
    } else if this.is_nullish() && !definition.strict {
        JsValue::Object(realm.global.clone())
    } else {
        this
    };
    let result = run_frame(
        realm,
        &definition,
        env,
        this,
        Some(callee),
        arguments.to_vec(),
    );
    if definition.is_async {
        let promise = realm.promise();
        if let ObjectKind::Promise(data) = &mut promise.borrow_mut().kind {
            match &result {
                Ok(value) => {
                    data.state = crate::object::PromiseState::Fulfilled;
                    data.value = value.clone();
                }
                Err(error) => {
                    data.state = crate::object::PromiseState::Rejected;
                    data.value = error.clone();
                }
            }
        }
        return Ok(JsValue::Object(promise));
    }
    result
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
    let prototype = get_value_property(realm, &callee, &PropKey::from("prototype"))?;
    let prototype = match prototype {
        JsValue::Object(object) => object,
        _ => realm.object_proto.clone(),
    };
    if function.native.is_some() {
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

fn run_instance_initializer(
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

fn copy_this_fields(this: &JsValue, env: &EnvRef, size: usize) {
    let Some(object) = this.as_object() else {
        return;
    };
    for slot in 0..size {
        let value = object.get(&slot.to_string());
        env.slots.borrow_mut()[slot] = value;
    }
}

/// QuickJS `JS_GetPropertyInternal` (quickjs.c:9110).
pub fn get_value_property(realm: &mut Realm, value: &JsValue, key: &PropKey) -> JsResult<JsValue> {
    match value {
        JsValue::Object(object) => {
            let resolved = object.borrow().resolve_property(key);
            match resolved {
                crate::object::ResolvedProperty::Data { value, .. } => Ok(value),
                crate::object::ResolvedProperty::Accessor {
                    get: Some(getter), ..
                } => call_value(realm, getter, value.clone(), &[]),
                crate::object::ResolvedProperty::Accessor { get: None, .. } => {
                    Ok(JsValue::Undefined)
                }
                crate::object::ResolvedProperty::Missing => Ok(JsValue::Undefined),
            }
        }
        JsValue::String(text_value) => {
            let text = text_value.as_ref();
            if let Some(key) = key.as_str() {
                if key == "length" {
                    return Ok(JsValue::number(crate::object::utf16_len(text) as f64));
                }
                if let Some(index) = key_to_index(key)
                    && let Some(ch) = crate::object::utf16_char(text, index)
                {
                    return Ok(JsValue::string(ch));
                }
            }
            let prototype = ObjectData::ordinary(Some(realm.string_proto.clone()));
            prototype.get_property(realm, key, value)
        }
        JsValue::Bool(_) => {
            let prototype = ObjectData::ordinary(Some(realm.boolean_proto.clone()));
            prototype.get_property(realm, key, value)
        }
        JsValue::Int(_) | JsValue::Float(_) => {
            let prototype = ObjectData::ordinary(Some(realm.number_proto.clone()));
            prototype.get_property(realm, key, value)
        }
        JsValue::Symbol(_) => {
            let prototype = ObjectData::ordinary(Some(realm.symbol_proto.clone()));
            prototype.get_property(realm, key, value)
        }
        JsValue::Undefined | JsValue::Null => Err(realm.type_error(format!(
            "Cannot read properties of {} (reading '{}')",
            if value.is_null() { "null" } else { "undefined" },
            display_key(key)
        ))),
    }
}

/// QuickJS `JS_SetPropertyInternal` (quickjs.c:10818).
pub fn set_value_property(
    realm: &mut Realm,
    value: &JsValue,
    key: &PropKey,
    new_value: JsValue,
    _strict: bool,
) -> JsResult<()> {
    match value {
        JsValue::Object(object) => {
            let resolved = object.borrow().resolve_property(key);
            match resolved {
                crate::object::ResolvedProperty::Accessor {
                    set: Some(setter), ..
                } => {
                    call_value(realm, setter, value.clone(), &[new_value])?;
                    Ok(())
                }
                crate::object::ResolvedProperty::Accessor { set: None, .. } => Ok(()),
                crate::object::ResolvedProperty::Data {
                    writable,
                    own: true,
                    ..
                } => {
                    if writable {
                        let mut data = object.borrow_mut();
                        if data.put_array_value(realm, key, &new_value)?.is_some() {
                            return Ok(());
                        }
                        if let Some(property) = data.props.get_mut(key) {
                            property.value = new_value;
                        }
                    }
                    Ok(())
                }
                crate::object::ResolvedProperty::Data {
                    writable,
                    own: false,
                    ..
                } => {
                    if writable {
                        object
                            .borrow_mut()
                            .define(key.clone(), Property::data(new_value));
                    }
                    Ok(())
                }
                crate::object::ResolvedProperty::Missing => {
                    if object.borrow().extensible {
                        object
                            .borrow_mut()
                            .define(key.clone(), Property::data(new_value));
                    }
                    Ok(())
                }
            }
        }
        JsValue::Undefined | JsValue::Null => Err(realm.type_error(format!(
            "Cannot set properties of {} (setting '{}')",
            if value.is_null() { "null" } else { "undefined" },
            display_key(key)
        ))),
        JsValue::Symbol(_) => Ok(()),
        _ => Ok(()),
    }
}

fn display_key(key: &PropKey) -> String {
    match key {
        PropKey::Str(value) => value.to_string(),
        PropKey::Symbol(value) => format!("Symbol({value})"),
    }
}

fn global_get(realm: &Realm, name: &str) -> Option<JsValue> {
    let global = Object::from_ref(realm.global.clone());
    let key = PropKey::from(name);
    if global.to_ref().borrow().has_property(&key) {
        Some(global.get_key(&key))
    } else {
        None
    }
}

pub fn to_boolean(value: &JsValue) -> bool {
    match value {
        JsValue::Undefined | JsValue::Null => false,
        JsValue::Bool(value) => *value,
        JsValue::Int(value) => *value != 0,
        JsValue::Float(value) => *value != 0.0 && !value.is_nan(),
        JsValue::String(value) => !value.is_empty(),
        JsValue::Symbol(_) => true,
        JsValue::Object(_) => true,
    }
}

pub fn to_number(value: JsValue) -> JsResult<f64> {
    Ok(match value {
        JsValue::Undefined => f64::NAN,
        JsValue::Null => 0.0,
        JsValue::Bool(value) => {
            if value {
                1.0
            } else {
                0.0
            }
        }
        JsValue::Int(value) => value as f64,
        JsValue::Float(value) => value,
        JsValue::String(value) => string_to_number(&value),
        JsValue::Symbol(_) => {
            return Err(JsValue::string(
                "TypeError: cannot convert a Symbol value to a number",
            ));
        }
        JsValue::Object(_) => {
            return Err(JsValue::string(
                "TypeError: cannot convert object to number",
            ));
        }
    })
}

fn string_to_number(value: &str) -> f64 {
    let value = value.trim();
    if value.is_empty() {
        return 0.0;
    }
    if value == "Infinity" || value == "+Infinity" {
        return f64::INFINITY;
    }
    if value == "-Infinity" {
        return f64::NEG_INFINITY;
    }
    if let Some(rest) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return u64::from_str_radix(rest, 16)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    if let Some(rest) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        return u64::from_str_radix(rest, 2)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    if let Some(rest) = value
        .strip_prefix("0o")
        .or_else(|| value.strip_prefix("0O"))
    {
        return u64::from_str_radix(rest, 8)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    value.parse().unwrap_or(f64::NAN)
}

pub fn to_string(realm: &mut Realm, value: JsValue) -> JsResult<String> {
    match value {
        JsValue::Undefined => Ok("undefined".into()),
        JsValue::Null => Ok("null".into()),
        JsValue::Bool(value) => Ok(if value { "true" } else { "false" }.into()),
        JsValue::Int(value) => Ok(value.to_string()),
        JsValue::Float(value) => Ok(format_number(value)),
        JsValue::String(value) => Ok(value.to_string()),
        JsValue::Symbol(id) => {
            let description = realm.symbol_description(id);
            Ok(if description.is_empty() {
                "Symbol()".into()
            } else {
                format!("Symbol({description})")
            })
        }
        JsValue::Object(object) => {
            let data = object.borrow();
            match &data.kind {
                ObjectKind::Array(array) => {
                    let items = array.items.clone();
                    let length = array.length;
                    drop(data);
                    let mut parts = Vec::with_capacity(length);
                    for index in 0..length {
                        let item = items.get(index).cloned().unwrap_or(JsValue::Undefined);
                        if item.is_nullish() {
                            parts.push(String::new());
                        } else {
                            parts.push(to_string(realm, item)?);
                        }
                    }
                    Ok(parts.join(","))
                }
                ObjectKind::BoxedString(value) => Ok(value.to_string()),
                ObjectKind::BoxedNumber(value) => Ok(format_number(*value)),
                ObjectKind::BoxedBool(value) => Ok(value.to_string()),
                ObjectKind::TypedArray(array) => {
                    let values = array.values();
                    drop(data);
                    let mut parts = Vec::with_capacity(values.len());
                    for value in values {
                        parts.push(to_string(realm, value)?);
                    }
                    Ok(parts.join(","))
                }
                ObjectKind::ArrayBuffer(_) => Ok("[object ArrayBuffer]".into()),
                ObjectKind::DataView(_) => Ok("[object DataView]".into()),
                ObjectKind::Function(function) => {
                    Ok(format!("function {}() {{ [native code] }}", function.name))
                }
                ObjectKind::Error => {
                    let name = data
                        .get_own_value(&PropKey::from("name"))
                        .and_then(|value| value.as_string())
                        .unwrap_or_else(|| Rc::from("Error"));
                    let message = data
                        .get_own_value(&PropKey::from("message"))
                        .map(|value| value.as_string().unwrap_or_else(|| Rc::from("")))
                        .unwrap_or_else(|| Rc::from(""));
                    if message.is_empty() {
                        Ok(name.to_string())
                    } else {
                        Ok(format!("{name}: {message}"))
                    }
                }
                ObjectKind::RegExp(regexp) => Ok(format!("/{}/{}", regexp.source, regexp.flags)),
                ObjectKind::Date(value) => Ok(format_date(*value)),
                _ => Ok("[object Object]".into()),
            }
        }
    }
}

fn format_date(value: f64) -> String {
    if !value.is_finite() {
        return "Invalid Date".into();
    }
    let seconds = (value / 1000.0).floor() as i64;
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        year,
        month + 1,
        day,
        time / 3600,
        (time / 60) % 60,
        time % 60
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + if m <= 2 { 1 } else { 0 }, m - 1, d)
}

pub fn format_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == f64::INFINITY {
        return "Infinity".into();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    if value.fract() == 0.0 && value.abs() < 1e21 {
        return format!("{}", value as i64);
    }
    let mut buffer = ryu::Buffer::new();
    buffer.format(value).to_owned()
}

pub fn pop_property_key(realm: &mut Realm, stack: &mut Vec<JsValue>) -> JsResult<PropKey> {
    let value = pop(stack)?;
    to_property_key(realm, value)
}

pub fn to_property_key(realm: &mut Realm, value: JsValue) -> JsResult<PropKey> {
    match value {
        JsValue::String(value) => Ok(PropKey::Str(value)),
        JsValue::Int(value) => Ok(PropKey::Str(Rc::from(value.to_string()))),
        JsValue::Float(value) => Ok(PropKey::Str(Rc::from(format_number(value)))),
        JsValue::Undefined => Ok(PropKey::from("undefined")),
        JsValue::Null => Ok(PropKey::from("null")),
        JsValue::Bool(value) => Ok(PropKey::from(if value { "true" } else { "false" })),
        JsValue::Symbol(id) => Ok(PropKey::Symbol(id)),
        JsValue::Object(_) => Ok(PropKey::Str(Rc::from(to_string(realm, value)?))),
    }
}

pub fn add_values(realm: &mut Realm, left: JsValue, right: JsValue) -> JsResult<JsValue> {
    let left = to_primitive(realm, left)?;
    let right = to_primitive(realm, right)?;
    if matches!(left, JsValue::String(_)) || matches!(right, JsValue::String(_)) {
        let left = to_string(realm, left)?;
        let right = to_string(realm, right)?;
        return Ok(JsValue::string(left + &right));
    }
    Ok(JsValue::number(to_number(left)? + to_number(right)?))
}

pub fn to_primitive(realm: &mut Realm, value: JsValue) -> JsResult<JsValue> {
    match value {
        JsValue::Object(object) => {
            {
                let data = object.borrow();
                match &data.kind {
                    ObjectKind::BoxedString(value) => return Ok(JsValue::String(value.clone())),
                    ObjectKind::BoxedNumber(value) => return Ok(JsValue::number(*value)),
                    ObjectKind::BoxedBool(value) => return Ok(JsValue::Bool(*value)),
                    ObjectKind::Array(_) => {}
                    ObjectKind::Error => {}
                    _ => {}
                }
            }
            let value_object = JsValue::Object(object);
            let value_of = get_value_property(realm, &value_object, &PropKey::from("valueOf"))?;
            if value_of.is_function() {
                let result = call_value(realm, value_of, value_object.clone(), &[])?;
                if !result.is_object() {
                    return Ok(result);
                }
            }
            let to_string_fn =
                get_value_property(realm, &value_object, &PropKey::from("toString"))?;
            if to_string_fn.is_function() {
                let result = call_value(realm, to_string_fn, value_object.clone(), &[])?;
                if !result.is_object() {
                    return Ok(result);
                }
            }
            Err(realm.type_error("Cannot convert object to primitive value"))
        }
        value => Ok(value),
    }
}

pub fn same_value(left: &JsValue, right: &JsValue) -> bool {
    if strict_equals(left, right) {
        if matches!(left, JsValue::Int(_) | JsValue::Float(_)) {
            let a = left.as_number().unwrap();
            let b = right.as_number().unwrap();
            if a == 0.0 && b == 0.0 {
                return a.is_sign_negative() == b.is_sign_negative();
            }
        }
        return true;
    }
    matches!(
        (left, right),
        (JsValue::Int(_) | JsValue::Float(_), JsValue::Int(_) | JsValue::Float(_))
            if left.as_number().is_some_and(f64::is_nan)
                && right.as_number().is_some_and(f64::is_nan)
    )
}

pub fn same_value_zero(left: &JsValue, right: &JsValue) -> bool {
    if strict_equals(left, right) {
        return true;
    }
    matches!(
        (left, right),
        (JsValue::Int(_) | JsValue::Float(_), JsValue::Int(_) | JsValue::Float(_))
            if left.as_number().is_some_and(f64::is_nan)
                && right.as_number().is_some_and(f64::is_nan)
    )
}

pub fn strict_equals(left: &JsValue, right: &JsValue) -> bool {
    match (left, right) {
        (JsValue::Undefined, JsValue::Undefined) | (JsValue::Null, JsValue::Null) => true,
        (JsValue::Bool(a), JsValue::Bool(b)) => a == b,
        (JsValue::Int(a), JsValue::Int(b)) => a == b,
        (JsValue::Int(a), JsValue::Float(b)) | (JsValue::Float(b), JsValue::Int(a)) => {
            (*a as f64) == *b
        }
        (JsValue::Float(a), JsValue::Float(b)) => a == b,
        (JsValue::String(a), JsValue::String(b)) => a == b,
        (JsValue::Symbol(a), JsValue::Symbol(b)) => a == b,
        (JsValue::Object(a), JsValue::Object(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

pub fn loose_equals(realm: &mut Realm, left: &JsValue, right: &JsValue) -> JsResult<bool> {
    if strict_equals(left, right) {
        return Ok(true);
    }
    match (left, right) {
        (JsValue::Null | JsValue::Undefined, JsValue::Null | JsValue::Undefined) => Ok(true),
        (JsValue::Int(_) | JsValue::Float(_), JsValue::String(_)) => {
            Ok(to_number(left.clone())? == to_number(right.clone())?)
        }
        (JsValue::String(_), JsValue::Int(_) | JsValue::Float(_)) => {
            Ok(to_number(left.clone())? == to_number(right.clone())?)
        }
        (JsValue::Bool(_), _) => {
            loose_equals(realm, &JsValue::number(to_number(left.clone())?), right)
        }
        (_, JsValue::Bool(_)) => {
            loose_equals(realm, left, &JsValue::number(to_number(right.clone())?))
        }
        (JsValue::Object(_), JsValue::Int(_) | JsValue::Float(_) | JsValue::String(_)) => {
            let left = to_primitive(realm, left.clone())?;
            loose_equals(realm, &left, right)
        }
        (JsValue::Int(_) | JsValue::Float(_) | JsValue::String(_), JsValue::Object(_)) => {
            let right = to_primitive(realm, right.clone())?;
            loose_equals(realm, left, &right)
        }
        _ => Ok(false),
    }
}

fn abstract_compare(realm: &mut Realm, left: &JsValue, right: &JsValue) -> JsResult<Option<i32>> {
    let left = to_primitive(realm, left.clone())?;
    let right = to_primitive(realm, right.clone())?;
    if let (JsValue::String(left), JsValue::String(right)) = (&left, &right) {
        return Ok(Some(match left.cmp(right) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }));
    }
    let left = to_number(left)?;
    let right = to_number(right)?;
    if left.is_nan() || right.is_nan() {
        Ok(None)
    } else if left < right {
        Ok(Some(-1))
    } else if left > right {
        Ok(Some(1))
    } else {
        Ok(Some(0))
    }
}

fn instance_of(realm: &mut Realm, value: &JsValue, constructor: &JsValue) -> JsResult<bool> {
    let Some(constructor_object) = constructor.as_object() else {
        return Err(realm.type_error("Right-hand side of instanceof is not callable"));
    };
    let prototype = get_value_property(realm, constructor, &PropKey::from("prototype"))?;
    let JsValue::Object(prototype) = prototype else {
        return Err(realm.type_error("Function has non-object prototype in instanceof"));
    };
    let mut current = match value {
        JsValue::Object(value) => value.borrow().prototype.clone(),
        _ => return Ok(false),
    };
    while let Some(object) = current {
        if Rc::ptr_eq(&object, &prototype) {
            return Ok(true);
        }
        current = object.borrow().prototype.clone();
    }
    let _ = constructor_object;
    Ok(false)
}

pub fn to_int32(value: f64) -> i32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    let value = value.trunc();
    let modulo = value.rem_euclid(4_294_967_296.0);
    if modulo >= 2_147_483_648.0 {
        (modulo - 4_294_967_296.0) as i32
    } else {
        modulo as i32
    }
}

pub fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    value.trunc().rem_euclid(4_294_967_296.0) as u32
}

pub fn iterable_items(realm: &mut Realm, value: JsValue) -> JsResult<Vec<JsValue>> {
    match &value {
        JsValue::String(text) => Ok(text.chars().map(JsValue::string).collect()),
        JsValue::Object(object) => {
            if let Some(items) = object.borrow().array_items().map(|items| items.to_vec()) {
                return Ok(items);
            }
            let data = object.borrow();
            if let ObjectKind::Iterator { items, .. } = &data.kind {
                return Ok(items.clone());
            }
            if let ObjectKind::Map(entries) = &data.kind {
                let entries = entries.clone();
                drop(data);
                return Ok(entries
                    .into_iter()
                    .map(|(key, value)| JsValue::Object(realm.array(vec![key, value])))
                    .collect());
            }
            if let ObjectKind::Set(values) = &data.kind {
                return Ok(values.clone());
            }
            if let ObjectKind::TypedArray(values) = &data.kind {
                return Ok(values.values());
            }
            drop(data);
            let iterator_key = PropKey::Symbol(realm.well_known_symbol("iterator"));
            if object.borrow().has_property(&iterator_key) {
                let method = get_value_property(realm, &value, &iterator_key)?;
                if method.is_function() {
                    let iterator = call_value(realm, method, value.clone(), &[])?;
                    let next = get_value_property(realm, &iterator, &PropKey::from("next"))?;
                    if !next.is_function() {
                        return Err(realm.type_error("Iterator has no next method"));
                    }
                    let mut items = Vec::new();
                    loop {
                        let result = call_value(realm, next.clone(), iterator.clone(), &[])?;
                        let done = get_value_property(realm, &result, &PropKey::from("done"))?;
                        if to_boolean(&done) {
                            break;
                        }
                        items.push(get_value_property(realm, &result, &PropKey::from("value"))?);
                    }
                    return Ok(items);
                }
            }
            let length = get_value_property(realm, &value, &PropKey::from("length"))?;
            if length.is_undefined() {
                return Err(realm.type_error("Value is not iterable"));
            }
            let length = to_uint32(to_number(length)?) as usize;
            let mut items = Vec::with_capacity(length);
            for index in 0..length {
                items.push(get_value_property(
                    realm,
                    &value,
                    &PropKey::Str(Rc::from(index.to_string())),
                )?);
            }
            Ok(items)
        }
        _ => Err(realm.type_error("Value is not iterable")),
    }
}

fn push_array_value(realm: &mut Realm, array: &ObjRef, value: JsValue) -> JsResult<()> {
    let mut data = array.borrow_mut();
    if let ObjectKind::Array(items) = &mut data.kind {
        items.items.push(value);
        items.length = items.items.len();
        Ok(())
    } else {
        drop(data);
        Err(realm.type_error("Array expected"))
    }
}

fn copy_own_properties(realm: &mut Realm, target: &ObjRef, source: &ObjRef) -> JsResult<()> {
    let keys = source.borrow().enumerable_keys();
    for key in keys {
        let value = source
            .borrow()
            .get_property(realm, &key, &JsValue::Object(source.clone()))?;
        target.borrow_mut().define(key, Property::data(value));
    }
    Ok(())
}

pub fn type_error(realm: &Realm, message: impl Into<String>) -> JsValue {
    realm.type_error(message)
}

pub fn range_error(realm: &Realm, message: impl Into<String>) -> JsValue {
    realm.range_error(message)
}

pub fn display_error(_realm: &Realm, value: JsValue) -> String {
    display_error_value(&value)
}

fn display_error_value(value: &JsValue) -> String {
    match value {
        JsValue::String(text) => text.to_string(),
        JsValue::Object(object) => {
            let data = object.borrow();
            if data.is_error() {
                let name = data
                    .get_own_value(&PropKey::from("name"))
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| Rc::from("Error"));
                let message = data
                    .get_own_value(&PropKey::from("message"))
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| Rc::from(""));
                if message.is_empty() {
                    name.to_string()
                } else {
                    format!("{name}: {message}")
                }
            } else {
                "[object Object]".into()
            }
        }
        other => format!("{other:?}"),
    }
}
