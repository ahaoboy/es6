//! Statement compilation: control flow, loops, try/catch and switch.
//!
//! Counterpart of js_parse_statement_or_decl (quickjs.c:29374) lowering.

use super::*;

enum LoopLimit {
    Var { depth: u16, slot: u16 },
    Const(f64),
}

struct ForNextPlan {
    depth: u16,
    slot: u16,
    delta: i8,
    limit: LoopLimit,
    op: CompareOp,
}

impl Compiler {
    /// Static imports are lowered to the host `require()` bridge:
    ///
    /// ```text
    /// GetGlobal require; PushConst "fs"; Call 1
    /// Dup; GetPropConst readFileSync; SetVar ...
    /// Pop
    /// ```
    pub(super) fn compile_import(&mut self, import: &ImportDecl) -> Result<(), ParseError> {
        self.compile_require(&import.source);

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Default(local) | ImportSpecifier::Namespace(local) => {
                    self.emit(Op::Dup);
                    self.store_ident(local, BindingMode::Auto);
                }
                ImportSpecifier::Named { imported, local } if imported == "default" => {
                    self.emit(Op::Dup);
                    self.store_ident(local, BindingMode::Auto);
                }
                ImportSpecifier::Named { imported, local } => {
                    self.emit(Op::Dup);
                    self.emit_get_prop_const(imported);
                    self.store_ident(local, BindingMode::Auto);
                }
            }
        }
        self.emit(Op::Pop);
        Ok(())
    }

    fn compile_require(&mut self, source: &str) {
        self.emit_get_global("require");
        let source = self.string_constant(source);
        self.emit(Op::PushConst(source));
        self.emit(Op::Call(1));
    }

    fn emit_module_exports(&mut self) {
        self.emit_ident_load(MODULE_EXPORTS_BINDING);
    }

    /// Pops `[exports, value]` and stores `value` under `name`.
    fn store_export(&mut self, name: &str) {
        self.emit_set_prop_const(name, false);
        self.emit(Op::Pop);
    }

    /// Record the exports of a module statement (`ModuleItem` evaluation,
    /// ES2022 16.2.1.6). The namespace is an ordinary object passed to the
    /// compiled module body, like the exports object of QuickJS' module
    /// bindings (`JSModuleDef`, quickjs.c:4381).
    pub(super) fn compile_export(&mut self, export: &ExportDecl) -> Result<(), ParseError> {
        match &export.kind {
            ExportKind::Default(expression) => {
                self.emit_module_exports();
                self.compile_expr(expression)?;
                self.store_export("default");
            }
            ExportKind::Declaration(statement) => {
                self.compile_statement(statement)?;
                for name in declaration_names(statement) {
                    self.emit_module_exports();
                    self.emit_ident_load(&name);
                    self.store_export(&name);
                }
            }
            ExportKind::Named(specifiers) => {
                for specifier in specifiers {
                    self.emit_module_exports();
                    match &export.source {
                        Some(source) => {
                            self.compile_require(source);
                            self.emit_get_prop_const(&specifier.local);
                        }
                        None => self.emit_ident_load(&specifier.local),
                    }
                    self.store_export(&specifier.exported);
                }
            }
            ExportKind::All { alias } => {
                let Some(source) = &export.source else {
                    return Err(self.error("`export *` requires a module specifier"));
                };
                match alias {
                    Some(alias) => {
                        self.emit_module_exports();
                        self.compile_require(source);
                        self.store_export(alias);
                    }
                    None => {
                        self.emit_module_exports();
                        self.compile_require(source);
                        self.emit(Op::ExportAll);
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn compile_statement(&mut self, statement: &Stmt) -> Result<(), ParseError> {
        self.span = statement.span;
        match &statement.kind {
            StmtKind::Empty | StmtKind::Debugger => {}
            StmtKind::Expr(expression) => {
                if !self.compile_discard_expr(expression)? {
                    self.compile_expr(expression)?;
                    self.emit(Op::Pop);
                }
            }
            StmtKind::Block(statements) => {
                self.compile_block(statements)?;
            }
            StmtKind::Var(declaration) => {
                self.compile_var_declaration(declaration)?;
            }
            StmtKind::Function(function) => {
                let def = self.compile_function(function, false, false, true)?;
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
            StmtKind::Import(import) => self.compile_import(import)?,
            StmtKind::Export(export) => self.compile_export(export)?,
            StmtKind::If {
                test,
                consequent,
                alternate,
            } => {
                let else_jump = self.emit_test_jump(test, false)?;
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

    pub(super) fn compile_stmt_or_block(&mut self, statement: &Stmt) -> Result<(), ParseError> {
        self.compile_statement(statement)
    }

    pub(super) fn compile_block(&mut self, statements: &[Stmt]) -> Result<(), ParseError> {
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

    pub(super) fn compile_var_declaration(
        &mut self,
        declaration: &VarDecl,
    ) -> Result<(), ParseError> {
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

    /// Emit a condition branch, fusing `local < local` and `local < constant`
    /// with the jump when the test is a comparison (QuickJS keeps them as
    /// separate opcodes, but its interpreter threads them without a stack
    /// round-trip; the fused form gets the same effect here).
    pub(super) fn emit_test_jump(
        &mut self,
        test: &Expr,
        jump_if_true: bool,
    ) -> Result<usize, ParseError> {
        self.span = test.span;
        if let ExprKind::Binary {
            op: BinaryOp::BitAnd,
            left,
            right,
        } = &test.kind
            && let ExprKind::Ident(name) = &left.kind
            && let ExprKind::Literal(Literal::Number(value)) = &right.kind
            && value.fract() == 0.0
            && *value >= i32::MIN as f64
            && *value <= i32::MAX as f64
            && let Some(reference) = self.resolve(name)
        {
            return Ok(self.emit_jump(Op::BitAndVarConstJump {
                depth: reference.depth,
                slot: reference.slot,
                value: *value as i32,
                jump_if_true,
                target: 0,
            }));
        }
        if let ExprKind::Binary { op, left, right } = &test.kind
            && let Some(op) = comparison_op(*op)
            && let ExprKind::Ident(name) = &left.kind
            && let Some(left_ref) = self.resolve(name)
        {
            if let ExprKind::Ident(right_name) = &right.kind
                && let Some(right_ref) = self.resolve(right_name)
            {
                return Ok(self.emit_jump(Op::CompareVarsJump {
                    op,
                    left_depth: left_ref.depth,
                    left_slot: left_ref.slot,
                    right_depth: right_ref.depth,
                    right_slot: right_ref.slot,
                    jump_if_true,
                    target: 0,
                }));
            }
            if let ExprKind::Literal(Literal::Number(value)) = &right.kind
                && value.fract() == 0.0
                && *value >= i32::MIN as f64
                && *value <= i32::MAX as f64
            {
                return Ok(self.emit_jump(Op::CompareVarConstJump {
                    op,
                    depth: left_ref.depth,
                    slot: left_ref.slot,
                    value: *value as i32,
                    jump_if_true,
                    target: 0,
                }));
            }
        }
        self.compile_expr(test)?;
        Ok(self.emit_jump(if jump_if_true {
            Op::JumpIfTrue(0)
        } else {
            Op::JumpIfFalse(0)
        }))
    }

    pub(super) fn compile_while(&mut self, test: &Expr, body: &Stmt) -> Result<(), ParseError> {
        let start = self.position();
        let end_jump = self.emit_test_jump(test, false)?;
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
        self.emit(Op::Jump(start as u32));
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

    pub(super) fn compile_do_while(&mut self, body: &Stmt, test: &Expr) -> Result<(), ParseError> {
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
        let test_jump = self.emit_test_jump(test, true)?;
        self.patch_jump(test_jump, start);
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

    /// Recognize `for (…; i < limit; i++)` so the increment and the loop
    /// test become one opcode. QuickJS keeps `OP_inc_loc` and the comparison
    /// separate; fusing them removes one dispatch and one local read.
    fn for_next_plan(&mut self, test: Option<&Expr>, update: Option<&Expr>) -> Option<ForNextPlan> {
        let test = test?;
        let update = update?;
        let ExprKind::Update {
            op: update_op,
            argument,
            ..
        } = &update.kind
        else {
            return None;
        };
        let ExprKind::Ident(update_name) = &argument.kind else {
            return None;
        };
        let ExprKind::Binary { op, left, right } = &test.kind else {
            return None;
        };
        let op = comparison_op(*op)?;
        let ExprKind::Ident(test_name) = &left.kind else {
            return None;
        };
        if test_name != update_name {
            return None;
        }
        let reference = self.resolve(update_name)?;
        let limit = match &right.kind {
            ExprKind::Ident(name) => {
                let reference = self.resolve(name)?;
                LoopLimit::Var {
                    depth: reference.depth,
                    slot: reference.slot,
                }
            }
            ExprKind::Literal(Literal::Number(value))
                if value.fract() == 0.0
                    && *value >= i32::MIN as f64
                    && *value <= i32::MAX as f64 =>
            {
                LoopLimit::Const(*value)
            }
            _ => return None,
        };
        Some(ForNextPlan {
            depth: reference.depth,
            slot: reference.slot,
            delta: if matches!(update_op, UpdateOp::Increment) {
                1
            } else {
                -1
            },
            limit,
            op,
        })
    }

    pub(super) fn compile_for(
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
            // QuickJS only materializes a fresh binding per iteration when a
            // closure can observe it (`resolve_variables`, quickjs.c:35111).
            // Without captures the loop variables live in the enclosing
            // scope like `var`, which removes the per-iteration
            // PushEnv/copy/PopEnv sequence entirely.
            let names: Vec<String> = declaration
                .declarations
                .iter()
                .flat_map(|declarator| pattern_names(&declarator.pattern))
                .collect();
            // Probe the test/update expressions too: a nested function there
            // could also observe the per-iteration binding.
            let capture_probe = {
                let mut probe = Vec::new();
                if let Some(test) = test {
                    probe.push(Stmt {
                        kind: StmtKind::Expr(test.clone()),
                        span: body.span,
                    });
                }
                if let Some(update) = update {
                    probe.push(Stmt {
                        kind: StmtKind::Expr(update.clone()),
                        span: body.span,
                    });
                }
                probe.push(body.clone());
                Stmt {
                    kind: StmtKind::Block(probe),
                    span: body.span,
                }
            };
            if !body_captures_names(&capture_probe, &names) {
                let frame = self.local_frames.last_mut().expect("scope frame");
                for name in &names {
                    frame.declare(name);
                }
                match init {
                    Some(ForInit::Var(declaration)) => {
                        self.compile_var_declaration(declaration)?;
                    }
                    _ => unreachable!(),
                }
                // Fall through to the shared fused loop body below.
                let start = self.position();
                let end_jump = if let Some(test) = test {
                    Some(self.emit_test_jump(test, false)?)
                } else {
                    None
                };
                let body_start = self.position();
                let next_plan = self.for_next_plan(test.as_ref(), update.as_ref());
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
                let mut jumped = false;
                if let Some(plan) = next_plan {
                    match plan.limit {
                        LoopLimit::Var { depth, slot } => self.emit(Op::ForNext {
                            depth: plan.depth,
                            slot: plan.slot,
                            delta: plan.delta,
                            limit_depth: depth,
                            limit_slot: slot,
                            op: plan.op,
                            body: body_start as u32,
                        }),
                        LoopLimit::Const(value) => self.emit(Op::ForNextConst {
                            depth: plan.depth,
                            slot: plan.slot,
                            delta: plan.delta,
                            value: value as i32,
                            op: plan.op,
                            body: body_start as u32,
                        }),
                    }
                    jumped = true;
                }
                if !jumped {
                    if let Some(update) = update {
                        self.compile_expr(update)?;
                        self.emit(Op::Pop);
                    }
                    self.emit(Op::Jump(start as u32));
                }
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
                let _ = start;
                return Ok(());
            }
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
                Some(self.emit_test_jump(test, false)?)
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
            self.emit(Op::Jump(start as u32));
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
            Some(self.emit_test_jump(test, false)?)
        } else {
            None
        };
        let body_start = self.position();
        let next_plan = self.for_next_plan(test.as_ref(), update.as_ref());
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
        let mut jumped = false;
        if let Some(plan) = next_plan {
            match plan.limit {
                LoopLimit::Var { depth, slot } => self.emit(Op::ForNext {
                    depth: plan.depth,
                    slot: plan.slot,
                    delta: plan.delta,
                    limit_depth: depth,
                    limit_slot: slot,
                    op: plan.op,
                    body: body_start as u32,
                }),
                LoopLimit::Const(value) => self.emit(Op::ForNextConst {
                    depth: plan.depth,
                    slot: plan.slot,
                    delta: plan.delta,
                    value: value as i32,
                    op: plan.op,
                    body: body_start as u32,
                }),
            }
            jumped = true;
        }
        if !jumped
            && let Some(update) = update
            && let ExprKind::Update { op, argument, .. } = &update.kind
            && let ExprKind::Ident(name) = &argument.kind
            && let Some(reference) = self.resolve(name)
        {
            self.emit(Op::UpdateVarJump {
                depth: reference.depth,
                slot: reference.slot,
                delta: if matches!(op, UpdateOp::Increment) {
                    1
                } else {
                    -1
                },
                target: start as u32,
            });
            jumped = true;
        }
        if !jumped {
            if let Some(update) = update {
                self.compile_expr(update)?;
                self.emit(Op::Pop);
            }
            self.emit(Op::Jump(start as u32));
        }
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

    pub(super) fn compile_for_in_of(
        &mut self,
        left: &ForInit,
        right: &Expr,
        body: &Stmt,
        is_in: bool,
        _is_await: bool,
    ) -> Result<(), ParseError> {
        let lexical = matches!(left, ForInit::Var(declaration) if declaration.kind != VarKind::Var);
        // As with `for (let ...;;)`: only materialize a fresh binding per
        // iteration when nested functions can observe it. Otherwise the
        // binding lives in the enclosing scope and no per-iteration
        // PushEnv/PopEnv pair is emitted.
        let flat_binding = if lexical {
            let ForInit::Var(declaration) = left else {
                unreachable!();
            };
            let names: Vec<String> = declaration
                .declarations
                .first()
                .map(|declarator| pattern_names(&declarator.pattern))
                .unwrap_or_default();
            !body_captures_names(body, &names)
        } else {
            false
        };
        let outer_depth = self.local_frames.len();
        self.compile_expr(right)?;
        self.emit(if is_in { Op::ForInInit } else { Op::IterInit });
        let start = self.position();
        let end_jump = self.emit_jump(Op::IterNext(0));
        let mut iteration_size = 0;
        if lexical && !flat_binding {
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
        } else if lexical {
            let ForInit::Var(declaration) = left else {
                unreachable!();
            };
            if let Some(declarator) = declaration.declarations.first() {
                let names = pattern_names(&declarator.pattern);
                let frame = self.local_frames.last_mut().expect("scope frame");
                for name in &names {
                    frame.declare(name);
                }
            }
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
        if lexical && !flat_binding {
            self.emit(Op::PopEnv);
            self.local_frames.pop();
        }
        self.emit(Op::Jump(start as u32));
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

    pub(super) fn compile_break(&mut self, label: Option<&str>) -> Result<(), ParseError> {
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

    pub(super) fn compile_continue(&mut self, label: Option<&str>) -> Result<(), ParseError> {
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
            self.emit(Op::Jump(target as u32));
        } else {
            let jump = self.emit_jump(Op::Jump(0));
            self.loops[position].continue_jumps.push(jump);
        }
        Ok(())
    }

    pub(super) fn emit_active_finalizers(&mut self) -> Result<(), ParseError> {
        self.emit_finalizers_until(0)
    }

    pub(super) fn emit_finalizers_until(&mut self, target_depth: usize) -> Result<(), ParseError> {
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

    pub(super) fn emit_cleanup(
        &mut self,
        env_depth: usize,
        stack_cleanup: usize,
        try_depth: usize,
    ) {
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

    pub(super) fn compile_try(
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

    pub(super) fn compile_statements_in_scope(
        &mut self,
        statements: &[Stmt],
    ) -> Result<usize, ParseError> {
        self.compile_block(statements)?;
        Ok(self.position())
    }

    pub(super) fn compile_switch(
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

    pub(super) fn alloc_temp(&mut self) -> (u16, u16) {
        let name = format!("\0temp{}", self.temp_counter);
        self.temp_counter += 1;
        let slot = self.local_frames[0].declare(&name);
        let depth = (self.local_frames.len() - 1) as u16;
        (depth, slot)
    }
}
