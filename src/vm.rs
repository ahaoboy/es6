use std::cell::{Cell, RefCell};
use std::rc::Rc;

use smallvec::SmallVec;

use crate::bytecode::{ClassDef, Constant, FunctionDef, Op};
use crate::object::{
    ObjRef, Object, ObjectData, ObjectKind, PromiseReaction, PromiseState, PropKey, Property,
    RegExpData, key_to_index,
};
use crate::runtime::Realm;
use crate::value::{Env, EnvRef, JsResult, JsValue};

use crate::coerce::{abstract_compare, instance_of};
pub use crate::coerce::{
    add_values, format_number, loose_equals, same_value, same_value_zero, strict_equals,
    to_boolean, to_int32, to_number, to_numeric, to_primitive, to_property_key, to_string,
    to_uint32,
};

enum Step {
    Continue,
    Return(JsValue),
    Await(JsValue),
}

enum RunOutcome {
    Return(JsValue),
    Suspended,
}

struct Handler {
    catch_pc: usize,
    stack_len: usize,
    env: EnvRef,
    index: usize,
}

/// A queued job. QuickJS stores the same entries in `JSContext.job_list` and
/// dispatches them in `JS_ExecutePendingJob` (quickjs.c:23952).
pub(crate) enum Job {
    Reaction {
        reaction: PromiseReaction,
        state: PromiseState,
        value: JsValue,
    },
    Resume {
        frame: Box<Frame>,
        completion: ObjRef,
        result: JsResult<JsValue>,
    },
}

pub(crate) type SuspendedFrame = Rc<RefCell<Option<Box<Frame>>>>;
type ValueStack = SmallVec<[JsValue; 4]>;
type ArgumentStack = SmallVec<[JsValue; 4]>;

/// Activation record. QuickJS' equivalent is `JSStackFrame` (quickjs.c:900
/// area) with `var_refs`, `sp` and the operand stack in `JS_CallInternal`
/// (quickjs.c:963). It is owned so `await` can move it into the job queue.
pub(crate) struct Frame {
    definition: Rc<FunctionDef>,
    env: EnvRef,
    this: JsValue,
    callee: Option<JsValue>,
    stack: ValueStack,
    arguments: ArgumentStack,
    handlers: SmallVec<[Handler; 4]>,
    pc: usize,
    completion: Option<ObjRef>,
}

thread_local! {
    /// Byte offset of the innermost uncaught VM exception. The VM keeps this
    /// out of `JsValue` so thrown values remain ordinary JavaScript values.
    static ERROR_OFFSET: Cell<Option<usize>> = const { Cell::new(None) };
    static CALL_STACK: RefCell<Vec<Rc<str>>> = const { RefCell::new(Vec::new()) };
}

fn stack_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("ES6_TRACE_STACK").is_some())
}

fn record_error_stack(exception: &JsValue) {
    if !stack_trace_enabled() {
        return;
    }
    let JsValue::Object(object) = exception else {
        return;
    };
    if object.borrow().has_own(&PropKey::from("stack")) {
        return;
    }
    let stack = CALL_STACK.with(|frames| {
        let frames = frames.borrow();
        let mut text = String::from("Error stack:\n");
        for name in frames.iter().rev() {
            text.push_str("  at ");
            text.push_str(name);
            text.push('\n');
        }
        text
    });
    object
        .borrow_mut()
        .define("stack", Property::hidden(JsValue::string(stack)));
}

pub fn take_error_offset() -> Option<usize> {
    ERROR_OFFSET.with(Cell::take)
}

pub fn clear_error_offset() {
    ERROR_OFFSET.with(|offset| offset.set(None));
}

pub fn execute(
    realm: &mut Realm,
    definition: Rc<FunctionDef>,
    env: EnvRef,
    this: JsValue,
) -> JsResult<JsValue> {
    ERROR_OFFSET.with(|offset| offset.set(None));
    match run_frame(
        realm,
        definition,
        env,
        this,
        None,
        ArgumentStack::new(),
        None,
    )? {
        RunOutcome::Return(value) => Ok(value),
        RunOutcome::Suspended => Ok(JsValue::Undefined),
    }
}

fn run_frame(
    realm: &mut Realm,
    definition: Rc<FunctionDef>,
    env: EnvRef,
    this: JsValue,
    callee: Option<JsValue>,
    arguments: ArgumentStack,
    completion: Option<ObjRef>,
) -> JsResult<RunOutcome> {
    let frame = Frame {
        definition,
        env,
        this,
        callee,
        stack: ValueStack::new(),
        arguments,
        handlers: SmallVec::new(),
        pc: 0,
        completion,
    };
    run_loop(realm, frame)
}

fn run_loop(realm: &mut Realm, mut frame: Frame) -> JsResult<RunOutcome> {
    loop {
        if frame.pc >= frame.definition.code.len() {
            return Ok(RunOutcome::Return(JsValue::Undefined));
        }
        let op = frame.definition.code[frame.pc];
        frame.pc += 1;
        match execute_op(realm, &mut frame, op) {
            Ok(Step::Continue) => {}
            Ok(Step::Return(value)) => return Ok(RunOutcome::Return(value)),
            Ok(Step::Await(value)) => {
                let Some(completion) = frame.completion.clone() else {
                    let value = await_sync(realm, value)?;
                    frame.stack.push(value);
                    continue;
                };
                let suspended: SuspendedFrame = Rc::new(RefCell::new(Some(Box::new(frame))));
                crate::promise::await_value(realm, value, suspended, completion)?;
                return Ok(RunOutcome::Suspended);
            }
            Err(exception) => {
                record_error_stack(&exception);
                if !unwind(&mut frame, exception.clone()) {
                    if let Some(offset) = frame.definition.positions.get(frame.pc - 1) {
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

fn unwind(frame: &mut Frame, exception: JsValue) -> bool {
    let Some(handler) = frame.handlers.pop() else {
        return false;
    };
    ERROR_OFFSET.with(|offset| offset.set(None));
    frame.handlers.truncate(handler.index);
    frame.stack.truncate(handler.stack_len);
    frame.env = handler.env;
    frame.stack.push(exception);
    frame.pc = handler.catch_pc;
    true
}

/// `await` in a non-async context keeps the old synchronous behaviour.
fn await_sync(_realm: &mut Realm, value: JsValue) -> JsResult<JsValue> {
    let Some(promise) = crate::promise::promise_object(&value) else {
        return Ok(value);
    };
    let Some((state, value)) = crate::promise::state(&promise) else {
        return Ok(JsValue::Undefined);
    };
    match state {
        PromiseState::Fulfilled => Ok(value),
        PromiseState::Rejected => Err(value),
        PromiseState::Pending => Ok(JsValue::Undefined),
    }
}

pub(crate) fn run_job(realm: &mut Realm, job: Job) -> JsResult<()> {
    match job {
        Job::Reaction {
            reaction,
            state,
            value,
        } => {
            crate::promise::reaction(realm, reaction, state, value);
            Ok(())
        }
        Job::Resume {
            mut frame,
            completion,
            result,
        } => {
            frame.completion = Some(completion.clone());
            match result {
                Ok(value) => frame.stack.push(value),
                Err(exception) => {
                    if !unwind(&mut frame, exception.clone()) {
                        let _ = crate::promise::reject(realm, &completion, exception);
                        return Ok(());
                    }
                }
            }
            match run_loop(realm, *frame) {
                Ok(RunOutcome::Return(value)) => {
                    crate::promise::resolve(realm, &completion, value)?;
                }
                Ok(RunOutcome::Suspended) => {}
                Err(error) => {
                    clear_error_offset();
                    let _ = crate::promise::reject(realm, &completion, error);
                }
            }
            Ok(())
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
            let value = constant_value(realm, &frame.definition, index, &frame.env)?;
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
            let value = frame.env.get(depth as usize, slot as usize);
            frame.stack.push(value);
        }
        Op::SetVar { depth, slot } => {
            let value = pop(&mut frame.stack)?;
            frame.env.set(depth as usize, slot as usize, value);
        }
        Op::GetGlobal(index) => {
            let name = constant_string(&frame.definition, index);
            match global_get(realm, &name) {
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
            let key = property_key_from_constant(&frame.definition, index)?;
            let object = pop(&mut frame.stack)?;
            let value = get_value_property(realm, &object, &key)?;
            frame.stack.push(value);
        }
        Op::SetPropConst(index) => {
            let value = pop(&mut frame.stack)?;
            let key = property_key_from_constant(&frame.definition, index)?;
            let object = pop(&mut frame.stack)?;
            set_value_property(realm, &object, &key, value.clone(), true)?;
            frame.stack.push(value);
        }
        Op::SetPropConstKeep(index) => {
            let value = pop(&mut frame.stack)?;
            let key = property_key_from_constant(&frame.definition, index)?;
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
                    property.get = Some(Box::new(function));
                } else {
                    property.set = Some(Box::new(function));
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
            let value =
                to_uint32(to_numeric(realm, left)?) >> (to_uint32(to_numeric(realm, right)?) & 31);
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
            frame
                .stack
                .push(JsValue::number(-to_numeric(realm, value)?));
        }
        Op::Pos => {
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::number(to_numeric(realm, value)?));
        }
        Op::BitNot => {
            let value = pop(&mut frame.stack)?;
            frame
                .stack
                .push(JsValue::Int(!to_int32(to_numeric(realm, value)?)));
        }
        Op::TypeOf => {
            let value = pop(&mut frame.stack)?;
            frame.stack.push(JsValue::string(value.type_of()));
        }
        Op::TypeOfGlobal(index) => {
            let name = constant_string(&frame.definition, index);
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
            return Ok(Step::Await(value));
        }
        Op::IncVar {
            depth,
            slot,
            prefix,
        } => {
            let old = frame.env.get(depth as usize, slot as usize);
            let number = to_numeric(realm, old)?;
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
            let number = to_numeric(realm, old)?;
            let new = JsValue::number(number - 1.0);
            frame.env.set(depth as usize, slot as usize, new.clone());
            frame
                .stack
                .push(if prefix { new } else { JsValue::number(number) });
        }
        Op::IncGlobal(index, prefix) => {
            let name = constant_string(&frame.definition, index);
            let old = global_get(realm, &name).unwrap_or(JsValue::Undefined);
            let number = to_numeric(realm, old)?;
            let new = JsValue::number(number + 1.0);
            Object::from_ref(realm.global.clone()).set_key(PropKey::Str(name), new.clone());
            frame
                .stack
                .push(if prefix { new } else { JsValue::number(number) });
        }
        Op::DecGlobal(index, prefix) => {
            let name = constant_string(&frame.definition, index);
            let old = global_get(realm, &name).unwrap_or(JsValue::Undefined);
            let number = to_numeric(realm, old)?;
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
            let number = to_numeric(realm, old)?;
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
                &frame.definition,
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
    let value = operation(to_numeric(realm, left)?, to_numeric(realm, right)?);
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
        to_int32(to_numeric(realm, left)?),
        to_int32(to_numeric(realm, right)?),
    );
    frame.stack.push(JsValue::Int(value));
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

fn pop(stack: &mut ValueStack) -> JsResult<JsValue> {
    stack
        .pop()
        .ok_or_else(|| JsValue::string("InternalError: stack underflow"))
}

fn peek(stack: &[JsValue]) -> JsResult<&JsValue> {
    stack
        .last()
        .ok_or_else(|| JsValue::string("InternalError: stack underflow"))
}

fn pop_arguments(stack: &mut ValueStack, count: usize) -> JsResult<ArgumentStack> {
    if stack.len() < count {
        return Err(JsValue::string("InternalError: stack underflow"));
    }
    let start = stack.len() - count;
    let values = stack[start..].iter().cloned().collect();
    stack.truncate(start);
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

fn constant_string(definition: &FunctionDef, index: u32) -> Rc<str> {
    match definition.constants.get(index as usize) {
        Some(Constant::String(value)) => value.clone(),
        _ => Rc::from(""),
    }
}

fn constant_strings(definition: &FunctionDef, index: u32) -> Rc<[Rc<str>]> {
    match definition.constants.get(index as usize) {
        Some(Constant::Strings(value)) => value.clone(),
        _ => Rc::from([]),
    }
}

fn property_key_from_constant(definition: &FunctionDef, index: u32) -> JsResult<PropKey> {
    match definition.constants.get(index as usize) {
        Some(Constant::String(value)) => Ok(crate::object::key_from_string(value.clone())),
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
                property.get = Some(Box::new(function));
            } else {
                property.set = Some(Box::new(function));
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
    call_value_owned(realm, callee, this, arguments.iter().cloned().collect())
}

fn call_value_owned(
    realm: &mut Realm,
    callee: JsValue,
    this: JsValue,
    arguments: ArgumentStack,
) -> JsResult<JsValue> {
    let function = callee
        .as_object()
        .and_then(|object| object.to_ref().borrow().function());
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
    let definition = function
        .def
        .clone()
        .ok_or_else(|| realm.type_error("Invalid function"))?;
    let parent = function.env.clone().unwrap_or_else(|| Env::new(0, None));
    let env = Env::new(definition.env_size as usize, Some(parent));
    if definition.name.as_ref() == "<class_fields>" {
        copy_this_fields(&this, &env, definition.env_size as usize);
    }
    let trace = stack_trace_enabled();
    if trace {
        CALL_STACK.with(|stack| stack.borrow_mut().push(definition.name.clone()));
    }
    let this = if definition.is_arrow {
        function.lexical_this.clone().unwrap_or(JsValue::Undefined)
    } else if this.is_nullish() && !definition.strict {
        JsValue::Object(realm.global.clone())
    } else {
        this
    };
    let completion = definition.is_async.then(|| realm.promise());
    let result = run_frame(
        realm,
        definition.clone(),
        env,
        this,
        Some(callee),
        arguments,
        completion.clone(),
    );
    if trace {
        CALL_STACK.with(|stack| {
            stack.borrow_mut().pop();
        });
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
            let canonical = key.canonical();
            let prototype = object.borrow().prototype.clone();
            let mut data = object.borrow_mut();
            if data.put_array_value(realm, key, &new_value)?.is_some() {
                return Ok(());
            }
            if let Some(property) = data.props.get_mut(&canonical) {
                if property.is_accessor() {
                    let setter = property.set.as_deref().cloned();
                    drop(data);
                    if let Some(setter) = setter {
                        call_value(realm, setter, value.clone(), &[new_value])?;
                    }
                    return Ok(());
                }
                if property.writable {
                    property.value = new_value;
                }
                return Ok(());
            }
            let extensible = data.extensible;
            drop(data);

            if let Some(prototype) = prototype
                && prototype.borrow().has_property(&canonical)
            {
                let resolved = prototype.borrow().resolve_property(&canonical);
                match resolved {
                    crate::object::ResolvedProperty::Accessor {
                        set: Some(setter), ..
                    } => {
                        call_value(realm, setter, value.clone(), &[new_value])?;
                    }
                    crate::object::ResolvedProperty::Accessor { set: None, .. } => {}
                    crate::object::ResolvedProperty::Data { writable, .. } => {
                        if writable && extensible {
                            object
                                .borrow_mut()
                                .define(canonical, Property::data(new_value));
                        }
                    }
                    crate::object::ResolvedProperty::Missing => {}
                }
                return Ok(());
            }
            if extensible {
                object
                    .borrow_mut()
                    .define(canonical, Property::data(new_value));
            }
            Ok(())
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
        PropKey::Index(value) => value.to_string(),
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

pub(crate) fn pop_property_key(realm: &mut Realm, stack: &mut ValueStack) -> JsResult<PropKey> {
    let value = pop(stack)?;
    to_property_key(realm, value)
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
