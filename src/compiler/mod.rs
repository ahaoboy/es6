use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::*;
use crate::bytecode::{ClassDef, ClassMethod, CompareOp, Constant, FunctionDef, Op};
use crate::parser::ParseError;

#[derive(Clone)]
struct ScopeFrame {
    bindings: HashMap<String, u16>,
    size: u16,
}

impl ScopeFrame {
    fn new() -> Self {
        Self {
            bindings: HashMap::new(),
            size: 0,
        }
    }

    fn declare(&mut self, name: &str) -> u16 {
        if let Some(slot) = self.bindings.get(name) {
            return *slot;
        }
        let slot = self.size;
        self.bindings.insert(name.to_owned(), slot);
        self.size += 1;
        slot
    }
}

#[derive(Clone, Copy)]
struct VarRef {
    depth: u16,
    slot: u16,
}

struct BreakContext {
    label: Option<String>,
    env_depth: usize,
    stack_cleanup: usize,
    try_depth: usize,
    jumps: Vec<usize>,
}

struct LoopContext {
    label: Option<String>,
    env_depth: usize,
    stack_cleanup: usize,
    try_depth: usize,
    continue_target: Option<usize>,
    continue_jumps: Vec<usize>,
}

#[derive(Clone, Copy)]
enum BindingMode {
    Auto,
    Global,
}

/// Name of the exports namespace parameter of a compiled module body.
///
/// It corresponds to the `exports` argument of a CommonJS-style module wrapper;
/// QuickJS instead keeps `JSModuleDef` bindings in the module record
/// (`JSModuleDef`, quickjs.c:4381), which is the same indirection without a
/// named parameter.
pub const MODULE_EXPORTS_BINDING: &str = "__module_exports__";

pub fn compile_program(program: &Program) -> Result<Rc<FunctionDef>, ParseError> {
    let mut compiler = Compiler::new(true, Vec::new(), "<eval>".into());
    compiler.compile_script(&program.body)?;
    let strict = has_use_strict(&program.body);
    let def = compiler.finish("<eval>", 0, false, false, false, strict);
    // Top-level bytecode registers with the GC like QuickJS'
    // `JS_GC_OBJ_TYPE_FUNCTION_BYTECODE` (quickjs.c:450).
    Ok(crate::gc::register_def(Rc::new(def)))
}

/// Compile an ES module body into a function taking the exports namespace.
///
/// ES modules are strict and their bindings are module-scoped, so the body is
/// compiled as a function rather than as global script code. This is the
/// equivalent of wrapping the source in `(function (exports) { ... })`, which
/// keeps top-level `var`/`let`/`const` private to the module — the role
/// `JSModuleDef` plays in QuickJS (quickjs.c:4381).
pub fn compile_module(program: &Program) -> Result<Rc<FunctionDef>, ParseError> {
    let mut compiler = Compiler::new(false, Vec::new(), "<module>".into());
    compiler.strict = true;
    let function = Function {
        name: Some("<module>".into()),
        params: vec![Pattern::Ident(MODULE_EXPORTS_BINDING.into())],
        body: FunctionBody::Block(program.body.clone()),
        is_arrow: false,
        is_async: false,
        is_generator: false,
        span: Span::default(),
    };
    let def = compiler.compile_function(&function, false, false)?;
    Ok(crate::gc::register_def(def))
}

pub fn compile_function_source(function: &Function) -> Result<Rc<FunctionDef>, ParseError> {
    let mut compiler = Compiler::new(false, Vec::new(), function.name.clone().unwrap_or_default());
    let def = compiler.compile_function(function, false, false)?;
    Ok(crate::gc::register_def(def))
}

/// Scope-resolving bytecode emitter. The two-level scope model corresponds to
/// QuickJS' `JSFunctionDef` + `JSVarDef`/`closure_var` bookkeeping and its
/// `emit_op` helpers (quickjs.c:24059).
struct Compiler {
    code: Vec<Op>,
    positions: Vec<u32>,
    span: Span,
    constants: Vec<Constant>,
    prop_caches: Vec<crate::object::PropCache>,
    local_frames: Vec<ScopeFrame>,
    captured_frames: Vec<ScopeFrame>,
    used_captured: bool,
    script: bool,
    strict: bool,
    breakables: Vec<BreakContext>,
    loops: Vec<LoopContext>,
    try_depth: usize,
    finally_stack: Vec<Vec<Stmt>>,
    temp_counter: usize,
    source_name: String,
    super_allowed: bool,
}

impl Compiler {
    fn new(script: bool, captured_frames: Vec<ScopeFrame>, source_name: String) -> Self {
        Self {
            code: Vec::new(),
            positions: Vec::new(),
            span: Span::default(),
            constants: Vec::new(),
            prop_caches: Vec::new(),
            local_frames: vec![ScopeFrame::new()],
            captured_frames,
            used_captured: false,
            script,
            strict: false,
            breakables: Vec::new(),
            loops: Vec::new(),
            try_depth: 0,
            finally_stack: Vec::new(),
            temp_counter: 0,
            source_name,
            super_allowed: false,
        }
    }

    fn finish(
        mut self,
        name: &str,
        length: u32,
        is_arrow: bool,
        is_async: bool,
        is_generator: bool,
        strict: bool,
    ) -> FunctionDef {
        if !matches!(self.code.last(), Some(Op::Return | Op::ReturnUndefined)) {
            self.emit(Op::ReturnUndefined);
        }
        let use_env = self.used_captured
            || self
                .code
                .iter()
                .any(|op| matches!(op, Op::Closure(_) | Op::CreateClass(_)));
        FunctionDef {
            name: Rc::from(name),
            code: self.code,
            positions: self.positions,
            constants: self.constants,
            prop_caches: self
                .prop_caches
                .into_iter()
                .map(std::cell::RefCell::new)
                .collect(),
            env_size: self.local_frames[0].size,
            use_env,
            length,
            is_arrow,
            is_async,
            is_generator,
            strict,
            is_class_fields: name == "<class_fields>",
            source: self.source_name,
        }
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            message: message.into(),
            line: 0,
            column: 0,
            offset: 0,
        }
    }

    fn emit(&mut self, op: Op) {
        self.code.push(op);
        self.positions.push(self.span.start as u32);
    }

    fn position(&self) -> usize {
        self.code.len()
    }

    fn emit_jump(&mut self, op: Op) -> usize {
        let index = self.code.len();
        self.code.push(op);
        self.positions.push(self.span.start as u32);
        index
    }

    fn patch_jump(&mut self, index: usize, target: usize) {
        match &mut self.code[index] {
            Op::Jump(value)
            | Op::JumpIfFalse(value)
            | Op::JumpIfTrue(value)
            | Op::JumpIfNotUndefined(value)
            | Op::JumpIfNullish(value)
            | Op::JumpIfNullishKeep(value)
            | Op::TryStart(value)
            | Op::IterNext(value)
            | Op::Yield(value)
            | Op::CompareVarsJump { target: value, .. }
            | Op::CompareVarConstJump { target: value, .. }
            | Op::BitAndVarConstJump { target: value, .. } => *value = target as u32,
            _ => {}
        }
    }

    fn add_constant(&mut self, constant: Constant) -> u32 {
        let index = self.constants.len() as u32;
        self.constants.push(constant);
        index
    }

    fn push_string(&mut self, value: &str) {
        let index = self.string_constant(value);
        self.emit(Op::PushConst(index));
    }

    fn string_constant(&mut self, value: &str) -> u32 {
        self.add_constant(Constant::String(crate::string::JsString::from(value)))
    }

    fn number_constant(&mut self, value: f64) -> u32 {
        let constant =
            if value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64 {
                Constant::Int(value as i32)
            } else {
                Constant::Number(value)
            };
        self.add_constant(constant)
    }

    fn add_prop_cache(&mut self) -> u32 {
        let index = self.prop_caches.len() as u32;
        self.prop_caches.push(crate::object::PropCache::default());
        index
    }

    fn emit_get_prop_const(&mut self, name: &str) {
        let key = self.string_constant(name);
        let cache = self.add_prop_cache();
        self.emit(Op::GetPropConst { key, cache });
    }

    fn emit_get_global(&mut self, name: &str) {
        let name = self.string_constant(name);
        let cache = self.add_prop_cache();
        self.emit(Op::GetGlobal { name, cache });
    }

    fn emit_set_prop_const(&mut self, name: &str, keep: bool) {
        let key = self.string_constant(name);
        let cache = self.add_prop_cache();
        self.emit(if keep {
            Op::SetPropConstKeep { key, cache }
        } else {
            Op::SetPropConst { key, cache }
        });
    }

    fn resolve(&mut self, name: &str) -> Option<VarRef> {
        let local_len = self.local_frames.len();
        for (index, frame) in self.local_frames.iter().enumerate().rev() {
            if let Some(slot) = frame.bindings.get(name) {
                return Some(VarRef {
                    depth: (local_len - 1 - index) as u16,
                    slot: *slot,
                });
            }
        }
        for (index, frame) in self.captured_frames.iter().enumerate() {
            if let Some(slot) = frame.bindings.get(name) {
                self.used_captured = true;
                return Some(VarRef {
                    depth: (local_len + index) as u16,
                    slot: *slot,
                });
            }
        }
        None
    }

    fn capture_frames(&self) -> Vec<ScopeFrame> {
        let mut frames: Vec<ScopeFrame> = self.local_frames.iter().rev().cloned().collect();
        frames.extend(self.captured_frames.iter().cloned());
        frames
    }

    fn lookup_global_mode(&self) -> bool {
        self.script && self.local_frames.len() == 1
    }

    fn compile_script(&mut self, statements: &[Stmt]) -> Result<(), ParseError> {
        // ES module imports are evaluated before the body of the module.
        for statement in statements {
            if let StmtKind::Import(import) = &statement.kind {
                self.compile_import(import)?;
            }
        }
        // Global `var` bindings are hoisted and initialized to undefined before
        // any statement runs; function declarations overwrite them afterwards.
        for name in collect_var_names(statements) {
            self.emit(Op::Undefined);
            let name = self.string_constant(&name);
            self.emit(Op::SetGlobal(name));
        }
        self.hoist_functions(statements, true)?;
        for (index, statement) in statements.iter().enumerate() {
            if matches!(statement.kind, StmtKind::Function(_) | StmtKind::Import(_)) {
                continue;
            }
            if index + 1 == statements.len()
                && let StmtKind::Expr(expression) = &statement.kind
            {
                self.compile_expr(expression)?;
                self.emit(Op::Return);
                return Ok(());
            }
            self.compile_statement(statement)?;
        }
        self.emit(Op::Undefined);
        self.emit(Op::Return);
        Ok(())
    }

    fn compile_function_body(&mut self, statements: &[Stmt]) -> Result<(), ParseError> {
        // Only reachable for module bodies: imports are evaluated before any
        // statement, exactly like `compile_script` hoists them.
        for statement in statements {
            if let StmtKind::Import(import) = &statement.kind {
                self.compile_import(import)?;
            }
        }
        self.hoist_functions(statements, false)?;
        for statement in statements {
            if matches!(statement.kind, StmtKind::Function(_) | StmtKind::Import(_)) {
                continue;
            }
            self.compile_statement(statement)?;
        }
        Ok(())
    }

    fn compile_function(
        &mut self,
        function: &Function,
        is_constructor: bool,
        super_allowed: bool,
    ) -> Result<Rc<FunctionDef>, ParseError> {
        let mut child = Compiler::new(false, self.capture_frames(), self.source_name.clone());
        child.super_allowed = super_allowed;
        child.strict = self.strict
            || match &function.body {
                FunctionBody::Block(statements) => has_use_strict(statements),
                FunctionBody::Expr(_) => false,
            };
        for param in &function.params {
            for name in pattern_names(param) {
                child.local_frames[0].declare(&name);
            }
        }
        let arguments_slot = if function.is_arrow || !function_uses_arguments(function) {
            None
        } else {
            let slot = child.local_frames[0].declare("arguments");
            child.emit(Op::Arguments);
            child.emit(Op::SetVar { depth: 0, slot });
            Some(())
        };
        let _ = arguments_slot;
        // Named function expressions bind their own name in the function
        // scope so the body can self-reference it, like QuickJS' function
        // scope var for named expressions (`js_parse_function_decl2`,
        // quickjs.c:37379).
        if !function.is_arrow
            && let Some(name) = &function.name
            && !name.is_empty()
        {
            let slot = child.local_frames[0].declare(name);
            child.emit(Op::Callee);
            child.emit(Op::SetVar { depth: 0, slot });
        }
        if let FunctionBody::Block(statements) = &function.body {
            for name in collect_var_names(statements) {
                child.local_frames[0].declare(&name);
            }
            for name in collect_direct_bindings(statements) {
                child.local_frames[0].declare(&name);
            }
        }
        for (index, param) in function.params.iter().enumerate() {
            match param {
                Pattern::Rest(inner) => {
                    child.emit(Op::RestArgs(index as u16));
                    child.bind_pattern(inner, BindingMode::Auto)?;
                }
                _ => {
                    child.emit(Op::LoadArg(index as u16));
                    child.bind_pattern(param, BindingMode::Auto)?;
                }
            }
        }
        match &function.body {
            FunctionBody::Block(statements) => {
                child.compile_function_body(statements)?;
            }
            FunctionBody::Expr(expression) => {
                child.compile_expr(expression)?;
                child.emit(Op::Return);
            }
        }
        let name = function
            .name
            .clone()
            .unwrap_or_else(|| "<anonymous>".into());
        let mut length = 0u32;
        for param in &function.params {
            match param {
                Pattern::Ident(_) => length += 1,
                Pattern::Assign { target, .. } if matches!(**target, Pattern::Ident(_)) => {
                    length += 1
                }
                _ => break,
            }
        }
        let is_arrow = function.is_arrow;
        let is_async = function.is_async;
        let is_generator = function.is_generator;
        let is_strict = child.strict;
        let _ = is_constructor;
        Ok(Rc::new(child.finish(
            &name,
            length,
            is_arrow,
            is_async,
            is_generator,
            is_strict,
        )))
    }

    fn hoist_functions(
        &mut self,
        statements: &[Stmt],
        _script_scope: bool,
    ) -> Result<(), ParseError> {
        for statement in statements {
            let function = match &statement.kind {
                StmtKind::Function(function) => Some(function),
                // `export function f() {}` hoists like a plain declaration.
                StmtKind::Export(export) => match &export.kind {
                    ExportKind::Declaration(inner) => match &inner.kind {
                        StmtKind::Function(function) => Some(function),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            };
            let Some(function) = function else {
                continue;
            };
            let Some(name) = &function.name else {
                continue;
            };
            let def = self.compile_function(function, false, false)?;
            let constant = self.add_constant(Constant::Function(def));
            self.emit(Op::Closure(constant));
            self.store_ident(name, BindingMode::Auto);
        }
        Ok(())
    }
}

mod class;
mod expr;
mod helpers;
mod pattern;
mod stmt;

use helpers::*;
