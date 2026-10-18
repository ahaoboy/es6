//! Expression compilation: operators, members, calls, literals.
//!
//! Counterpart of the js_parse_expr2 family (quickjs.c:28515).

use super::*;

impl Compiler {
    /// QuickJS expression lowering from `js_parse_expr2` and the operator
    /// specific parsers (quickjs.c:28515).
    pub(super) fn compile_expr(&mut self, expression: &Expr) -> Result<(), ParseError> {
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
                if *op == BinaryOp::Add
                    && let ExprKind::Ident(left_name) = &left.kind
                    && let Some(left_ref) = self.resolve(left_name)
                {
                    if let ExprKind::Ident(right_name) = &right.kind
                        && let Some(right_ref) = self.resolve(right_name)
                    {
                        self.emit(Op::AddVars {
                            left_depth: left_ref.depth,
                            left_slot: left_ref.slot,
                            right_depth: right_ref.depth,
                            right_slot: right_ref.slot,
                        });
                        return Ok(());
                    }
                    if let ExprKind::Literal(Literal::Number(value)) = &right.kind {
                        let index = self.add_constant(Constant::Number(*value));
                        self.emit(Op::AddVarConst {
                            depth: left_ref.depth,
                            slot: left_ref.slot,
                            value: index,
                        });
                        return Ok(());
                    }
                }
                let compare = match op {
                    BinaryOp::Lt => Some(CompareOp::Lt),
                    BinaryOp::Le => Some(CompareOp::Le),
                    BinaryOp::Gt => Some(CompareOp::Gt),
                    BinaryOp::Ge => Some(CompareOp::Ge),
                    _ => None,
                };
                if let Some(compare) = compare
                    && let ExprKind::Ident(left_name) = &left.kind
                    && let Some(left_ref) = self.resolve(left_name)
                {
                    if let ExprKind::Ident(right_name) = &right.kind
                        && let Some(right_ref) = self.resolve(right_name)
                    {
                        self.emit(Op::CompareVars {
                            op: compare,
                            left_depth: left_ref.depth,
                            left_slot: left_ref.slot,
                            right_depth: right_ref.depth,
                            right_slot: right_ref.slot,
                        });
                        return Ok(());
                    }
                    if let ExprKind::Literal(Literal::Number(value)) = &right.kind {
                        let value = self.add_constant(Constant::Number(*value));
                        self.emit(Op::CompareVarConst {
                            op: compare,
                            depth: left_ref.depth,
                            slot: left_ref.slot,
                            value,
                        });
                        return Ok(());
                    }
                }
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
                    let mut fused = false;
                    if let MemberProperty::Computed(index) = property
                        && let ExprKind::Ident(object_name) = &object.kind
                        && let Some(object_ref) = self.resolve(object_name)
                    {
                        if let ExprKind::Ident(index_name) = &index.kind
                            && let Some(index_ref) = self.resolve(index_name)
                        {
                            self.emit(Op::GetIndexVar {
                                object_depth: object_ref.depth,
                                object_slot: object_ref.slot,
                                index_depth: index_ref.depth,
                                index_slot: index_ref.slot,
                            });
                            fused = true;
                        } else if let ExprKind::Update {
                            op,
                            prefix,
                            argument,
                        } = &index.kind
                            && let ExprKind::Ident(index_name) = &argument.kind
                            && let Some(index_ref) = self.resolve(index_name)
                        {
                            self.emit(Op::GetIndexUpdate {
                                object_depth: object_ref.depth,
                                object_slot: object_ref.slot,
                                index_depth: index_ref.depth,
                                index_slot: index_ref.slot,
                                delta: if matches!(op, UpdateOp::Increment) {
                                    1
                                } else {
                                    -1
                                },
                                prefix: *prefix,
                            });
                            fused = true;
                        }
                    }
                    if !fused {
                        self.compile_expr(object)?;
                        match property {
                            MemberProperty::Ident(name) => {
                                self.emit_get_prop_const(name);
                            }
                            MemberProperty::Computed(expression) => {
                                self.compile_expr(expression)?;
                                self.emit(Op::GetProp);
                            }
                        }
                    }
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

    pub(super) fn compile_literal(&mut self, literal: &Literal) {
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

    pub(super) fn compile_array_literal(&mut self, elements: &[ArrayElement]) -> Result<(), ParseError> {
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

    pub(super) fn compile_object_literal(&mut self, properties: &[ObjectProperty]) -> Result<(), ParseError> {
        if !properties.is_empty() {
            let mut keys = Vec::with_capacity(properties.len());
            let mut static_keys = true;
            for property in properties {
                match property {
                    ObjectProperty::KeyValue { key, .. } => {
                        if let Some(name) = static_property_name(key) {
                            keys.push(Rc::from(name));
                        } else {
                            static_keys = false;
                            break;
                        }
                    }
                    ObjectProperty::Shorthand(name) => keys.push(Rc::from(name.as_str())),
                    _ => {
                        static_keys = false;
                        break;
                    }
                }
            }
            if static_keys {
                for property in properties {
                    match property {
                        ObjectProperty::KeyValue { value, .. } => self.compile_expr(value)?,
                        ObjectProperty::Shorthand(name) => self.emit_ident_load(name),
                        _ => unreachable!(),
                    }
                }
                let keys = self.add_constant(Constant::Strings(Rc::from(keys)));
                self.emit(Op::NewObjectFrom {
                    keys,
                    count: properties.len() as u16,
                });
                return Ok(());
            }
        }
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

    pub(super) fn compile_key_name(&mut self, key: &PropertyName) -> Result<(), ParseError> {
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

    pub(super) fn compile_member_key(&mut self, property: &MemberProperty) -> Result<(), ParseError> {
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

    pub(super) fn compile_optional_member(
        &mut self,
        object: &Expr,
        property: &MemberProperty,
    ) -> Result<(), ParseError> {
        self.compile_expr(object)?;
        self.emit(Op::Dup);
        let nullish_jump = self.emit_jump(Op::JumpIfNullishKeep(0));
        match property {
            MemberProperty::Ident(name) => {
                self.emit_get_prop_const(name);
            }
            MemberProperty::Computed(expression) => {
                self.compile_expr(expression)?;
                self.emit(Op::GetProp);
            }
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

    pub(super) fn compile_optional_member_call(
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
        match property {
            MemberProperty::Ident(name) => {
                self.emit_get_prop_const(name);
            }
            MemberProperty::Computed(expression) => {
                self.compile_expr(expression)?;
                self.emit(Op::GetProp);
            }
        }
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

    pub(super) fn compile_optional_call(
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
            match property {
                MemberProperty::Ident(name) => {
                    self.emit_get_prop_const(name);
                }
                MemberProperty::Computed(expression) => {
                    self.compile_expr(expression)?;
                    self.emit(Op::GetProp);
                }
            }
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

    pub(super) fn compile_call(
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
            match property {
                MemberProperty::Ident(name) => {
                    self.emit_get_prop_const(name);
                }
                MemberProperty::Computed(expression) => {
                    self.compile_expr(expression)?;
                    self.emit(Op::GetProp);
                }
            }
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

    pub(super) fn compile_arguments(&mut self, arguments: &[Argument]) -> Result<(), ParseError> {
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

    pub(super) fn compile_arguments_array(&mut self, arguments: &[Argument]) -> Result<(), ParseError> {
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

    pub(super) fn compile_unary(&mut self, op: UnaryOp, argument: &Expr) -> Result<(), ParseError> {
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

    pub(super) fn compile_update(
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

    pub(super) fn compile_assignment(
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
                if op == AssignOp::Add
                    && let ExprKind::Ident(source) = &value.kind
                    && let Some(dst) = reference
                    && let Some(src) = self.resolve(source)
                {
                    self.emit(Op::AddVar {
                        dst_depth: dst.depth,
                        dst_slot: dst.slot,
                        src_depth: src.depth,
                        src_slot: src.slot,
                    });
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
                if op == AssignOp::Assign
                    && let MemberProperty::Computed(index) = property
                    && let ExprKind::Ident(object_name) = &object.kind
                    && let ExprKind::Ident(index_name) = &index.kind
                    && let Some(object_ref) = self.resolve(object_name)
                    && let Some(index_ref) = self.resolve(index_name)
                    && expr_is_stable(value)
                {
                    self.compile_expr(value)?;
                    self.emit(Op::SetIndexVar {
                        object_depth: object_ref.depth,
                        object_slot: object_ref.slot,
                        index_depth: index_ref.depth,
                        index_slot: index_ref.slot,
                    });
                    return Ok(());
                }
                self.compile_expr(object)?;
                if let MemberProperty::Ident(name) = property {
                    if op == AssignOp::Assign {
                        self.compile_expr(value)?;
                        self.emit_set_prop_const(name, false);
                        return Ok(());
                    }
                    self.emit(Op::Dup);
                    self.emit_get_prop_const(name);
                    self.compile_expr(value)?;
                    self.emit(assign_binary_op(op));
                    self.emit_set_prop_const(name, false);
                    return Ok(());
                }
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

    pub(super) fn compile_assignment_target_store(&mut self, target: &Expr) -> Result<(), ParseError> {
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
                if let MemberProperty::Ident(name) = property {
                    self.emit(Op::GetVar { depth, slot });
                    self.emit_set_prop_const(name, false);
                } else {
                    self.compile_member_key(property)?;
                    self.emit(Op::GetVar { depth, slot });
                    self.emit(Op::SetProp);
                }
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

    pub(super) fn emit_ident_load(&mut self, name: &str) {
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

    pub(super) fn store_ident(&mut self, name: &str, mode: BindingMode) {
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
}
