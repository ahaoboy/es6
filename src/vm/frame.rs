//! Activation record.
//!
//! QuickJS' equivalent is `JSStackFrame` (quickjs.c:427) with `var_refs`,
//! `cur_sp` and the operand stack of `JS_CallInternal` (quickjs.c:18051).
//! The frame is heap owned so `await` and generator `yield` can move it into
//! the job queue and back.

use std::rc::Rc;

use smallvec::SmallVec;

use crate::bytecode::FunctionDef;
use crate::object::ObjRef;
use crate::value::{EnvRef, JsValue};

use super::{ArgumentStack, ValueStack};

/// Dispatch result for one opcode.
pub(crate) enum Step {
    Continue,
    Return(JsValue),
    Await(JsValue),
    /// `yield`: suspend the frame, resuming at `resume_pc` (quickjs
    /// `OP_yield`, quickjs.c:20871).
    Yield {
        value: JsValue,
        resume_pc: usize,
    },
}

pub(crate) enum RunOutcome {
    Return(JsValue),
    Suspended,
    /// The frame suspended at a `yield` and moved out with the value.
    Yield {
        frame: Box<Frame>,
        value: JsValue,
    },
}

/// Installed exception handler. `catch_pc` mirrors the recovery label of
/// `OP_try_start` in QuickJS (`quickjs.c:18051` exception path).
pub(crate) struct Handler {
    pub(crate) catch_pc: usize,
    pub(crate) stack_len: usize,
    pub(crate) env: Option<EnvRef>,
    pub(crate) local_frames: usize,
    pub(crate) locals: usize,
    pub(crate) index: usize,
}

/// Non-capturing functions keep their locals in flat per-frame storage
/// instead of a heap environment, mirroring the stack slots QuickJS keeps
/// in `JSStackFrame` (quickjs.c:427).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventLoop {
    Env,
    Locals,
}

/// Activation record. QuickJS' equivalent is `JSStackFrame` (quickjs.c:427)
/// with `var_refs`, `sp` and the operand stack in `JS_CallInternal`
/// (quickjs.c:18051). It is owned so `await` can move it into the job queue.
pub(crate) struct Frame {
    pub(crate) definition: Rc<FunctionDef>,
    pub(crate) env: Option<EnvRef>,
    pub(crate) event_loop: EventLoop,
    /// Flat slot storage for non-capturing functions, mirroring
    /// `JSStackFrame.slots`. Each entry is `(offset, size)`.
    pub(crate) local_frames: SmallVec<[(usize, usize); 4]>,
    pub(crate) locals: SmallVec<[JsValue; 4]>,
    pub(crate) this: JsValue,
    pub(crate) callee: Option<JsValue>,
    pub(crate) stack: ValueStack,
    pub(crate) arguments: ArgumentStack,
    pub(crate) handlers: Vec<Handler>,
    pub(crate) pc: usize,
    pub(crate) completion: Option<ObjRef>,
}

impl Frame {
    pub(crate) fn new() -> Box<Self> {
        Box::new(Self {
            definition: FunctionDef::empty(),
            env: None,
            event_loop: EventLoop::Locals,
            local_frames: SmallVec::new(),
            locals: SmallVec::new(),
            this: JsValue::Undefined,
            callee: None,
            stack: ValueStack::new(),
            arguments: ArgumentStack::new(),
            handlers: Vec::new(),
            pc: 0,
            completion: None,
        })
    }

    /// Reset the flat-local or environment storage for a fresh call.
    pub(crate) fn begin(&mut self, use_env: bool) {
        self.local_frames.clear();
        self.locals.clear();
        self.handlers.clear();
        if !use_env {
            let size = self.definition.env_size as usize;
            self.locals.resize(size, JsValue::Undefined);
            self.local_frames.push((0, size));
            if self.definition.name.as_ref() == "<class_fields>" {
                for slot in 0..size {
                    self.locals[slot] = crate::object::Object::from_value(&self.this)
                        .map(|object| object.get(&slot.to_string()))
                        .unwrap_or(JsValue::Undefined);
                }
            }
            return;
        }
        if self.definition.name.as_ref() == "<class_fields>"
            && let Some(env) = &self.env
        {
            super::call::copy_this_fields(&self.this, env, self.definition.env_size as usize);
        }
    }

    /// Drop every reference the frame still holds, for pool reuse.
    pub(crate) fn clear(&mut self) {
        self.stack.clear();
        self.arguments.clear();
        self.handlers.clear();
        self.locals.clear();
        self.local_frames.clear();
        self.env = None;
        self.definition = FunctionDef::empty();
        self.this = JsValue::Undefined;
        self.callee = None;
        self.completion = None;
    }

    /// Truncate the scope stack back to a saved depth (exception unwind).
    pub(crate) fn restore_scope(&mut self, local_frames: usize, locals: usize) {
        self.local_frames.truncate(local_frames);
        self.locals.truncate(locals);
    }

    #[inline]
    pub(crate) fn get_var(&self, depth: u16, slot: u16) -> JsValue {
        match self.event_loop {
            EventLoop::Env => self
                .env
                .as_ref()
                .map(|env| env.get(depth as usize, slot as usize))
                .unwrap_or(JsValue::Undefined),
            EventLoop::Locals => {
                let (offset, size) = if depth == 0 {
                    match self.local_frames.last() {
                        Some(frame) => *frame,
                        None => return JsValue::Undefined,
                    }
                } else {
                    let count = self.local_frames.len();
                    let depth = depth as usize;
                    if depth >= count {
                        return JsValue::Undefined;
                    }
                    self.local_frames[count - 1 - depth]
                };
                let slot = slot as usize;
                if slot < size {
                    self.locals[offset + slot].clone()
                } else {
                    JsValue::Undefined
                }
            }
        }
    }

    #[inline]
    pub(crate) fn set_var(&mut self, depth: u16, slot: u16, value: JsValue) {
        match self.event_loop {
            EventLoop::Env => {
                if let Some(env) = &self.env {
                    env.set(depth as usize, slot as usize, value);
                }
            }
            EventLoop::Locals => {
                let frame = if depth == 0 {
                    self.local_frames.last().copied()
                } else {
                    let count = self.local_frames.len();
                    let depth = depth as usize;
                    (depth < count).then(|| self.local_frames[count - 1 - depth])
                };
                if let Some((offset, size)) = frame {
                    let slot = slot as usize;
                    if slot < size {
                        self.locals[offset + slot] = value;
                    }
                }
            }
        }
    }

    pub(crate) fn env(&self) -> &EnvRef {
        self.env
            .as_ref()
            .expect("capturing function must have an environment")
    }
}
