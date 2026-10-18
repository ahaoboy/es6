//! Free helper functions: name collection, rguments usage analysis,
//! static-key extraction and operator tables.

use super::*;

pub(super) fn pattern_names(pattern: &Pattern) -> Vec<String> {
    let mut names = Vec::new();
    collect_pattern_names(pattern, &mut names);
    names
}

/// Whether any nested function or class inside `body` textually references
/// one of `names`. QuickJS only materializes a fresh binding per `for`-`let`
/// iteration when the loop variable is captured (`resolve_variables`,
/// quickjs.c:35111); this is the conservative static stand-in: a name that
/// only appears in nested functions keeps the per-iteration environment, and
/// shadowing merely over-approximates in the safe direction.
pub(super) fn body_captures_names(body: &Stmt, names: &[String]) -> bool {
    // Inner walkers: called once inside a nested function; plain identifiers
    // here are captures.
    fn scan_statements(statements: &[Stmt], names: &[String]) -> bool {
        statements
            .iter()
            .any(|statement| scan_statement(statement, names))
    }
    fn scan_statement(statement: &Stmt, names: &[String]) -> bool {
        match &statement.kind {
            StmtKind::Block(statements) => scan_statements(statements, names),
            StmtKind::Var(declaration) => declaration.declarations.iter().any(|declarator| {
                declarator
                    .init
                    .as_ref()
                    .is_some_and(|init| scan_expression(init, names))
            }),
            StmtKind::Function(function) => scan_function(function, names),
            StmtKind::Class(class) => scan_class(class, names),
            StmtKind::Expr(expression) | StmtKind::Throw(expression) => {
                scan_expression(expression, names)
            }
            StmtKind::Return(expression) => expression
                .as_ref()
                .is_some_and(|expression| scan_expression(expression, names)),
            StmtKind::If {
                test,
                consequent,
                alternate,
            } => {
                scan_expression(test, names)
                    || scan_statement(consequent, names)
                    || alternate
                        .as_ref()
                        .is_some_and(|alternate| scan_statement(alternate, names))
            }
            StmtKind::While { test, body } | StmtKind::DoWhile { body, test } => {
                scan_expression(test, names) || scan_statement(body, names)
            }
            StmtKind::For {
                init,
                test,
                update,
                body,
            } => {
                init.as_ref().is_some_and(|init| match init {
                    ForInit::Var(declaration) => {
                        declaration.declarations.iter().any(|declarator| {
                            declarator
                                .init
                                .as_ref()
                                .is_some_and(|init| scan_expression(init, names))
                        })
                    }
                    ForInit::Expr(expression) => scan_expression(expression, names),
                }) || test
                    .as_ref()
                    .is_some_and(|test| scan_expression(test, names))
                    || update
                        .as_ref()
                        .is_some_and(|update| scan_expression(update, names))
                    || scan_statement(body, names)
            }
            StmtKind::ForIn { left, right, body } | StmtKind::ForOf { left, right, body } => {
                let left_expression = match left {
                    ForInit::Var(declaration) => {
                        declaration.declarations.iter().any(|declarator| {
                            declarator
                                .init
                                .as_ref()
                                .is_some_and(|init| scan_expression(init, names))
                        })
                    }
                    ForInit::Expr(expression) => scan_expression(expression, names),
                };
                left_expression || scan_expression(right, names) || scan_statement(body, names)
            }
            StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Empty | StmtKind::Debugger => {
                false
            }
            StmtKind::Try {
                block,
                handler,
                finalizer,
            } => {
                scan_statements(block, names)
                    || handler
                        .as_ref()
                        .is_some_and(|handler| scan_statements(&handler.body, names))
                    || finalizer
                        .as_ref()
                        .is_some_and(|finalizer| scan_statements(finalizer, names))
            }
            StmtKind::Switch {
                discriminant,
                cases,
            } => {
                scan_expression(discriminant, names)
                    || cases.iter().any(|case| scan_statements(&case.body, names))
            }
            StmtKind::Labeled { body, .. } => scan_statement(body, names),
            StmtKind::Import(_) | StmtKind::Export(_) => false,
        }
    }
    fn scan_function(function: &Function, names: &[String]) -> bool {
        match &function.body {
            FunctionBody::Block(statements) => scan_statements(statements, names),
            FunctionBody::Expr(expression) => scan_expression(expression, names),
        }
    }
    fn scan_class(class: &ClassDecl, names: &[String]) -> bool {
        class.elements.iter().any(|element| {
            element
                .value
                .as_ref()
                .is_some_and(|value| scan_expression(value, names))
                || element
                    .function
                    .as_ref()
                    .is_some_and(|function| scan_function(function, names))
        })
    }
    fn scan_expression(expression: &Expr, names: &[String]) -> bool {
        match &expression.kind {
            ExprKind::Ident(name) => names.iter().any(|captured| captured == name),
            ExprKind::Function(function) | ExprKind::Arrow(function) => {
                scan_function(function, names)
            }
            ExprKind::Class(class) => scan_class(class, names),
            ExprKind::This | ExprKind::Literal(_) => false,
            ExprKind::Array(elements) => elements.iter().any(|element| match element {
                ArrayElement::Hole => false,
                ArrayElement::Expr(expression) | ArrayElement::Spread(expression) => {
                    scan_expression(expression, names)
                }
            }),
            ExprKind::Object(properties) => properties.iter().any(|property| match property {
                ObjectProperty::KeyValue { value, .. } => scan_expression(value, names),
                ObjectProperty::Shorthand(_) => false,
                ObjectProperty::Spread(expression) => scan_expression(expression, names),
                ObjectProperty::Method { function, .. } => scan_function(function, names),
            }),
            ExprKind::Unary { argument, .. }
            | ExprKind::Update { argument, .. }
            | ExprKind::Spread(argument)
            | ExprKind::Await(argument)
            | ExprKind::YieldStar(argument) => scan_expression(argument, names),
            ExprKind::Yield(argument) => argument
                .as_ref()
                .is_some_and(|argument| scan_expression(argument, names)),
            ExprKind::Binary { left, right, .. } | ExprKind::Logical { left, right, .. } => {
                scan_expression(left, names) || scan_expression(right, names)
            }
            ExprKind::Assign { target, value, .. } => {
                scan_expression(target, names) || scan_expression(value, names)
            }
            ExprKind::Conditional {
                test,
                consequent,
                alternate,
            } => {
                scan_expression(test, names)
                    || scan_expression(consequent, names)
                    || scan_expression(alternate, names)
            }
            ExprKind::Sequence(expressions) => expressions
                .iter()
                .any(|expression| scan_expression(expression, names)),
            ExprKind::Member {
                object, property, ..
            } => {
                scan_expression(object, names)
                    || match property {
                        MemberProperty::Ident(_) => false,
                        MemberProperty::Computed(expression) => scan_expression(expression, names),
                    }
            }
            ExprKind::Call {
                callee, arguments, ..
            } => {
                scan_expression(callee, names)
                    || arguments.iter().any(|argument| match argument {
                        Argument::Expr(expression) | Argument::Spread(expression) => {
                            scan_expression(expression, names)
                        }
                    })
            }
            ExprKind::New { callee, arguments } => {
                scan_expression(callee, names)
                    || arguments.iter().any(|argument| match argument {
                        Argument::Expr(expression) | Argument::Spread(expression) => {
                            scan_expression(expression, names)
                        }
                    })
            }
            ExprKind::Template { expressions, .. } => expressions
                .iter()
                .any(|expression| scan_expression(expression, names)),
            ExprKind::TaggedTemplate {
                tag, expressions, ..
            } => {
                scan_expression(tag, names)
                    || expressions
                        .iter()
                        .any(|expression| scan_expression(expression, names))
            }
        }
    }

    // Outer walkers: only descend to find nested function/class nodes; plain
    // identifiers at this level are ordinary loop-body uses, not captures.
    fn find_statements(statements: &[Stmt], names: &[String]) -> bool {
        statements
            .iter()
            .any(|statement| find_statement(statement, names))
    }
    fn find_statement(statement: &Stmt, names: &[String]) -> bool {
        match &statement.kind {
            StmtKind::Function(function) => scan_function(function, names),
            StmtKind::Class(class) => scan_class(class, names),
            StmtKind::Block(statements) => find_statements(statements, names),
            StmtKind::Var(declaration) => declaration.declarations.iter().any(|declarator| {
                declarator
                    .init
                    .as_ref()
                    .is_some_and(|init| find_expression(init, names))
            }),
            StmtKind::Expr(expression) | StmtKind::Throw(expression) => {
                find_expression(expression, names)
            }
            StmtKind::Return(expression) => expression
                .as_ref()
                .is_some_and(|expression| find_expression(expression, names)),
            StmtKind::If {
                test,
                consequent,
                alternate,
            } => {
                find_expression(test, names)
                    || find_statement(consequent, names)
                    || alternate
                        .as_ref()
                        .is_some_and(|alternate| find_statement(alternate, names))
            }
            StmtKind::While { test, body } | StmtKind::DoWhile { body, test } => {
                find_expression(test, names) || find_statement(body, names)
            }
            StmtKind::For {
                init,
                test,
                update,
                body,
            } => {
                init.as_ref().is_some_and(|init| match init {
                    ForInit::Var(declaration) => {
                        declaration.declarations.iter().any(|declarator| {
                            declarator
                                .init
                                .as_ref()
                                .is_some_and(|init| find_expression(init, names))
                        })
                    }
                    ForInit::Expr(expression) => find_expression(expression, names),
                }) || test
                    .as_ref()
                    .is_some_and(|test| find_expression(test, names))
                    || update
                        .as_ref()
                        .is_some_and(|update| find_expression(update, names))
                    || find_statement(body, names)
            }
            StmtKind::ForIn { left, right, body } | StmtKind::ForOf { left, right, body } => {
                let left_expression = match left {
                    ForInit::Var(declaration) => {
                        declaration.declarations.iter().any(|declarator| {
                            declarator
                                .init
                                .as_ref()
                                .is_some_and(|init| find_expression(init, names))
                        })
                    }
                    ForInit::Expr(expression) => find_expression(expression, names),
                };
                left_expression || find_expression(right, names) || find_statement(body, names)
            }
            StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Empty | StmtKind::Debugger => {
                false
            }
            StmtKind::Try {
                block,
                handler,
                finalizer,
            } => {
                find_statements(block, names)
                    || handler
                        .as_ref()
                        .is_some_and(|handler| find_statements(&handler.body, names))
                    || finalizer
                        .as_ref()
                        .is_some_and(|finalizer| find_statements(finalizer, names))
            }
            StmtKind::Switch {
                discriminant,
                cases,
            } => {
                find_expression(discriminant, names)
                    || cases.iter().any(|case| find_statements(&case.body, names))
            }
            StmtKind::Labeled { body, .. } => find_statement(body, names),
            StmtKind::Import(_) | StmtKind::Export(_) => false,
        }
    }
    fn find_expression(expression: &Expr, names: &[String]) -> bool {
        match &expression.kind {
            ExprKind::Function(function) | ExprKind::Arrow(function) => {
                scan_function(function, names)
            }
            ExprKind::Class(class) => scan_class(class, names),
            ExprKind::Ident(_) | ExprKind::This | ExprKind::Literal(_) => false,
            ExprKind::Array(elements) => elements.iter().any(|element| match element {
                ArrayElement::Hole => false,
                ArrayElement::Expr(expression) | ArrayElement::Spread(expression) => {
                    find_expression(expression, names)
                }
            }),
            ExprKind::Object(properties) => properties.iter().any(|property| match property {
                ObjectProperty::KeyValue { value, .. } => find_expression(value, names),
                ObjectProperty::Shorthand(_) => false,
                ObjectProperty::Spread(expression) => find_expression(expression, names),
                ObjectProperty::Method { function, .. } => scan_function(function, names),
            }),
            ExprKind::Unary { argument, .. }
            | ExprKind::Update { argument, .. }
            | ExprKind::Spread(argument)
            | ExprKind::Await(argument)
            | ExprKind::YieldStar(argument) => find_expression(argument, names),
            ExprKind::Yield(argument) => argument
                .as_ref()
                .is_some_and(|argument| find_expression(argument, names)),
            ExprKind::Binary { left, right, .. } | ExprKind::Logical { left, right, .. } => {
                find_expression(left, names) || find_expression(right, names)
            }
            ExprKind::Assign { target, value, .. } => {
                find_expression(target, names) || find_expression(value, names)
            }
            ExprKind::Conditional {
                test,
                consequent,
                alternate,
            } => {
                find_expression(test, names)
                    || find_expression(consequent, names)
                    || find_expression(alternate, names)
            }
            ExprKind::Sequence(expressions) => expressions
                .iter()
                .any(|expression| find_expression(expression, names)),
            ExprKind::Member {
                object, property, ..
            } => {
                find_expression(object, names)
                    || match property {
                        MemberProperty::Ident(_) => false,
                        MemberProperty::Computed(expression) => find_expression(expression, names),
                    }
            }
            ExprKind::Call {
                callee, arguments, ..
            } => {
                find_expression(callee, names)
                    || arguments.iter().any(|argument| match argument {
                        Argument::Expr(expression) | Argument::Spread(expression) => {
                            find_expression(expression, names)
                        }
                    })
            }
            ExprKind::New { callee, arguments } => {
                find_expression(callee, names)
                    || arguments.iter().any(|argument| match argument {
                        Argument::Expr(expression) | Argument::Spread(expression) => {
                            find_expression(expression, names)
                        }
                    })
            }
            ExprKind::Template { expressions, .. } => expressions
                .iter()
                .any(|expression| find_expression(expression, names)),
            ExprKind::TaggedTemplate {
                tag, expressions, ..
            } => {
                find_expression(tag, names)
                    || expressions
                        .iter()
                        .any(|expression| find_expression(expression, names))
            }
        }
    }
    find_statement(body, names)
}

pub(super) fn collect_pattern_names(pattern: &Pattern, names: &mut Vec<String>) {
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

pub(super) fn collect_var_names(statements: &[Stmt]) -> Vec<String> {
    let mut names = Vec::new();
    collect_var_names_inner(statements, &mut names);
    names
}

pub(super) fn collect_var_names_inner(statements: &[Stmt], names: &mut Vec<String>) {
    for statement in statements {
        match &statement.kind {
            StmtKind::Export(export) => {
                if let ExportKind::Declaration(declaration) = &export.kind {
                    collect_var_names_inner(std::slice::from_ref(declaration.as_ref()), names);
                }
            }
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

/// Bindings introduced by a declaration, used to populate the exports
/// namespace of an `export <declaration>` statement.
pub(super) fn declaration_names(statement: &Stmt) -> Vec<String> {
    match &statement.kind {
        StmtKind::Var(declaration) => {
            let mut names = Vec::new();
            for declarator in &declaration.declarations {
                collect_pattern_names(&declarator.pattern, &mut names);
            }
            names
        }
        StmtKind::Function(function) => function.name.iter().cloned().collect(),
        StmtKind::Class(class) => class.name.iter().cloned().collect(),
        _ => Vec::new(),
    }
}

pub(super) fn collect_direct_bindings(statements: &[Stmt]) -> Vec<String> {
    let mut names = Vec::new();
    for statement in statements {
        match &statement.kind {
            StmtKind::Export(export) => {
                if let ExportKind::Declaration(declaration) = &export.kind {
                    names.extend(collect_direct_bindings(std::slice::from_ref(
                        declaration.as_ref(),
                    )));
                }
            }
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
            StmtKind::Import(import) => {
                for specifier in &import.specifiers {
                    names.push(match specifier {
                        ImportSpecifier::Default(local)
                        | ImportSpecifier::Namespace(local)
                        | ImportSpecifier::Named { local, .. } => local.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    names
}

pub(super) fn expression_to_pattern(expression: &Expr) -> Option<Pattern> {
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

pub(super) fn comparison_op(op: BinaryOp) -> Option<CompareOp> {
    Some(match op {
        BinaryOp::Lt => CompareOp::Lt,
        BinaryOp::Le => CompareOp::Le,
        BinaryOp::Gt => CompareOp::Gt,
        BinaryOp::Ge => CompareOp::Ge,
        _ => return None,
    })
}

pub(super) fn binary_op(op: BinaryOp) -> Op {
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

pub(super) fn assign_binary_op(op: AssignOp) -> Op {
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

pub(super) fn has_use_strict(statements: &[Stmt]) -> bool {
    matches!(
        statements.first().map(|statement| &statement.kind),
        Some(StmtKind::Expr(Expr {
            kind: ExprKind::Literal(Literal::String(value)),
            ..
        })) if value == "use strict"
    )
}

/// True when the function body can observe its own `arguments` binding.
/// Nested non-arrow functions have their own binding and are skipped; arrows
/// capture the enclosing one and are traversed.
pub(super) fn function_uses_arguments(function: &Function) -> bool {
    match &function.body {
        FunctionBody::Block(statements) => statements.iter().any(uses_arguments_stmt),
        FunctionBody::Expr(expression) => uses_arguments_expr(expression),
    }
}

pub(super) fn uses_arguments_stmt(statement: &Stmt) -> bool {
    match &statement.kind {
        StmtKind::Empty
        | StmtKind::Break(_)
        | StmtKind::Continue(_)
        | StmtKind::Debugger
        | StmtKind::Import(_) => false,
        StmtKind::Export(export) => match &export.kind {
            ExportKind::Default(expression) => uses_arguments_expr(expression),
            ExportKind::Declaration(statement) => uses_arguments_stmt(statement),
            ExportKind::Named(_) | ExportKind::All { .. } => false,
        },
        StmtKind::Expr(expression) | StmtKind::Throw(expression) => uses_arguments_expr(expression),
        StmtKind::Block(statements) => statements.iter().any(uses_arguments_stmt),
        StmtKind::Var(declaration) => declaration.declarations.iter().any(|declarator| {
            declarator.init.as_ref().is_some_and(uses_arguments_expr)
                || uses_arguments_pattern(&declarator.pattern)
        }),
        StmtKind::Function(function) => function.is_arrow && function_uses_arguments(function),
        StmtKind::Class(class) => uses_arguments_class(class),
        StmtKind::If {
            test,
            consequent,
            alternate,
        } => {
            uses_arguments_expr(test)
                || uses_arguments_stmt(consequent)
                || alternate.as_deref().is_some_and(uses_arguments_stmt)
        }
        StmtKind::While { test, body } | StmtKind::DoWhile { test, body } => {
            uses_arguments_expr(test) || uses_arguments_stmt(body)
        }
        StmtKind::For {
            init,
            test,
            update,
            body,
        } => {
            init.as_ref().is_some_and(uses_arguments_for_init)
                || test.as_ref().is_some_and(uses_arguments_expr)
                || update.as_ref().is_some_and(uses_arguments_expr)
                || uses_arguments_stmt(body)
        }
        StmtKind::ForIn { left, right, body } | StmtKind::ForOf { left, right, body } => {
            uses_arguments_for_init(left) || uses_arguments_expr(right) || uses_arguments_stmt(body)
        }
        StmtKind::Return(expression) => expression.as_ref().is_some_and(uses_arguments_expr),
        StmtKind::Try {
            block,
            handler,
            finalizer,
        } => {
            block.iter().any(uses_arguments_stmt)
                || handler.as_ref().is_some_and(|handler| {
                    handler.param.as_ref().is_some_and(uses_arguments_pattern)
                        || handler.body.iter().any(uses_arguments_stmt)
                })
                || finalizer
                    .as_ref()
                    .is_some_and(|statements| statements.iter().any(uses_arguments_stmt))
        }
        StmtKind::Switch {
            discriminant,
            cases,
        } => {
            uses_arguments_expr(discriminant)
                || cases.iter().any(|case| {
                    case.test.as_ref().is_some_and(uses_arguments_expr)
                        || case.body.iter().any(uses_arguments_stmt)
                })
        }
        StmtKind::Labeled { body, .. } => uses_arguments_stmt(body),
    }
}

pub(super) fn uses_arguments_for_init(init: &ForInit) -> bool {
    match init {
        ForInit::Var(declaration) => declaration.declarations.iter().any(|declarator| {
            declarator.init.as_ref().is_some_and(uses_arguments_expr)
                || uses_arguments_pattern(&declarator.pattern)
        }),
        ForInit::Expr(expression) => uses_arguments_expr(expression),
    }
}

pub(super) fn uses_arguments_pattern(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Ident(_) => false,
        Pattern::Array(elements) => elements.iter().flatten().any(uses_arguments_pattern),
        Pattern::Object(properties) => properties.iter().any(|property| {
            matches!(&property.key, PropertyName::Computed(expression) if uses_arguments_expr(expression))
                || uses_arguments_pattern(&property.value)
        }),
        Pattern::Rest(inner) => uses_arguments_pattern(inner),
        Pattern::Assign { target, default } => {
            uses_arguments_pattern(target) || uses_arguments_expr(default)
        }
    }
}

pub(super) fn uses_arguments_class(class: &ClassDecl) -> bool {
    class.super_class.as_deref().is_some_and(uses_arguments_expr)
        || class.elements.iter().any(|element| {
            matches!(&element.key, PropertyName::Computed(expression) if uses_arguments_expr(expression))
        })
}

pub(super) fn uses_arguments_expr(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Ident(name) => name == "arguments",
        ExprKind::Literal(_) | ExprKind::This => false,
        ExprKind::Array(elements) => elements.iter().any(|element| match element {
            ArrayElement::Hole => false,
            ArrayElement::Expr(expression) | ArrayElement::Spread(expression) => {
                uses_arguments_expr(expression)
            }
        }),
        ExprKind::Object(properties) => properties.iter().any(|property| match property {
            ObjectProperty::KeyValue { key, value } => {
                matches!(key, PropertyName::Computed(expression) if uses_arguments_expr(expression))
                    || uses_arguments_expr(value)
            }
            ObjectProperty::Shorthand(name) => name == "arguments",
            ObjectProperty::Method { key, function, .. } => {
                (matches!(key, PropertyName::Computed(expression) if uses_arguments_expr(expression)))
                    || (function.is_arrow && function_uses_arguments(function))
            }
            ObjectProperty::Spread(expression) => uses_arguments_expr(expression),
        }),
        ExprKind::Function(function) => function.is_arrow && function_uses_arguments(function),
        ExprKind::Arrow(function) => function_uses_arguments(function),
        ExprKind::Class(class) => uses_arguments_class(class),
        ExprKind::Unary { argument, .. }
        | ExprKind::Update { argument, .. }
        | ExprKind::Spread(argument)
        | ExprKind::Await(argument) => uses_arguments_expr(argument),
        ExprKind::Binary { left, right, .. } | ExprKind::Logical { left, right, .. } => {
            uses_arguments_expr(left) || uses_arguments_expr(right)
        }
        ExprKind::Assign { target, value, .. } => {
            uses_arguments_expr(target) || uses_arguments_expr(value)
        }
        ExprKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            uses_arguments_expr(test)
                || uses_arguments_expr(consequent)
                || uses_arguments_expr(alternate)
        }
        ExprKind::Sequence(expressions) => expressions.iter().any(uses_arguments_expr),
        ExprKind::Member {
            object, property, ..
        } => {
            uses_arguments_expr(object)
                || matches!(property, MemberProperty::Computed(index) if uses_arguments_expr(index))
        }
        ExprKind::Call {
            callee, arguments, ..
        }
        | ExprKind::New { callee, arguments } => {
            uses_arguments_expr(callee)
                || arguments.iter().any(|argument| match argument {
                    Argument::Expr(expression) | Argument::Spread(expression) => {
                        uses_arguments_expr(expression)
                    }
                })
        }
        ExprKind::Template { expressions, .. } => expressions.iter().any(uses_arguments_expr),
        ExprKind::TaggedTemplate {
            tag, expressions, ..
        } => uses_arguments_expr(tag) || expressions.iter().any(uses_arguments_expr),
        ExprKind::Yield(expression) => expression.as_deref().is_some_and(uses_arguments_expr),
        ExprKind::YieldStar(expression) => uses_arguments_expr(expression),
    }
}

/// Whether evaluating `expression` may reassign locals or invoke user code
/// that could change the object/index variables of a fused indexed store.
pub(super) fn expr_is_stable(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::This => true,
        ExprKind::Unary { argument, .. } => expr_is_stable(argument),
        ExprKind::Binary { left, right, .. } | ExprKind::Logical { left, right, .. } => {
            expr_is_stable(left) && expr_is_stable(right)
        }
        ExprKind::Conditional {
            test,
            consequent,
            alternate,
        } => expr_is_stable(test) && expr_is_stable(consequent) && expr_is_stable(alternate),
        ExprKind::Sequence(expressions) => expressions.iter().all(expr_is_stable),
        ExprKind::Array(elements) => elements.iter().all(|element| match element {
            ArrayElement::Hole => true,
            ArrayElement::Expr(expression) => expr_is_stable(expression),
            ArrayElement::Spread(_) => false,
        }),
        ExprKind::Object(properties) => properties.iter().all(|property| match property {
            ObjectProperty::KeyValue { key, value } => {
                !matches!(key, PropertyName::Computed(_)) && expr_is_stable(value)
            }
            ObjectProperty::Shorthand(_) => true,
            ObjectProperty::Spread(_) | ObjectProperty::Method { .. } => false,
        }),
        ExprKind::Member {
            object, property, ..
        } => {
            expr_is_stable(object)
                && match property {
                    MemberProperty::Ident(_) => true,
                    MemberProperty::Computed(index) => expr_is_stable(index),
                }
        }
        ExprKind::Assign { .. }
        | ExprKind::Update { .. }
        | ExprKind::Call { .. }
        | ExprKind::New { .. }
        | ExprKind::Function(_)
        | ExprKind::Arrow(_)
        | ExprKind::Class(_)
        | ExprKind::Template { .. }
        | ExprKind::TaggedTemplate { .. }
        | ExprKind::Spread(_)
        | ExprKind::Await(_)
        | ExprKind::Yield(_) => false,
        ExprKind::YieldStar(_) => false,
    }
}

pub(super) fn static_property_name(name: &PropertyName) -> Option<String> {
    match name {
        PropertyName::Ident(value) | PropertyName::String(value) => Some(value.clone()),
        PropertyName::Number(value) => Some(format_number(*value)),
        PropertyName::Computed(_) => None,
    }
}

pub(super) fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{}", value as i64)
    } else {
        let mut buffer = ryu::Buffer::new();
        buffer.format(value).to_owned()
    }
}
