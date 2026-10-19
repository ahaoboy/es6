//! Activation record.
//!
//! QuickJS' equivalent is `JSStackFrame` (quickjs.c:427) with `var_refs`,
//! `cur_sp` and the operand stack of `JS_CallInternal` (quickjs.c:18051).
//! The frame is heap owned so `await` and generator `yield` can move it into
//! the job queue and back.

use std::rc::Rc;

use crate::bytecode::FunctionDef;
use crate::object::ObjRef;
use crate::value::{EnvRef, JsValue};

use super::{ArgumentStack, ValueStack};

/// Dispatch result for one opcode.
///
/// Payloads stay on the value stack (and `frame.pc` carries the resume
/// point), so the common `Continue` path returns a single-byte enum instead
/// of a 24-byte tagged value — the interpreter executes this return once per
/// opcode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Continue,
    Return,
    Await,
    /// `yield`: suspend the frame at `frame.pc` (quickjs `OP_yield`,
    /// quickjs.c:20871).
    Yield,
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

/// Activation record. QuickJS' equivalent is `JSStackFrame` (quickjs.c:427)
/// with `var_refs`, `sp` and the operand stack in `JS_CallInternal`
/// (quickjs.c:18051). It is owned so `await` can move it into the job queue.
pub(crate) struct Frame {
    pub(crate) definition: Rc<FunctionDef>,
    pub(crate) env: Option<EnvRef>,
    /// Flat slot storage for non-capturing functions, mirroring
    /// `JSStackFrame.slots`. Each entry is `(offset, size)`.
    pub(crate) local_frames: Vec<(usize, usize)>,
    /// Base and size of the innermost local scope, cached so the dominant
    /// `depth == 0` access does not walk `local_frames` (QuickJS keeps the
    /// equivalent in `JSStackFrame.slots`).
    pub(crate) local_offset: usize,
    pub(crate) local_size: usize,
    pub(crate) locals: Vec<JsValue>,
    pub(crate) this: JsValue,
    /// A sloppy call with an undefined/null receiver: this resolves to the
    /// global object lazily, so callees that never read it skip the Rc
    /// traffic entirely.
    pub(crate) global_this: bool,
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
            local_frames: Vec::new(),
            local_offset: 0,
            local_size: 0,
            locals: Vec::new(),
            this: JsValue::Undefined,
            global_this: false,
            callee: None,
            stack: ValueStack::new(),
            arguments: ArgumentStack::new(),
            handlers: Vec::new(),
            pc: 0,
            completion: None,
        })
    }

    /// Reset the flat frame and the optional environment for a fresh call.
    /// Pooled frames arrive empty (`recycle_frame` clears them), so only the
    /// root scope has to be materialized.
    pub(crate) fn begin(&mut self, use_env: bool) {
        self.local_offset = 0;
        self.local_size = 0;
        let size = self.definition.env_size as usize;
        self.locals.resize(size, JsValue::Undefined);
        self.local_frames.push((0, size));
        self.local_size = size;
        // Leading plain parameters land directly in their slots, like
        // QuickJS' contiguous argument buffer (quickjs.c:18132). Captured
        // parameters are read from the environment, so both stores receive
        // the value; a write later updates only the captured one.
        let simple = (self.definition.simple_params as usize).min(size);
        for index in 0..simple {
            if let Some(value) = self.arguments.get(index) {
                self.locals[index] = value.clone();
            }
        }
        if self.definition.is_class_fields {
            for slot in 0..size {
                self.locals[slot] = crate::object::Object::from_value(&self.this)
                    .map(|object| object.get(&slot.to_string()))
                    .unwrap_or(JsValue::Undefined);
            }
        }
        if !use_env {
            return;
        }
        if let Some(env) = &self.env {
            let slots = env.slots.borrow().len();
            for index in 0..simple.min(slots) {
                if let Some(value) = self.arguments.get(index) {
                    env.slots.borrow_mut()[index] = value.clone();
                }
            }
        }
        if self.definition.is_class_fields
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
        self.local_offset = 0;
        self.local_size = 0;
        self.env = None;
        // `definition` is overwritten before the frame runs again; leaving
        // the Rc in place avoids a thread-local lookup per call.
        self.this = JsValue::Undefined;
        self.global_this = false;
        self.callee = None;
        self.completion = None;
    }

    /// Effective receiver: substitutes the global object for sloppy calls
    /// that were invoked without one.
    #[inline(always)]
    pub(crate) fn this_value(&self, realm: &crate::runtime::Realm) -> JsValue {
        if self.global_this {
            JsValue::Object(realm.global.clone())
        } else {
            self.this.clone()
        }
    }

    /// Truncate the scope stack back to a saved depth (exception unwind).
    pub(crate) fn restore_scope(&mut self, local_frames: usize, locals: usize) {
        self.local_frames.truncate(local_frames);
        self.locals.truncate(locals);
        match self.local_frames.last() {
            Some((offset, size)) => {
                self.local_offset = *offset;
                self.local_size = *size;
            }
            None => {
                self.local_offset = 0;
                self.local_size = 0;
            }
        }
    }

    #[inline(always)]
    pub(crate) fn get_var(&self, depth: u16, slot: u16) -> JsValue {
        if slot & crate::bytecode::ENV_SLOT != 0 {
            let slot = (slot & !crate::bytecode::ENV_SLOT) as usize;
            let value = self
                .env
                .as_ref()
                .map(|env| env.get(depth as usize, slot))
                .unwrap_or(JsValue::Undefined);

            return value;
        }
        if depth == 0 {
            let slot = slot as usize;
            if slot < self.local_size
                && let Some(value) = self.locals.get(self.local_offset + slot)
            {
                return value.clone();
            }

            return JsValue::Undefined;
        }
        let count = self.local_frames.len();
        let depth = depth as usize;
        if depth >= count {
            return JsValue::Undefined;
        }
        let (offset, size) = self.local_frames[count - 1 - depth];
        let slot = slot as usize;
        if slot >= size {
            return JsValue::Undefined;
        }
        self.locals
            .get(offset + slot)
            .cloned()
            .unwrap_or(JsValue::Undefined)
    }

    #[inline(always)]
    pub(crate) fn set_var(&mut self, depth: u16, slot: u16, value: JsValue) {
        if slot & crate::bytecode::ENV_SLOT != 0 {
            let slot = (slot & !crate::bytecode::ENV_SLOT) as usize;

            if let Some(env) = &self.env {
                env.set(depth as usize, slot, value);
            }
            return;
        }
        if depth == 0 {
            let slot = slot as usize;

            if slot < self.local_size
                && let Some(target) = self.locals.get_mut(self.local_offset + slot)
            {
                *target = value;
            }
            return;
        }
        let count = self.local_frames.len();
        let depth = depth as usize;
        if depth < count {
            let (offset, size) = self.local_frames[count - 1 - depth];
            let slot = slot as usize;
            if slot < size
                && let Some(target) = self.locals.get_mut(offset + slot)
            {
                *target = value;
            }
        }
    }
}

impl boa_gc::Finalize for Handler {}
#[allow(unsafe_code)]
unsafe impl boa_gc::Trace for Handler {
    boa_gc::custom_trace!(this, mark, {
        if let Some(env) = &this.env {
            mark(env);
        }
    });
}

impl boa_gc::Finalize for Frame {}
#[allow(unsafe_code)]
unsafe impl boa_gc::Trace for Frame {
    boa_gc::custom_trace!(this, mark, {
        if let Some(env) = &this.env {
            mark(env);
        }
        for value in &this.locals {
            mark(value);
        }
        mark(&this.this);
        if let Some(callee) = &this.callee {
            mark(callee);
        }
        for value in &this.stack {
            mark(value);
        }
        for value in &this.arguments {
            mark(value);
        }
        for handler in &this.handlers {
            mark(handler);
        }
        if let Some(completion) = &this.completion {
            mark(completion);
        }
    });
}
