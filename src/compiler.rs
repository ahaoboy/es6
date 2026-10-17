use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::*;
use crate::bytecode::{ClassDef, ClassMethod, Constant, FunctionDef, Op};
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

pub fn compile_program(program: &Program) -> Result<Rc<FunctionDef>, ParseError> {
    let mut compiler = Compiler::new(true, Vec::new(), "<eval>".into());
    compiler.compile_script(&program.body)?;
    let strict = has_use_strict(&program.body);
    let def = compiler.finish("<eval>", 0, false, false, strict);
    Ok(Rc::new(def))
}

pub fn compile_function_source(function: &Function) -> Result<Rc<FunctionDef>, ParseError> {
    let mut compiler = Compiler::new(false, Vec::new(), function.name.clone().unwrap_or_default());
    let def = compiler.compile_function(function, false, false)?;
    Ok(def)
}

/// Scope-resolving bytecode emitter. The two-level scope model corresponds to
/// QuickJS' `JSFunctionDef` + `JSVarDef`/`closure_var` bookkeeping and its
/// `emit_op` helpers (quickjs.c:24059).
struct Compiler {
    code: Vec<Op>,
    positions: Vec<u32>,
    span: Span,
    constants: Vec<Constant>,
    local_frames: Vec<ScopeFrame>,
    captured_frames: Vec<ScopeFrame>,
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
            local_frames: vec![ScopeFrame::new()],
            captured_frames,
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
        strict: bool,
    ) -> FunctionDef {
        if !matches!(self.code.last(), Some(Op::Return | Op::ReturnUndefined)) {
            self.emit(Op::ReturnUndefined);
        }
        FunctionDef {
            name: Rc::from(name),
            code: self.code,
            positions: self.positions,
            constants: self.constants,
            env_size: self.local_frames[0].size,
            length,
            is_arrow,
            is_async,
            strict,
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
            | Op::IterNext(value) => *value = target,
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
        self.add_constant(Constant::String(Rc::from(value)))
    }

    fn resolve(&self, name: &str) -> Option<VarRef> {
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
        // Global `var` bindings are hoisted and initialized to undefined before
        // any statement runs; function declarations overwrite them afterwards.
        for name in collect_var_names(statements) {
            self.emit(Op::Undefined);
            let name = self.string_constant(&name);
            self.emit(Op::SetGlobal(name));
        }
        self.hoist_functions(statements, true)?;
        for (index, statement) in statements.iter().enumerate() {
            if matches!(statement.kind, StmtKind::Function(_)) {
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
        self.hoist_functions(statements, false)?;
        for statement in statements {
            if matches!(statement.kind, StmtKind::Function(_)) {
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
        let arguments_slot = if function.is_arrow {
            None
        } else {
            let slot = child.local_frames[0].declare("arguments");
            child.emit(Op::Arguments);
            child.emit(Op::SetVar { depth: 0, slot });
            Some(())
        };
        let _ = arguments_slot;
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
        let is_strict = child.strict;
        let _ = is_constructor;
        Ok(Rc::new(
            child.finish(&name, length, is_arrow, is_async, is_strict),
        ))
    }

    fn hoist_functions(
        &mut self,
        statements: &[Stmt],
        _script_scope: bool,
    ) -> Result<(), ParseError> {
        for statement in statements {
            if let StmtKind::Function(function) = &statement.kind {
                let Some(name) = &function.name else {
                    continue;
                };
                let def = self.compile_function(function, false, false)?;
                let constant = self.add_constant(Constant::Function(def));
                self.emit(Op::Closure(constant));
                self.store_ident(name, BindingMode::Auto);
            }
        }
        Ok(())
    }

    /// QuickJS `js_parse_statement` lowering (quickjs.c:28756).
    fn compile_statement(&mut self, statement: &Stmt) -> Result<(), ParseError> {
        self.span = statement.span;
        match &statement.kind {
            StmtKind::Empty | StmtKind::Debugger => {}
            StmtKind::Expr(expression) => {
                self.compile_expr(expression)?;
                self.emit(Op::Pop);
            }
            StmtKind::Block(statements) => {
                self.compile_block(statements)?;
            }
            StmtKind::Var(declaration) => {
                self.compile_var_declaration(declaration)?;
            }
            StmtKind::Function(function) => {
                let def = self.compile_function(function, false, false)?;
                let constant = self.add_constant(Constant::Function(def));
                self.emit(Op::Closure(constant));
                if let Some(name) = &function.name {
                    self.store_ident(name, BindingMode::Auto);
                } else {
                    self.emit(Op::Pop);
                }
            }
            StmtKind::Class(class) => {
                self.compile_class(class)?;
                if let Some(name) = &class.name {
                    self.store_ident(name, BindingMode::Auto);
                } else {
                    self.emit(Op::Pop);
                }
            }
            StmtKind::If {
                test,
                consequent,
                alternate,
            } => {
                self.compile_expr(test)?;
                let else_jump = self.emit_jump(Op::JumpIfFalse(0));
                self.compile_stmt_or_block(consequent)?;
                if let Some(alternate) = alternate {
                    let end_jump = self.emit_jump(Op::Jump(0));
                    let else_target = self.position();
                    self.patch_jump(else_jump, else_target);
                    self.compile_stmt_or_block(alternate)?;
                    let end = self.position();
                    self.patch_jump(end_jump, end);
                } else {
                    let end = self.position();
                    self.patch_jump(else_jump, end);
                }
            }
            StmtKind::While { test, body } => {
                self.compile_while(test, body)?;
            }
            StmtKind::DoWhile { body, test } => {
                self.compile_do_while(body, test)?;
            }
            StmtKind::For {
                init,
                test,
                update,
                body,
            } => {
                self.compile_for(init, test, update, body)?;
            }
            StmtKind::ForIn { left, right, body } => {
                self.compile_for_in_of(left, right, body, true, false)?;
            }
            StmtKind::ForOf { left, right, body } => {
                self.compile_for_in_of(left, right, body, false, false)?;
            }
            StmtKind::Return(argument) => {
                if let Some(argument) = argument {
                    self.compile_expr(argument)?;
                    self.emit_active_finalizers()?;
                    self.emit(Op::Return);
                } else {
                    self.emit_active_finalizers()?;
                    self.emit(Op::ReturnUndefined);
                }
            }
            StmtKind::Break(label) => {
                self.compile_break(label.as_deref())?;
            }
            StmtKind::Continue(label) => {
                self.compile_continue(label.as_deref())?;
            }
            StmtKind::Throw(expression) => {
                self.compile_expr(expression)?;
                self.emit(Op::Throw);
            }
            StmtKind::Try {
                block,
                handler,
                finalizer,
            } => {
                self.compile_try(block, handler.as_ref(), finalizer.as_deref())?;
            }
            StmtKind::Switch {
                discriminant,
                cases,
            } => {
                self.compile_switch(discriminant, cases)?;
            }
            StmtKind::Labeled { label, body } => {
                let context = BreakContext {
                    label: Some(label.clone()),
                    env_depth: self.local_frames.len(),
                    stack_cleanup: 0,
                    try_depth: self.try_depth,
                    jumps: Vec::new(),
                };
                self.breakables.push(context);
                self.compile_statement(body)?;
                let context = self.breakables.pop().unwrap();
                let end = self.position();
                for jump in context.jumps {
                    self.patch_jump(jump, end);
                }
            }
        }
        Ok(())
    }

    fn compile_stmt_or_block(&mut self, statement: &Stmt) -> Result<(), ParseError> {
        self.compile_statement(statement)
    }

    fn compile_block(&mut self, statements: &[Stmt]) -> Result<(), ParseError> {
        let names = collect_direct_bindings(statements);
        if names.is_empty() {
            self.hoist_functions(statements, false)?;
            for statement in statements {
                if matches!(statement.kind, StmtKind::Function(_)) {
                    continue;
                }
                self.compile_statement(statement)?;
            }
            return Ok(());
        }
        let mut frame = ScopeFrame::new();
        for name in &names {
            frame.declare(name);
        }
        let size = frame.size;
        self.local_frames.push(frame);
        self.emit(Op::PushEnv(size));
        self.hoist_functions(statements, false)?;
        for statement in statements {
            if matches!(statement.kind, StmtKind::Function(_)) {
                continue;
            }
            self.compile_statement(statement)?;
        }
        self.emit(Op::PopEnv);
        self.local_frames.pop();
        Ok(())
    }

    fn compile_var_declaration(&mut self, declaration: &VarDecl) -> Result<(), ParseError> {
        for declarator in &declaration.declarations {
            // `var x;` is fully handled by hoisting. In particular it must not
            // overwrite a parameter binding with the same name.
            if declarator.init.is_none() && declaration.kind == VarKind::Var {
                continue;
            }
            if let Some(init) = &declarator.init {
                self.compile_expr(init)?;
            } else {
                self.emit(Op::Undefined);
            }
            if declaration.kind == VarKind::Var && self.lookup_global_mode() {
                self.bind_pattern(&declarator.pattern, BindingMode::Global)?;
            } else {
                self.bind_pattern(&declarator.pattern, BindingMode::Auto)?;
            }
        }
        Ok(())
    }

    fn compile_while(&mut self, test: &Expr, body: &Stmt) -> Result<(), ParseError> {
        let start = self.position();
        self.compile_expr(test)?;
        let end_jump = self.emit_jump(Op::JumpIfFalse(0));
        let break_context = BreakContext {
            label: None,
            env_depth: self.local_frames.len(),
            stack_cleanup: 0,
            try_depth: self.try_depth,
            jumps: Vec::new(),
        };
        let loop_context = LoopContext {
            label: None,
            env_depth: self.local_frames.len(),
            stack_cleanup: 0,
            try_depth: self.try_depth,
            continue_target: Some(start),
            continue_jumps: Vec::new(),
        };
        self.breakables.push(break_context);
        self.loops.push(loop_context);
        self.compile_statement(body)?;
        let loop_context = self.loops.pop().unwrap();
        let break_context = self.breakables.pop().unwrap();
        self.emit(Op::Jump(start));
        let end = self.position();
        self.patch_jump(end_jump, end);
        for jump in break_context.jumps {
            self.patch_jump(jump, end);
        }
        for jump in loop_context.continue_jumps {
            self.patch_jump(jump, start);
        }
        Ok(())
    }

    fn compile_do_while(&mut self, body: &Stmt, test: &Expr) -> Result<(), ParseError> {
        let start = self.position();
        let break_context = BreakContext {
            label: None,
            env_depth: self.local_frames.len(),
            stack_cleanup: 0,
            try_depth: self.try_depth,
            jumps: Vec::new(),
        };
        let loop_context = LoopContext {
            label: None,
            env_depth: self.local_frames.len(),
            stack_cleanup: 0,
            try_depth: self.try_depth,
            continue_target: None,
            continue_jumps: Vec::new(),
        };
        self.breakables.push(break_context);
        self.loops.push(loop_context);
        self.compile_statement(body)?;
        let continue_target = self.position();
        self.compile_expr(test)?;
        self.emit(Op::JumpIfTrue(start));
        let loop_context = self.loops.pop().unwrap();
        let break_context = self.breakables.pop().unwrap();
        let end = self.position();
        for jump in break_context.jumps {
            self.patch_jump(jump, end);
        }
        for jump in loop_context.continue_jumps {
            self.patch_jump(jump, continue_target);
        }
        Ok(())
    }

    fn compile_for(
        &mut self,
        init: &Option<ForInit>,
        test: &Option<Expr>,
        update: &Option<Expr>,
        body: &Stmt,
    ) -> Result<(), ParseError> {
        let lexical = matches!(
            init,
            Some(ForInit::Var(declaration)) if declaration.kind != VarKind::Var
        );
        if lexical {
            let declaration = match init {
                Some(ForInit::Var(declaration)) => declaration,
                _ => unreachable!(),
            };
            let mut frame = ScopeFrame::new();
            for declarator in &declaration.declarations {
                for name in pattern_names(&declarator.pattern) {
                    frame.declare(&name);
                }
            }
            let size = frame.size;
            let bindings = frame.bindings.clone();
            self.local_frames.push(frame);
            self.emit(Op::PushEnv(size));
            self.compile_var_declaration(declaration)?;
            let loop_depth = self.local_frames.len();

            let start = self.position();
            let mut iteration = ScopeFrame::new();
            iteration.bindings = bindings.clone();
            iteration.size = size;
            self.local_frames.push(iteration);
            self.emit(Op::PushEnv(size));
            for slot in bindings.values() {
                self.emit(Op::GetVar {
                    depth: 1,
                    slot: *slot,
                });
                self.emit(Op::SetVar {
                    depth: 0,
                    slot: *slot,
                });
            }
            let end_jump = if let Some(test) = test {
                self.compile_expr(test)?;
                Some(self.emit_jump(Op::JumpIfFalse(0)))
            } else {
                None
            };
            let break_context = BreakContext {
                label: None,
                env_depth: loop_depth,
                stack_cleanup: 0,
                try_depth: self.try_depth,
                jumps: Vec::new(),
            };
            let loop_context = LoopContext {
                label: None,
                env_depth: self.local_frames.len(),
                stack_cleanup: 0,
                try_depth: self.try_depth,
                continue_target: None,
                continue_jumps: Vec::new(),
            };
            self.breakables.push(break_context);
            self.loops.push(loop_context);
            self.compile_statement(body)?;
            let continue_target = self.position();
            if let Some(update) = update {
                self.compile_expr(update)?;
                self.emit(Op::Pop);
            }
            for slot in bindings.values() {
                self.emit(Op::GetVar {
                    depth: 0,
                    slot: *slot,
                });
                self.emit(Op::SetVar {
                    depth: 1,
                    slot: *slot,
                });
            }
            self.emit(Op::PopEnv);
            self.local_frames.pop();
            self.emit(Op::Jump(start));
            let loop_context = self.loops.pop().unwrap();
            let break_context = self.breakables.pop().unwrap();
            let exit = self.position();
            if let Some(jump) = end_jump {
                self.patch_jump(jump, exit);
            }
            self.emit(Op::PopEnv);
            let end = self.position();
            for jump in break_context.jumps {
                self.patch_jump(jump, end);
            }
            for jump in loop_context.continue_jumps {
                self.patch_jump(jump, continue_target);
            }
            self.emit(Op::PopEnv);
            self.local_frames.pop();
            return Ok(());
        }

        match init {
            Some(ForInit::Var(declaration)) => self.compile_var_declaration(declaration)?,
            Some(ForInit::Expr(expression)) => {
                self.compile_expr(expression)?;
                self.emit(Op::Pop);
            }
            None => {}
        }
        let start = self.position();
        let end_jump = if let Some(test) = test {
            self.compile_expr(test)?;
            Some(self.emit_jump(Op::JumpIfFalse(0)))
        } else {
            None
        };
        let break_context = BreakContext {
            label: None,
            env_depth: self.local_frames.len(),
            stack_cleanup: 0,
            try_depth: self.try_depth,
            jumps: Vec::new(),
        };
        let loop_context = LoopContext {
            label: None,
            env_depth: self.local_frames.len(),
            stack_cleanup: 0,
            try_depth: self.try_depth,
            continue_target: None,
            continue_jumps: Vec::new(),
        };
        self.breakables.push(break_context);
        self.loops.push(loop_context);
        self.compile_statement(body)?;
        let continue_target = self.position();
        if let Some(update) = update {
            self.compile_expr(update)?;
            self.emit(Op::Pop);
        }
        self.emit(Op::Jump(start));
        let loop_context = self.loops.pop().unwrap();
        let break_context = self.breakables.pop().unwrap();
        let end = self.position();
        if let Some(jump) = end_jump {
            self.patch_jump(jump, end);
        }
        for jump in break_context.jumps {
            self.patch_jump(jump, end);
        }
        for jump in loop_context.continue_jumps {
            self.patch_jump(jump, continue_target);
        }
        Ok(())
    }

    fn compile_for_in_of(
        &mut self,
        left: &ForInit,
        right: &Expr,
        body: &Stmt,
        is_in: bool,
        _is_await: bool,
    ) -> Result<(), ParseError> {
        let lexical = matches!(left, ForInit::Var(declaration) if declaration.kind != VarKind::Var);
        let outer_depth = self.local_frames.len();
        self.compile_expr(right)?;
        self.emit(if is_in { Op::ForInInit } else { Op::IterInit });
        let start = self.position();
        let end_jump = self.emit_jump(Op::IterNext(0));
        let mut iteration_size = 0;
        if lexical {
            let ForInit::Var(declaration) = left else {
                unreachable!();
            };
            let mut frame = ScopeFrame::new();
            if let Some(declarator) = declaration.declarations.first() {
                for name in pattern_names(&declarator.pattern) {
                    frame.declare(&name);
                }
            }
            iteration_size = frame.size;
            self.local_frames.push(frame);
            self.emit(Op::PushEnv(iteration_size));
        }
        match left {
            ForInit::Var(declaration) => {
                if let Some(declarator) = declaration.declarations.first() {
                    self.bind_pattern(&declarator.pattern, BindingMode::Auto)?;
                }
            }
            ForInit::Expr(expression) => {
                self.compile_assignment_target_store(expression)?;
            }
        }
        let break_context = BreakContext {
            label: None,
            env_depth: outer_depth,
            stack_cleanup: 1,
            try_depth: self.try_depth,
            jumps: Vec::new(),
        };
        let loop_context = LoopContext {
            label: None,
            env_depth: outer_depth,
            stack_cleanup: 0,
            try_depth: self.try_depth,
            continue_target: Some(start),
            continue_jumps: Vec::new(),
        };
        self.breakables.push(break_context);
        self.loops.push(loop_context);
        self.compile_statement(body)?;
        let loop_context = self.loops.pop().unwrap();
        let break_context = self.breakables.pop().unwrap();
        if lexical {
            self.emit(Op::PopEnv);
            self.local_frames.pop();
        }
        self.emit(Op::Jump(start));
        let end = self.position();
        self.patch_jump(end_jump, end);
        for jump in break_context.jumps {
            self.patch_jump(jump, end);
        }
        for jump in loop_context.continue_jumps {
            self.patch_jump(jump, start);
        }
        let _ = iteration_size;
        Ok(())
    }

    fn compile_break(&mut self, label: Option<&str>) -> Result<(), ParseError> {
        let position = self
            .breakables
            .iter()
            .rposition(|context| match label {
                Some(label) => context.label.as_deref() == Some(label),
                None => true,
            })
            .ok_or_else(|| self.error("break outside of loop or switch"))?;
        let (env_depth, stack_cleanup, try_depth) = {
            let context = &self.breakables[position];
            (context.env_depth, context.stack_cleanup, context.try_depth)
        };
        self.emit_finalizers_until(try_depth)?;
        self.emit_cleanup(env_depth, stack_cleanup, try_depth);
        let jump = self.emit_jump(Op::Jump(0));
        self.breakables[position].jumps.push(jump);
        Ok(())
    }

    fn compile_continue(&mut self, label: Option<&str>) -> Result<(), ParseError> {
        let position = self
            .loops
            .iter()
            .rposition(|context| match label {
                Some(label) => context.label.as_deref() == Some(label),
                None => true,
            })
            .ok_or_else(|| self.error("continue outside of loop"))?;
        let (env_depth, stack_cleanup, try_depth, target) = {
            let context = &self.loops[position];
            (
                context.env_depth,
                context.stack_cleanup,
                context.try_depth,
                context.continue_target,
            )
        };
        self.emit_finalizers_until(try_depth)?;
        self.emit_cleanup(env_depth, stack_cleanup, try_depth);
        if let Some(target) = target {
            self.emit(Op::Jump(target));
        } else {
            let jump = self.emit_jump(Op::Jump(0));
            self.loops[position].continue_jumps.push(jump);
        }
        Ok(())
    }

    fn emit_active_finalizers(&mut self) -> Result<(), ParseError> {
        self.emit_finalizers_until(0)
    }

    fn emit_finalizers_until(&mut self, target_depth: usize) -> Result<(), ParseError> {
        let stack = self.finally_stack.clone();
        if stack.len() <= target_depth {
            return Ok(());
        }
        for index in (target_depth..stack.len()).rev() {
            let saved = self.finally_stack.clone();
            self.finally_stack.truncate(index);
            self.compile_block(&stack[index])?;
            self.finally_stack = saved;
        }
        Ok(())
    }

    fn emit_cleanup(&mut self, env_depth: usize, stack_cleanup: usize, try_depth: usize) {
        for _ in 0..stack_cleanup {
            self.emit(Op::Pop);
        }
        for _ in try_depth..self.try_depth {
            self.emit(Op::TryEnd);
        }
        for _ in env_depth..self.local_frames.len() {
            self.emit(Op::PopEnv);
        }
    }

    fn compile_try(
        &mut self,
        block: &[Stmt],
        handler: Option<&CatchClause>,
        finalizer: Option<&[Stmt]>,
    ) -> Result<(), ParseError> {
        let try_start = self.emit_jump(Op::TryStart(0));
        self.try_depth += 1;
        if let Some(finalizer) = finalizer {
            self.finally_stack.push(finalizer.to_vec());
        }
        self.compile_statements_in_scope(block)?;
        if finalizer.is_some() {
            self.finally_stack.pop();
        }
        self.emit(Op::TryEnd);
        self.try_depth -= 1;
        let normal_jump = self.emit_jump(Op::Jump(0));
        let handler_pc = self.position();
        self.patch_jump(try_start, handler_pc);

        if let Some(handler) = handler {
            let mut frame = ScopeFrame::new();
            if let Some(param) = &handler.param {
                for name in pattern_names(param) {
                    frame.declare(&name);
                }
            }
            let size = frame.size;
            if size > 0 {
                self.local_frames.push(frame);
                self.emit(Op::PushEnv(size));
            }
            if let Some(param) = &handler.param {
                self.bind_pattern(param, BindingMode::Auto)?;
            } else {
                self.emit(Op::Pop);
            }
            if let Some(finalizer) = finalizer {
                self.finally_stack.push(finalizer.to_vec());
            }
            self.compile_statements_in_scope(&handler.body)?;
            if finalizer.is_some() {
                self.finally_stack.pop();
            }
            if size > 0 {
                self.emit(Op::PopEnv);
                self.local_frames.pop();
            }
        } else if let Some(finalizer) = finalizer {
            // An exception is on the stack. Run finally, then rethrow it.
            self.compile_statements_in_scope(finalizer)?;
            self.emit(Op::Throw);
        }

        if handler.is_some() {
            if let Some(finalizer) = finalizer {
                let finally_pc = self.position();
                self.patch_jump(normal_jump, finally_pc);
                self.compile_statements_in_scope(finalizer)?;
            } else {
                let end = self.position();
                self.patch_jump(normal_jump, end);
            }
        } else {
            let finally_pc = self.position();
            self.patch_jump(normal_jump, finally_pc);
            self.compile_statements_in_scope(finalizer.unwrap_or(&[]))?;
        }
        Ok(())
    }

    fn compile_statements_in_scope(&mut self, statements: &[Stmt]) -> Result<usize, ParseError> {
        self.compile_block(statements)?;
        Ok(self.position())
    }

    fn compile_switch(
        &mut self,
        discriminant: &Expr,
        cases: &[SwitchCase],
    ) -> Result<(), ParseError> {
        let (depth, slot) = self.alloc_temp();
        self.compile_expr(discriminant)?;
        self.emit(Op::SetVar { depth, slot });
        let mut body_jumps = Vec::new();
        let mut default_index = None;
        for (index, case) in cases.iter().enumerate() {
            if let Some(test) = &case.test {
                self.emit(Op::GetVar { depth, slot });
                self.compile_expr(test)?;
                self.emit(Op::Eq);
                let skip = self.emit_jump(Op::JumpIfFalse(0));
                let jump = self.emit_jump(Op::Jump(0));
                body_jumps.push(Some(jump));
                let next = self.position();
                self.patch_jump(skip, next);
            } else {
                default_index = Some(index);
                body_jumps.push(None);
            }
        }
        let fallback = self.emit_jump(Op::Jump(0));
        let break_context = BreakContext {
            label: None,
            env_depth: self.local_frames.len(),
            stack_cleanup: 0,
            try_depth: self.try_depth,
            jumps: Vec::new(),
        };
        self.breakables.push(break_context);
        let mut body_positions = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            body_positions.push(self.position());
            if let Some(jump) = body_jumps[index] {
                self.patch_jump(jump, self.position());
            }
            for statement in &case.body {
                self.compile_statement(statement)?;
            }
        }
        let end = self.position();
        if let Some(index) = default_index {
            self.patch_jump(fallback, body_positions[index]);
        } else {
            self.patch_jump(fallback, end);
        }
        let break_context = self.breakables.pop().unwrap();
        for jump in break_context.jumps {
            self.patch_jump(jump, end);
        }
        Ok(())
    }

    fn alloc_temp(&mut self) -> (u16, u16) {
        let name = format!("\0temp{}", self.temp_counter);
        self.temp_counter += 1;
        let slot = self.local_frames[0].declare(&name);
        let depth = (self.local_frames.len() - 1) as u16;
        (depth, slot)
    }

    fn compile_class(&mut self, class: &ClassDecl) -> Result<(), ParseError> {
        if class.super_class.is_some() {
            // The prototype chain and parent constructor are wired at runtime.
        }
        let name = class.name.clone().unwrap_or_else(|| "<anonymous>".into());
        let mut constructor = None;
        let mut methods = Vec::new();
        let mut instance_fields = Vec::new();
        let mut static_fields = Vec::new();
        for element in &class.elements {
            if matches!(element.kind, ClassElementKind::Field) {
                let field_name = static_property_name(&element.key)
                    .ok_or_else(|| self.error("Computed class fields are not supported"))?;
                if element.is_static {
                    static_fields.push((field_name, element.value.clone()));
                } else {
                    instance_fields.push((field_name, element.value.clone()));
                }
                continue;
            }
            let Some(function) = &element.function else {
                continue;
            };
            let method_name = match &element.key {
                PropertyName::Ident(value) | PropertyName::String(value) => value.clone(),
                PropertyName::Number(value) => value.to_string(),
                PropertyName::Computed(_) => {
                    return Err(self.error("Computed class method names are not supported"));
                }
            };
            let is_constructor = !element.is_static && element.kind_is_constructor(&method_name);
            let compiled =
                self.compile_function(function, is_constructor, class.super_class.is_some())?;
            if is_constructor {
                constructor = Some(compiled);
            } else {
                methods.push(ClassMethod {
                    name: Rc::from(method_name),
                    function: compiled,
                    is_static: element.is_static,
                    is_getter: matches!(element.kind, ClassElementKind::Getter),
                    is_setter: matches!(element.kind, ClassElementKind::Setter),
                });
            }
        }
        if constructor.is_none() {
            let mut code = Vec::new();
            if class.super_class.is_some() {
                code.push(Op::SuperCall(0));
            }
            code.push(Op::Undefined);
            code.push(Op::Return);
            constructor = Some(Rc::new(FunctionDef {
                name: Rc::from(name.as_str()),
                positions: vec![class.span.start as u32; code.len()],
                code,
                constants: Vec::new(),
                env_size: 0,
                length: 0,
                is_arrow: false,
                is_async: false,
                strict: true,
                source: self.source_name.clone(),
            }));
        }
        let instance_init = self.compile_field_initializer(&instance_fields)?;
        let static_init = self.compile_field_initializer(&static_fields)?;
        let def = Rc::new(ClassDef {
            name: Rc::from(name.as_str()),
            constructor,
            methods,
            instance_init,
            static_init,
            has_super: class.super_class.is_some(),
        });
        if let Some(parent) = &class.super_class {
            self.compile_expr(parent)?;
        } else {
            self.emit(Op::Undefined);
        }
        let index = self.add_constant(Constant::Class(def));
        self.emit(Op::CreateClass(index));
        Ok(())
    }

    fn compile_field_initializer(
        &mut self,
        fields: &[(String, Option<Expr>)],
    ) -> Result<Option<Rc<FunctionDef>>, ParseError> {
        if fields.is_empty() {
            return Ok(None);
        }
        let mut child = Compiler::new(false, self.capture_frames(), self.source_name.clone());
        child.strict = self.strict;
        for (name, initializer) in fields {
            child.emit(Op::This);
            child.push_string(name);
            if let Some(initializer) = initializer {
                child.compile_expr(initializer)?;
            } else {
                child.emit(Op::Undefined);
            }
            child.emit(Op::SetProp);
            child.emit(Op::Pop);
        }
        child.emit(Op::ReturnUndefined);
        Ok(Some(Rc::new(child.finish(
            "<class_fields>",
            0,
            false,
            false,
            true,
        ))))
    }

    /// QuickJS expression lowering from `js_parse_expr` and the operator
    /// specific parsers (quickjs.c:24732).
    fn compile_expr(&mut self, expression: &Expr) -> Result<(), ParseError> {
        self.span = expression.span;
        match &expression.kind {
            ExprKind::Literal(literal) => self.compile_literal(literal),
            ExprKind::Ident(name) => {
                if let Some(reference) = self.resolve(name) {
                    self.emit(Op::GetVar {
                        depth: reference.depth,
                        slot: reference.slot,
                    });
                } else {
                    let name = self.string_constant(name);
                    self.emit(Op::GetGlobal(name));
                }
            }
            ExprKind::This => self.emit(Op::This),
            ExprKind::Array(elements) => self.compile_array_literal(elements)?,
            ExprKind::Object(properties) => self.compile_object_literal(properties)?,
            ExprKind::Function(function) => {
                let def = self.compile_function(function, false, false)?;
                let constant = self.add_constant(Constant::Function(def));
                self.emit(Op::Closure(constant));
            }
            ExprKind::Arrow(function) => {
                let def = self.compile_function(function, false, false)?;
                let constant = self.add_constant(Constant::Function(def));
                self.emit(Op::Closure(constant));
            }
            ExprKind::Class(class) => self.compile_class(class)?,
            ExprKind::Unary { op, argument } => self.compile_unary(*op, argument)?,
            ExprKind::Update {
                op,
                prefix,
                argument,
            } => self.compile_update(*op, *prefix, argument)?,
            ExprKind::Binary { op, left, right } => {
                self.compile_expr(left)?;
                self.compile_expr(right)?;
                self.emit(binary_op(*op));
            }
            ExprKind::Logical { op, left, right } => {
                self.compile_expr(left)?;
                let jump = match op {
                    LogicalOp::And => {
                        self.emit(Op::Dup);
                        self.emit_jump(Op::JumpIfFalse(0))
                    }
                    LogicalOp::Or => {
                        self.emit(Op::Dup);
                        self.emit_jump(Op::JumpIfTrue(0))
                    }
                    LogicalOp::Nullish => {
                        self.emit(Op::Dup);
                        self.emit_jump(Op::JumpIfNullish(0))
                    }
                };
                self.emit(Op::Pop);
                self.compile_expr(right)?;
                let end = self.position();
                self.patch_jump(jump, end);
            }
            ExprKind::Assign { op, target, value } => {
                self.compile_assignment(*op, target, value)?;
            }
            ExprKind::Conditional {
                test,
                consequent,
                alternate,
            } => {
                self.compile_expr(test)?;
                let else_jump = self.emit_jump(Op::JumpIfFalse(0));
                self.compile_expr(consequent)?;
                let end_jump = self.emit_jump(Op::Jump(0));
                let else_target = self.position();
                self.patch_jump(else_jump, else_target);
                self.compile_expr(alternate)?;
                let end = self.position();
                self.patch_jump(end_jump, end);
            }
            ExprKind::Sequence(expressions) => {
                for (index, expression) in expressions.iter().enumerate() {
                    self.compile_expr(expression)?;
                    if index + 1 != expressions.len() {
                        self.emit(Op::Pop);
                    }
                }
            }
            ExprKind::Member {
                object,
                property,
                optional,
            } => {
                if matches!(&object.kind, ExprKind::Ident(name) if name == "super")
                    && self.super_allowed
                {
                    self.compile_member_key(property)?;
                    self.emit(Op::SuperGetProp);
                } else if *optional {
                    self.compile_optional_member(object, property)?;
                } else {
                    self.compile_expr(object)?;
                    self.compile_member_key(property)?;
                    self.emit(Op::GetProp);
                }
            }
            ExprKind::Call {
                callee,
                arguments,
                optional,
            } => {
                self.compile_call(callee, arguments, false, *optional)?;
            }
            ExprKind::New { callee, arguments } => {
                self.compile_call(callee, arguments, true, false)?;
            }
            ExprKind::Template {
                quasis,
                expressions,
            } => {
                self.push_string(quasis.first().map(String::as_str).unwrap_or(""));
                for (index, expression) in expressions.iter().enumerate() {
                    self.compile_expr(expression)?;
                    self.emit(Op::Add);
                    if let Some(quasi) = quasis.get(index + 1) {
                        self.push_string(quasi);
                        self.emit(Op::Add);
                    }
                }
            }
            ExprKind::TaggedTemplate {
                tag,
                quasis,
                expressions,
            } => {
                self.compile_expr(tag)?;
                self.emit(Op::NewArray(0));
                for quasi in quasis {
                    self.push_string(quasi);
                    self.emit(Op::ArrayPush);
                }
                for expression in expressions {
                    self.compile_expr(expression)?;
                }
                self.emit(Op::Call((expressions.len() + 1) as u16));
            }
            ExprKind::Spread(expression) => {
                self.compile_expr(expression)?;
            }
            ExprKind::Await(expression) => {
                self.compile_expr(expression)?;
                self.emit(Op::Await);
            }
            ExprKind::Yield(expression) => {
                if let Some(expression) = expression {
                    self.compile_expr(expression)?;
                } else {
                    self.emit(Op::Undefined);
                }
            }
        }
        Ok(())
    }

    fn compile_literal(&mut self, literal: &Literal) {
        match literal {
            Literal::Undefined => self.emit(Op::Undefined),
            Literal::Null => self.emit(Op::Null),
            Literal::Bool(true) => self.emit(Op::True),
            Literal::Bool(false) => self.emit(Op::False),
            Literal::Number(value) => {
                let index = self.add_constant(Constant::Number(*value));
                self.emit(Op::PushConst(index));
            }
            Literal::String(value) => {
                let index = self.add_constant(Constant::String(Rc::from(value.as_str())));
                self.emit(Op::PushConst(index));
            }
            Literal::Regex { source, flags } => {
                let index = self.add_constant(Constant::Regex {
                    source: Rc::from(source.as_str()),
                    flags: Rc::from(flags.as_str()),
                });
                self.emit(Op::PushConst(index));
            }
        }
    }

    fn compile_array_literal(&mut self, elements: &[ArrayElement]) -> Result<(), ParseError> {
        self.emit(Op::NewArray(0));
        for element in elements {
            match element {
                ArrayElement::Hole => {
                    self.emit(Op::Undefined);
                    self.emit(Op::ArrayPush);
                }
                ArrayElement::Expr(expression) => {
                    self.compile_expr(expression)?;
                    self.emit(Op::ArrayPush);
                }
                ArrayElement::Spread(expression) => {
                    self.compile_expr(expression)?;
                    self.emit(Op::ArraySpread);
                }
            }
        }
        Ok(())
    }

    fn compile_object_literal(&mut self, properties: &[ObjectProperty]) -> Result<(), ParseError> {
        self.emit(Op::NewObject);
        for property in properties {
            match property {
                ObjectProperty::Spread(expression) => {
                    self.compile_expr(expression)?;
                    self.emit(Op::ObjectSpread);
                }
                ObjectProperty::Shorthand(name) => {
                    self.emit(Op::Dup);
                    self.compile_key_name(&PropertyName::Ident(name.clone()))?;
                    if let Some(reference) = self.resolve(name) {
                        self.emit(Op::GetVar {
                            depth: reference.depth,
                            slot: reference.slot,
                        });
                    } else {
                        let name = self.string_constant(name);
                        self.emit(Op::GetGlobal(name));
                    }
                    self.emit(Op::SetPropKeep);
                }
                ObjectProperty::KeyValue { key, value } => {
                    self.emit(Op::Dup);
                    self.compile_key_name(key)?;
                    self.compile_expr(value)?;
                    self.emit(Op::SetPropKeep);
                }
                ObjectProperty::Method {
                    key,
                    function,
                    kind,
                } => {
                    self.emit(Op::Dup);
                    self.compile_key_name(key)?;
                    let def = self.compile_function(function, false, false)?;
                    let constant = self.add_constant(Constant::Function(def));
                    self.emit(Op::Closure(constant));
                    match kind {
                        MethodKind::Method => self.emit(Op::SetPropKeep),
                        MethodKind::Getter => self.emit(Op::DefineAccessor { is_getter: true }),
                        MethodKind::Setter => self.emit(Op::DefineAccessor { is_getter: false }),
                    }
                }
            }
        }
        Ok(())
    }

    fn compile_key_name(&mut self, key: &PropertyName) -> Result<(), ParseError> {
        match key {
            PropertyName::Ident(value) | PropertyName::String(value) => {
                self.push_string(value);
            }
            PropertyName::Number(value) => {
                self.push_string(&format_number(*value));
            }
            PropertyName::Computed(expression) => {
                self.compile_expr(expression)?;
            }
        }
        Ok(())
    }

    fn compile_member_key(&mut self, property: &MemberProperty) -> Result<(), ParseError> {
        match property {
            MemberProperty::Ident(name) => {
                self.push_string(name);
            }
            MemberProperty::Computed(expression) => {
                self.compile_expr(expression)?;
            }
        }
        Ok(())
    }

    fn compile_optional_member(
        &mut self,
        object: &Expr,
        property: &MemberProperty,
    ) -> Result<(), ParseError> {
        self.compile_expr(object)?;
        self.emit(Op::Dup);
        let nullish_jump = self.emit_jump(Op::JumpIfNullishKeep(0));
        self.compile_member_key(property)?;
        self.emit(Op::GetProp);
        let end_jump = self.emit_jump(Op::Jump(0));
        let nullish = self.position();
        self.patch_jump(nullish_jump, nullish);
        self.emit(Op::Pop);
        self.emit(Op::Pop);
        self.emit(Op::Undefined);
        let end = self.position();
        self.patch_jump(end_jump, end);
        Ok(())
    }

    fn compile_optional_member_call(
        &mut self,
        object: &Expr,
        property: &MemberProperty,
        arguments: &[Argument],
        has_spread: bool,
    ) -> Result<(), ParseError> {
        self.compile_expr(object)?;
        self.emit(Op::Dup);
        let nullish_jump = self.emit_jump(Op::JumpIfNullishKeep(0));
        self.emit(Op::Dup);
        self.compile_member_key(property)?;
        self.emit(Op::GetProp);
        if has_spread {
            self.compile_arguments_array(arguments)?;
            self.emit(Op::CallMethodSpread);
        } else {
            for argument in arguments {
                if let Argument::Expr(expression) = argument {
                    self.compile_expr(expression)?;
                }
            }
            self.emit(Op::CallMethod(arguments.len() as u16));
        }
        let end_jump = self.emit_jump(Op::Jump(0));
        let nullish = self.position();
        self.patch_jump(nullish_jump, nullish);
        self.emit(Op::Pop);
        self.emit(Op::Pop);
        self.emit(Op::Undefined);
        let end = self.position();
        self.patch_jump(end_jump, end);
        Ok(())
    }

    fn compile_optional_call(
        &mut self,
        callee: &Expr,
        arguments: &[Argument],
        has_spread: bool,
    ) -> Result<(), ParseError> {
        if let ExprKind::Member {
            object,
            property,
            optional: false,
        } = &callee.kind
        {
            self.compile_expr(object)?;
            self.emit(Op::Dup);
            self.compile_member_key(property)?;
            self.emit(Op::GetProp);
            self.emit(Op::Dup);
            let nullish_jump = self.emit_jump(Op::JumpIfNullishKeep(0));
            if has_spread {
                self.compile_arguments_array(arguments)?;
                self.emit(Op::CallMethodSpread);
            } else {
                for argument in arguments {
                    if let Argument::Expr(expression) = argument {
                        self.compile_expr(expression)?;
                    }
                }
                self.emit(Op::CallMethod(arguments.len() as u16));
            }
            let end_jump = self.emit_jump(Op::Jump(0));
            let nullish = self.position();
            self.patch_jump(nullish_jump, nullish);
            self.emit(Op::Pop);
            self.emit(Op::Pop);
            self.emit(Op::Pop);
            self.emit(Op::Undefined);
            let end = self.position();
            self.patch_jump(end_jump, end);
            return Ok(());
        }
        self.compile_expr(callee)?;
        self.emit(Op::Dup);
        let nullish_jump = self.emit_jump(Op::JumpIfNullishKeep(0));
        if has_spread {
            self.compile_arguments_array(arguments)?;
            self.emit(Op::CallSpread);
        } else {
            for argument in arguments {
                if let Argument::Expr(expression) = argument {
                    self.compile_expr(expression)?;
                }
            }
            self.emit(Op::Call(arguments.len() as u16));
        }
        let end_jump = self.emit_jump(Op::Jump(0));
        let nullish = self.position();
        self.patch_jump(nullish_jump, nullish);
        self.emit(Op::Pop);
        self.emit(Op::Pop);
        self.emit(Op::Undefined);
        let end = self.position();
        self.patch_jump(end_jump, end);
        Ok(())
    }

    fn compile_call(
        &mut self,
        callee: &Expr,
        arguments: &[Argument],
        is_new: bool,
        optional: bool,
    ) -> Result<(), ParseError> {
        let has_spread = arguments
            .iter()
            .any(|argument| matches!(argument, Argument::Spread(_)));
        if optional {
            if let ExprKind::Member {
                object,
                property,
                optional: true,
            } = &callee.kind
            {
                return self.compile_optional_member_call(object, property, arguments, has_spread);
            }
            return self.compile_optional_call(callee, arguments, has_spread);
        }
        if !is_new
            && self.super_allowed
            && let ExprKind::Ident(name) = &callee.kind
            && name == "super"
        {
            self.compile_arguments(arguments)?;
            self.emit(Op::SuperCall(arguments.len() as u16));
            return Ok(());
        }
        if !is_new
            && let ExprKind::Member {
                object,
                property,
                optional: true,
            } = &callee.kind
        {
            return self.compile_optional_member_call(object, property, arguments, has_spread);
        }
        if !is_new
            && let ExprKind::Member {
                object, property, ..
            } = &callee.kind
        {
            if matches!(&object.kind, ExprKind::Ident(name) if name == "super")
                && self.super_allowed
            {
                self.compile_member_key(property)?;
                if has_spread {
                    self.compile_arguments_array(arguments)?;
                    self.emit(Op::CallSuperMethodSpread);
                } else {
                    for argument in arguments {
                        if let Argument::Expr(expression) = argument {
                            self.compile_expr(expression)?;
                        }
                    }
                    self.emit(Op::CallSuperMethod(arguments.len() as u16));
                }
                return Ok(());
            }
            self.compile_expr(object)?;
            self.emit(Op::Dup);
            self.compile_member_key(property)?;
            self.emit(Op::GetProp);
            if has_spread {
                self.compile_arguments_array(arguments)?;
                self.emit(Op::CallMethodSpread);
            } else {
                for argument in arguments {
                    if let Argument::Expr(expression) = argument {
                        self.compile_expr(expression)?;
                    }
                }
                self.emit(Op::CallMethod(arguments.len() as u16));
            }
            return Ok(());
        }
        self.compile_expr(callee)?;
        if has_spread {
            self.compile_arguments_array(arguments)?;
            self.emit(if is_new {
                Op::NewSpread
            } else {
                Op::CallSpread
            });
        } else {
            for argument in arguments {
                if let Argument::Expr(expression) = argument {
                    self.compile_expr(expression)?;
                }
            }
            self.emit(if is_new {
                Op::New(arguments.len() as u16)
            } else {
                Op::Call(arguments.len() as u16)
            });
        }
        Ok(())
    }

    fn compile_arguments(&mut self, arguments: &[Argument]) -> Result<(), ParseError> {
        for argument in arguments {
            match argument {
                Argument::Expr(expression) => self.compile_expr(expression)?,
                Argument::Spread(_) => {
                    return Err(self.error("Spread argument outside call"));
                }
            }
        }
        Ok(())
    }

    fn compile_arguments_array(&mut self, arguments: &[Argument]) -> Result<(), ParseError> {
        self.emit(Op::NewArray(0));
        for argument in arguments {
            match argument {
                Argument::Expr(expression) => {
                    self.compile_expr(expression)?;
                    self.emit(Op::ArrayPush);
                }
                Argument::Spread(expression) => {
                    self.compile_expr(expression)?;
                    self.emit(Op::ArraySpread);
                }
            }
        }
        Ok(())
    }

    fn compile_unary(&mut self, op: UnaryOp, argument: &Expr) -> Result<(), ParseError> {
        match op {
            UnaryOp::Plus => {
                self.compile_expr(argument)?;
                self.emit(Op::Pos);
            }
            UnaryOp::Minus => {
                self.compile_expr(argument)?;
                self.emit(Op::Neg);
            }
            UnaryOp::Not => {
                self.compile_expr(argument)?;
                self.emit(Op::Not);
            }
            UnaryOp::BitNot => {
                self.compile_expr(argument)?;
                self.emit(Op::BitNot);
            }
            UnaryOp::TypeOf => {
                if let ExprKind::Ident(name) = &argument.kind
                    && self.resolve(name).is_none()
                {
                    let name = self.string_constant(name);
                    self.emit(Op::TypeOfGlobal(name));
                    return Ok(());
                }
                self.compile_expr(argument)?;
                self.emit(Op::TypeOf);
            }
            UnaryOp::Void => {
                self.compile_expr(argument)?;
                self.emit(Op::Pop);
                self.emit(Op::Undefined);
            }
            UnaryOp::Delete => match &argument.kind {
                ExprKind::Member {
                    object, property, ..
                } => {
                    self.compile_expr(object)?;
                    self.compile_member_key(property)?;
                    self.emit(Op::DeleteProp);
                }
                ExprKind::Ident(name) => {
                    if let Some(reference) = self.resolve(name) {
                        let _ = reference;
                    } else {
                        let name = self.string_constant(name);
                        self.emit(Op::DeleteGlobal(name));
                        return Ok(());
                    }
                    self.emit(Op::False);
                }
                _ => {
                    self.compile_expr(argument)?;
                    self.emit(Op::Pop);
                    self.emit(Op::True);
                }
            },
        }
        Ok(())
    }

    fn compile_update(
        &mut self,
        op: UpdateOp,
        prefix: bool,
        argument: &Expr,
    ) -> Result<(), ParseError> {
        let delta = if matches!(op, UpdateOp::Increment) {
            1.0
        } else {
            -1.0
        };
        match &argument.kind {
            ExprKind::Ident(name) => {
                if let Some(reference) = self.resolve(name) {
                    self.emit(if delta > 0.0 {
                        Op::IncVar {
                            depth: reference.depth,
                            slot: reference.slot,
                            prefix,
                        }
                    } else {
                        Op::DecVar {
                            depth: reference.depth,
                            slot: reference.slot,
                            prefix,
                        }
                    });
                } else if delta > 0.0 {
                    let name = self.string_constant(name);
                    self.emit(Op::IncGlobal(name, prefix));
                } else {
                    let name = self.string_constant(name);
                    self.emit(Op::DecGlobal(name, prefix));
                }
            }
            ExprKind::Member {
                object, property, ..
            } => {
                self.compile_expr(object)?;
                self.compile_member_key(property)?;
                self.emit(Op::UpdateProp { delta, prefix });
            }
            _ => return Err(self.error("Invalid update target")),
        }
        Ok(())
    }

    fn compile_assignment(
        &mut self,
        op: AssignOp,
        target: &Expr,
        value: &Expr,
    ) -> Result<(), ParseError> {
        match &target.kind {
            ExprKind::Ident(name) => {
                let reference = self.resolve(name);
                if op == AssignOp::Assign {
                    self.compile_expr(value)?;
                    self.emit(Op::Dup);
                    self.store_ident(name, BindingMode::Auto);
                    return Ok(());
                }
                if matches!(op, AssignOp::And | AssignOp::Or | AssignOp::Nullish) {
                    self.emit_ident_load(name);
                    self.emit(Op::Dup);
                    let jump = match op {
                        AssignOp::And => self.emit_jump(Op::JumpIfFalse(0)),
                        AssignOp::Or => self.emit_jump(Op::JumpIfTrue(0)),
                        AssignOp::Nullish => self.emit_jump(Op::JumpIfNotUndefined(0)),
                        _ => unreachable!(),
                    };
                    self.emit(Op::Pop);
                    self.compile_expr(value)?;
                    self.emit(Op::Dup);
                    self.store_ident(name, BindingMode::Auto);
                    let end = self.position();
                    self.patch_jump(jump, end);
                    return Ok(());
                }
                self.emit_ident_load(name);
                self.compile_expr(value)?;
                self.emit(assign_binary_op(op));
                self.emit(Op::Dup);
                self.store_ident(name, BindingMode::Auto);
                let _ = reference;
            }
            ExprKind::Member {
                object, property, ..
            } => {
                self.compile_expr(object)?;
                self.compile_member_key(property)?;
                if op == AssignOp::Assign {
                    self.compile_expr(value)?;
                    self.emit(Op::SetProp);
                    return Ok(());
                }
                self.emit(Op::Dup2);
                self.emit(Op::GetProp);
                self.compile_expr(value)?;
                self.emit(assign_binary_op(op));
                self.emit(Op::SetProp);
            }
            ExprKind::Array(_) | ExprKind::Object(_) => {
                if op != AssignOp::Assign {
                    return Err(self.error("Invalid compound destructuring assignment"));
                }
                self.compile_expr(value)?;
                self.emit(Op::Dup);
                let pattern = expression_to_pattern(target)
                    .ok_or_else(|| self.error("Invalid destructuring target"))?;
                self.bind_pattern(&pattern, BindingMode::Auto)?;
            }
            _ => return Err(self.error("Invalid assignment target")),
        }
        Ok(())
    }

    fn compile_assignment_target_store(&mut self, target: &Expr) -> Result<(), ParseError> {
        match &target.kind {
            ExprKind::Ident(name) => {
                self.store_ident(name, BindingMode::Auto);
            }
            ExprKind::Member {
                object, property, ..
            } => {
                let (depth, slot) = self.alloc_temp();
                self.emit(Op::SetVar { depth, slot });
                self.compile_expr(object)?;
                self.compile_member_key(property)?;
                self.emit(Op::GetVar { depth, slot });
                self.emit(Op::SetProp);
                self.emit(Op::Pop);
            }
            ExprKind::Array(_) | ExprKind::Object(_) => {
                let pattern = expression_to_pattern(target)
                    .ok_or_else(|| self.error("Invalid destructuring target"))?;
                self.bind_pattern(&pattern, BindingMode::Auto)?;
            }
            _ => return Err(self.error("Invalid assignment target")),
        }
        Ok(())
    }

    fn emit_ident_load(&mut self, name: &str) {
        if let Some(reference) = self.resolve(name) {
            self.emit(Op::GetVar {
                depth: reference.depth,
                slot: reference.slot,
            });
        } else {
            let name = self.string_constant(name);
            self.emit(Op::GetGlobal(name));
        }
    }

    fn store_ident(&mut self, name: &str, mode: BindingMode) {
        if let Some(reference) = self.resolve(name) {
            self.emit(Op::SetVar {
                depth: reference.depth,
                slot: reference.slot,
            });
        } else {
            let _ = mode;
            let name = self.string_constant(name);
            self.emit(Op::SetGlobal(name));
        }
    }

    fn bind_pattern(&mut self, pattern: &Pattern, mode: BindingMode) -> Result<(), ParseError> {
        match pattern {
            Pattern::Ident(name) => {
                if matches!(mode, BindingMode::Global) {
                    let name = self.string_constant(name);
                    self.emit(Op::SetGlobal(name));
                } else {
                    self.store_ident(name, mode);
                }
                Ok(())
            }
            Pattern::Assign { target, default } => {
                self.emit(Op::Dup);
                let jump = self.emit_jump(Op::JumpIfNotUndefined(0));
                self.emit(Op::Pop);
                self.compile_expr(default)?;
                let end = self.position();
                self.patch_jump(jump, end);
                self.bind_pattern(target, mode)
            }
            Pattern::Array(elements) => {
                for (index, element) in elements.iter().enumerate() {
                    let Some(element) = element else {
                        continue;
                    };
                    if let Pattern::Rest(inner) = element {
                        self.emit(Op::ArrayRest(index as u16));
                        self.bind_pattern(inner, mode)?;
                        continue;
                    }
                    self.emit(Op::Dup);
                    self.push_string(&index.to_string());
                    self.emit(Op::GetProp);
                    self.bind_pattern(element, mode)?;
                }
                self.emit(Op::Pop);
                Ok(())
            }
            Pattern::Object(properties) => {
                let mut excluded: Vec<Rc<str>> = Vec::new();
                for property in properties {
                    if let PropertyName::Ident(name) = &property.key
                        && name == "..."
                    {
                        let excluded: Rc<[Rc<str>]> = Rc::from(excluded.clone());
                        let index = self.add_constant(Constant::Strings(excluded));
                        self.emit(Op::ObjectRest(index));
                        self.bind_pattern(&property.value, mode)?;
                        continue;
                    }
                    if let Some(name) = static_property_name(&property.key) {
                        excluded.push(Rc::from(name));
                    }
                    self.emit(Op::Dup);
                    self.compile_key_name(&property.key)?;
                    self.emit(Op::GetProp);
                    self.bind_pattern(&property.value, mode)?;
                }
                self.emit(Op::Pop);
                Ok(())
            }
            Pattern::Rest(inner) => self.bind_pattern(inner, mode),
        }
    }
}

impl ClassElement {
    fn kind_is_constructor(&self, name: &str) -> bool {
        name == "constructor"
    }
}

fn pattern_names(pattern: &Pattern) -> Vec<String> {
    let mut names = Vec::new();
    collect_pattern_names(pattern, &mut names);
    names
}

fn collect_pattern_names(pattern: &Pattern, names: &mut Vec<String>) {
    match pattern {
        Pattern::Ident(name) => names.push(name.clone()),
        Pattern::Assign { target, .. } => collect_pattern_names(target, names),
        Pattern::Array(elements) => {
            for element in elements.iter().flatten() {
                collect_pattern_names(element, names);
            }
        }
        Pattern::Object(properties) => {
            for property in properties {
                collect_pattern_names(&property.value, names);
            }
        }
        Pattern::Rest(inner) => collect_pattern_names(inner, names),
    }
}

fn collect_var_names(statements: &[Stmt]) -> Vec<String> {
    let mut names = Vec::new();
    collect_var_names_inner(statements, &mut names);
    names
}

fn collect_var_names_inner(statements: &[Stmt], names: &mut Vec<String>) {
    for statement in statements {
        match &statement.kind {
            StmtKind::Var(declaration) => {
                for declarator in &declaration.declarations {
                    if declaration.kind == VarKind::Var {
                        collect_pattern_names(&declarator.pattern, names);
                    }
                }
            }
            StmtKind::Function(function) => {
                if let Some(name) = &function.name {
                    names.push(name.clone());
                }
            }
            StmtKind::Block(body) => collect_var_names_inner(body, names),
            StmtKind::If {
                consequent,
                alternate,
                ..
            } => {
                collect_var_names_inner(std::slice::from_ref(consequent.as_ref()), names);
                if let Some(alternate) = alternate {
                    collect_var_names_inner(std::slice::from_ref(alternate.as_ref()), names);
                }
            }
            StmtKind::While { body, .. }
            | StmtKind::DoWhile { body, .. }
            | StmtKind::Labeled { body, .. } => {
                collect_var_names_inner(std::slice::from_ref(body.as_ref()), names);
            }
            StmtKind::For { init, body, .. } => {
                if let Some(ForInit::Var(declaration)) = init
                    && declaration.kind == VarKind::Var
                {
                    for declarator in &declaration.declarations {
                        collect_pattern_names(&declarator.pattern, names);
                    }
                }
                collect_var_names_inner(std::slice::from_ref(body.as_ref()), names);
            }
            StmtKind::ForIn { left, body, .. } | StmtKind::ForOf { left, body, .. } => {
                if let ForInit::Var(declaration) = left
                    && declaration.kind == VarKind::Var
                {
                    for declarator in &declaration.declarations {
                        collect_pattern_names(&declarator.pattern, names);
                    }
                }
                collect_var_names_inner(std::slice::from_ref(body.as_ref()), names);
            }
            StmtKind::Try {
                block,
                handler,
                finalizer,
            } => {
                collect_var_names_inner(block, names);
                if let Some(handler) = handler {
                    collect_var_names_inner(&handler.body, names);
                }
                if let Some(finalizer) = finalizer {
                    collect_var_names_inner(finalizer, names);
                }
            }
            StmtKind::Switch { cases, .. } => {
                for case in cases {
                    collect_var_names_inner(&case.body, names);
                }
            }
            _ => {}
        }
    }
}

fn collect_direct_bindings(statements: &[Stmt]) -> Vec<String> {
    let mut names = Vec::new();
    for statement in statements {
        match &statement.kind {
            StmtKind::Var(declaration) if declaration.kind != VarKind::Var => {
                for declarator in &declaration.declarations {
                    collect_pattern_names(&declarator.pattern, &mut names);
                }
            }
            StmtKind::Function(function) => {
                if let Some(name) = &function.name {
                    names.push(name.clone());
                }
            }
            StmtKind::Class(class) => {
                if let Some(name) = &class.name {
                    names.push(name.clone());
                }
            }
            _ => {}
        }
    }
    names
}

fn expression_to_pattern(expression: &Expr) -> Option<Pattern> {
    match &expression.kind {
        ExprKind::Ident(name) => Some(Pattern::Ident(name.clone())),
        ExprKind::Array(elements) => {
            let mut patterns = Vec::new();
            for element in elements {
                match element {
                    ArrayElement::Hole => patterns.push(None),
                    ArrayElement::Expr(expression) => {
                        patterns.push(Some(expression_to_pattern(expression)?));
                    }
                    ArrayElement::Spread(expression) => patterns.push(Some(Pattern::Rest(
                        Box::new(expression_to_pattern(expression)?),
                    ))),
                }
            }
            Some(Pattern::Array(patterns))
        }
        ExprKind::Object(properties) => {
            let mut patterns = Vec::new();
            for property in properties {
                match property {
                    ObjectProperty::Shorthand(name) => patterns.push(ObjectPatternProperty {
                        key: PropertyName::Ident(name.clone()),
                        value: Pattern::Ident(name.clone()),
                    }),
                    ObjectProperty::KeyValue { key, value } => {
                        patterns.push(ObjectPatternProperty {
                            key: key.clone(),
                            value: expression_to_pattern(value)?,
                        });
                    }
                    ObjectProperty::Spread(expression) => {
                        patterns.push(ObjectPatternProperty {
                            key: PropertyName::Ident("...".into()),
                            value: expression_to_pattern(expression)?,
                        });
                    }
                    ObjectProperty::Method { .. } => return None,
                }
            }
            Some(Pattern::Object(patterns))
        }
        ExprKind::Assign {
            op: AssignOp::Assign,
            target,
            value,
        } => Some(Pattern::Assign {
            target: Box::new(expression_to_pattern(target)?),
            default: value.clone(),
        }),
        _ => None,
    }
}

fn binary_op(op: BinaryOp) -> Op {
    match op {
        BinaryOp::Add => Op::Add,
        BinaryOp::Sub => Op::Sub,
        BinaryOp::Mul => Op::Mul,
        BinaryOp::Div => Op::Div,
        BinaryOp::Mod => Op::Mod,
        BinaryOp::Exp => Op::Exp,
        BinaryOp::BitAnd => Op::BitAnd,
        BinaryOp::BitOr => Op::BitOr,
        BinaryOp::BitXor => Op::BitXor,
        BinaryOp::Shl => Op::Shl,
        BinaryOp::Shr => Op::Shr,
        BinaryOp::UShr => Op::UShr,
        BinaryOp::Eq => Op::Eq,
        BinaryOp::Ne => Op::Ne,
        BinaryOp::StrictEq => Op::StrictEq,
        BinaryOp::StrictNe => Op::StrictNe,
        BinaryOp::Lt => Op::Lt,
        BinaryOp::Le => Op::Le,
        BinaryOp::Gt => Op::Gt,
        BinaryOp::Ge => Op::Ge,
        BinaryOp::In => Op::In,
        BinaryOp::InstanceOf => Op::InstanceOf,
    }
}

fn assign_binary_op(op: AssignOp) -> Op {
    match op {
        AssignOp::Add => Op::Add,
        AssignOp::Sub => Op::Sub,
        AssignOp::Mul => Op::Mul,
        AssignOp::Div => Op::Div,
        AssignOp::Mod => Op::Mod,
        AssignOp::Exp => Op::Exp,
        AssignOp::BitAnd => Op::BitAnd,
        AssignOp::BitOr => Op::BitOr,
        AssignOp::BitXor => Op::BitXor,
        AssignOp::Shl => Op::Shl,
        AssignOp::Shr => Op::Shr,
        AssignOp::UShr => Op::UShr,
        _ => Op::Nop,
    }
}

fn has_use_strict(statements: &[Stmt]) -> bool {
    matches!(
        statements.first().map(|statement| &statement.kind),
        Some(StmtKind::Expr(Expr {
            kind: ExprKind::Literal(Literal::String(value)),
            ..
        })) if value == "use strict"
    )
}

fn static_property_name(name: &PropertyName) -> Option<String> {
    match name {
        PropertyName::Ident(value) | PropertyName::String(value) => Some(value.clone()),
        PropertyName::Number(value) => Some(format_number(*value)),
        PropertyName::Computed(_) => None,
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{}", value as i64)
    } else {
        let mut buffer = ryu::Buffer::new();
        buffer.format(value).to_owned()
    }
}
