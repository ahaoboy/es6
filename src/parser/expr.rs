//! Expression parsing: binary precedence climbing (`js_parse_expr_binary`, quickjs.c:27972), unary/postfix/left-hand-side.

use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        let first = self.parse_assignment()?;
        if !self.eat_punct(Punct::Comma)? {
            return Ok(first);
        }
        let span = first.span;
        let mut expressions = vec![first];
        loop {
            expressions.push(self.parse_assignment()?);
            if !self.eat_punct(Punct::Comma)? {
                break;
            }
        }
        Ok(Expr {
            kind: ExprKind::Sequence(expressions),
            span,
        })
    }

    /// QuickJS `js_parse_assign_expr` (quickjs.c:24748).
    pub(super) fn parse_assignment(&mut self) -> Result<Expr, ParseError> {
        if let Some(arrow) = self.try_parse_arrow()? {
            return Ok(arrow);
        }
        let left = self.parse_conditional()?;
        let op = if self.current.is_punct(Punct::Assign) {
            Some(AssignOp::Assign)
        } else if self.current.is_punct(Punct::AddAssign) {
            Some(AssignOp::Add)
        } else if self.current.is_punct(Punct::SubAssign) {
            Some(AssignOp::Sub)
        } else if self.current.is_punct(Punct::MulAssign) {
            Some(AssignOp::Mul)
        } else if self.current.is_punct(Punct::DivAssign) {
            Some(AssignOp::Div)
        } else if self.current.is_punct(Punct::ModAssign) {
            Some(AssignOp::Mod)
        } else if self.current.is_punct(Punct::ExpAssign) {
            Some(AssignOp::Exp)
        } else if self.current.is_punct(Punct::BitAndAssign) {
            Some(AssignOp::BitAnd)
        } else if self.current.is_punct(Punct::BitOrAssign) {
            Some(AssignOp::BitOr)
        } else if self.current.is_punct(Punct::BitXorAssign) {
            Some(AssignOp::BitXor)
        } else if self.current.is_punct(Punct::ShlAssign) {
            Some(AssignOp::Shl)
        } else if self.current.is_punct(Punct::ShrAssign) {
            Some(AssignOp::Shr)
        } else if self.current.is_punct(Punct::UShrAssign) {
            Some(AssignOp::UShr)
        } else if self.current.is_punct(Punct::AndAssign) {
            Some(AssignOp::And)
        } else if self.current.is_punct(Punct::OrAssign) {
            Some(AssignOp::Or)
        } else if self.current.is_punct(Punct::NullishAssign) {
            Some(AssignOp::Nullish)
        } else {
            None
        };
        if let Some(op) = op {
            if !is_assignment_target(&left) {
                return Err(self.error("Invalid assignment target"));
            }
            self.advance()?;
            let value = self.parse_assignment()?;
            let span = left.span.join(value.span);
            return Ok(Expr {
                kind: ExprKind::Assign {
                    op,
                    target: Box::new(left),
                    value: Box::new(value),
                },
                span,
            });
        }
        Ok(left)
    }

    pub(super) fn try_parse_arrow(&mut self) -> Result<Option<Expr>, ParseError> {
        let checkpoint = (self.lexer.clone(), self.current.clone(), self.previous_line);
        let span = self.span();
        if self.current.is_ident("async")
            && let Some(next) = self.peek()
            && (next.is_punct(Punct::LParen) || matches!(next.kind, TokenKind::Ident(_)))
        {
            self.advance()?;
            if let Some(arrow) = self.try_parse_arrow_inner(span, true)? {
                return Ok(Some(arrow));
            }
            let (lexer, current, previous_line) = checkpoint;
            self.lexer = lexer;
            self.current = current;
            self.previous_line = previous_line;
            return Ok(None);
        }
        if let Ok(Some(arrow)) = self.try_parse_arrow_inner(span, false) {
            return Ok(Some(arrow));
        }
        let (lexer, current, previous_line) = checkpoint;
        self.lexer = lexer;
        self.current = current;
        self.previous_line = previous_line;
        Ok(None)
    }

    pub(super) fn try_parse_arrow_inner(
        &mut self,
        span: Span,
        is_async: bool,
    ) -> Result<Option<Expr>, ParseError> {
        let params = if let TokenKind::Ident(name) = &self.current.kind {
            let name = name.clone();
            self.advance()?;
            vec![Pattern::Ident(name)]
        } else if self.current.is_punct(Punct::LParen) {
            self.advance()?;
            let mut params = Vec::new();
            while !self.current.is_punct(Punct::RParen) {
                if self.eat_punct(Punct::Ellipsis)? {
                    params.push(Pattern::Rest(Box::new(self.parse_binding_pattern()?)));
                    break;
                }
                params.push(self.parse_binding_pattern_with_default()?);
                if !self.eat_punct(Punct::Comma)? {
                    break;
                }
            }
            self.expect_punct(Punct::RParen)?;
            params
        } else {
            return Ok(None);
        };
        if !self.current.is_punct(Punct::Arrow) {
            return Ok(None);
        }
        self.advance()?;
        let body = if self.current.is_punct(Punct::LBrace) {
            FunctionBody::Block(self.parse_block()?)
        } else {
            FunctionBody::Expr(Box::new(self.parse_assignment()?))
        };
        Ok(Some(Expr {
            kind: ExprKind::Arrow(Rc::new(Function {
                name: None,
                params,
                body,
                is_arrow: true,
                is_async,
                is_generator: false,
                span,
            })),
            span,
        }))
    }

    pub(super) fn parse_conditional(&mut self) -> Result<Expr, ParseError> {
        let test = self.parse_binary(0)?;
        if !self.eat_punct(Punct::Question)? {
            return Ok(test);
        }
        let consequent = self.parse_assignment()?;
        self.expect_punct(Punct::Colon)?;
        let alternate = self.parse_assignment()?;
        let span = test.span.join(alternate.span);
        Ok(Expr {
            kind: ExprKind::Conditional {
                test: Box::new(test),
                consequent: Box::new(consequent),
                alternate: Box::new(alternate),
            },
            span,
        })
    }

    /// Precedence-climbing equivalent of QuickJS' layered `js_parse_*_expr`
    /// functions; the operator table is shared with `js_parse_expr` (24732).
    pub(super) fn parse_binary(&mut self, minimum_precedence: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        while let Some((precedence, operator)) = binary_operator(&self.current) {
            if precedence < minimum_precedence {
                break;
            }
            if !self.allow_in && matches!(operator, BinaryKind::Binary(BinaryOp::In)) {
                break;
            }
            self.advance()?;
            let right_minimum = if operator == BinaryKind::Binary(BinaryOp::Exp) {
                precedence
            } else {
                precedence + 1
            };
            let right = self.parse_binary(right_minimum)?;
            let span = left.span.join(right.span);
            left = match operator {
                BinaryKind::Binary(op) => Expr {
                    kind: ExprKind::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    span,
                },
                BinaryKind::Logical(op) => Expr {
                    kind: ExprKind::Logical {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    span,
                },
            };
        }
        Ok(left)
    }

    /// QuickJS `js_parse_unary` (quickjs.c:24750).
    pub(super) fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let span = self.span();
        let op = if self.current.is_punct(Punct::Plus) {
            Some(UnaryOp::Plus)
        } else if self.current.is_punct(Punct::Minus) {
            Some(UnaryOp::Minus)
        } else if self.current.is_punct(Punct::Not) {
            Some(UnaryOp::Not)
        } else if self.current.is_punct(Punct::BitNot) {
            Some(UnaryOp::BitNot)
        } else if self.current.is_ident("typeof") {
            Some(UnaryOp::TypeOf)
        } else if self.current.is_ident("void") {
            Some(UnaryOp::Void)
        } else if self.current.is_ident("delete") {
            Some(UnaryOp::Delete)
        } else {
            None
        };
        if let Some(op) = op {
            self.advance()?;
            let argument = self.parse_unary()?;
            let span = span.join(argument.span);
            return Ok(Expr {
                kind: ExprKind::Unary {
                    op,
                    argument: Box::new(argument),
                },
                span,
            });
        }
        if self.current.is_ident("await") {
            self.advance()?;
            let argument = self.parse_unary()?;
            let span = span.join(argument.span);
            return Ok(Expr {
                kind: ExprKind::Await(Box::new(argument)),
                span,
            });
        }
        if self.current.is_ident("yield") && self.yield_allowed {
            // `yield` and `yield*` (ES2022 15.5.2): the operand requires no
            // line terminator, like `js_parse_assign_expr2` (quickjs.c:28240).
            let yield_line = self.current.line;
            self.advance()?;
            let delegate = self.eat_punct(Punct::Star)?;
            let same_line = self.current.line == yield_line;
            let terminator = matches!(self.current.kind, TokenKind::Eof)
                || self.current.is_punct(Punct::Semicolon)
                || self.current.is_punct(Punct::RParen)
                || self.current.is_punct(Punct::RBracket)
                || self.current.is_punct(Punct::RBrace)
                || self.current.is_punct(Punct::Comma)
                || self.current.is_punct(Punct::Colon);
            let argument = if delegate || (same_line && !terminator) {
                Some(Box::new(self.parse_assignment()?))
            } else {
                None
            };
            return Ok(Expr {
                kind: if delegate {
                    ExprKind::YieldStar(argument.expect("yield* requires an operand"))
                } else {
                    ExprKind::Yield(argument)
                },
                span,
            });
        }
        if self.current.is_punct(Punct::Inc) || self.current.is_punct(Punct::Dec) {
            let op = if self.current.is_punct(Punct::Inc) {
                UpdateOp::Increment
            } else {
                UpdateOp::Decrement
            };
            self.advance()?;
            let argument = self.parse_unary()?;
            let span = span.join(argument.span);
            return Ok(Expr {
                kind: ExprKind::Update {
                    op,
                    prefix: true,
                    argument: Box::new(argument),
                },
                span,
            });
        }
        self.parse_postfix()
    }

    pub(super) fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expression = self.parse_left_hand_side()?;
        if (self.current.is_punct(Punct::Inc) || self.current.is_punct(Punct::Dec))
            && self.current.line == self.previous_line
        {
            let op = if self.current.is_punct(Punct::Inc) {
                UpdateOp::Increment
            } else {
                UpdateOp::Decrement
            };
            self.advance()?;
            let span = expression.span;
            expression = Expr {
                kind: ExprKind::Update {
                    op,
                    prefix: false,
                    argument: Box::new(expression),
                },
                span,
            };
        }
        Ok(expression)
    }

    /// QuickJS `js_parse_postfix_expr` / `js_parse_left_hand_side_expr`
    /// (quickjs.c:25381 and 25391), including member/call chaining.
    pub(super) fn parse_left_hand_side(&mut self) -> Result<Expr, ParseError> {
        let mut expression = if self.current.is_ident("new") {
            self.parse_new()?
        } else {
            self.parse_primary()?
        };
        loop {
            if self.eat_punct(Punct::QuestionDot)? {
                if self.eat_punct(Punct::LBracket)? {
                    let previous_allow_in = self.allow_in;
                    self.allow_in = true;
                    let property = self.parse_expression();
                    self.allow_in = previous_allow_in;
                    let property = property?;
                    self.expect_punct(Punct::RBracket)?;
                    let span = expression.span.join(property.span);
                    expression = Expr {
                        kind: ExprKind::Member {
                            object: Box::new(expression),
                            property: MemberProperty::Computed(Box::new(property)),
                            optional: true,
                        },
                        span,
                    };
                } else if self.current.is_punct(Punct::LParen) {
                    let arguments = self.parse_arguments()?;
                    let span = expression.span;
                    expression = Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(expression),
                            arguments,
                            optional: true,
                        },
                        span,
                    };
                } else {
                    let name = match &self.current.kind {
                        TokenKind::Ident(name) => name.clone(),
                        _ => {
                            return Err(self.error(format!(
                                "Expected property name, found {}",
                                self.current.description()
                            )));
                        }
                    };
                    self.advance()?;
                    let span = expression.span;
                    expression = Expr {
                        kind: ExprKind::Member {
                            object: Box::new(expression),
                            property: MemberProperty::Ident(name),
                            optional: true,
                        },
                        span,
                    };
                }
                continue;
            }
            if self.eat_punct(Punct::Dot)? {
                let name = match &self.current.kind {
                    TokenKind::Ident(name) => name.clone(),
                    _ => {
                        return Err(self.error(format!(
                            "Expected property name, found {}",
                            self.current.description()
                        )));
                    }
                };
                self.advance()?;
                let span = expression.span;
                expression = Expr {
                    kind: ExprKind::Member {
                        object: Box::new(expression),
                        property: MemberProperty::Ident(name),
                        optional: false,
                    },
                    span,
                };
                continue;
            }
            if self.eat_punct(Punct::LBracket)? {
                let previous_allow_in = self.allow_in;
                self.allow_in = true;
                let property = self.parse_expression();
                self.allow_in = previous_allow_in;
                let property = property?;
                self.expect_punct(Punct::RBracket)?;
                let span = expression.span.join(property.span);
                expression = Expr {
                    kind: ExprKind::Member {
                        object: Box::new(expression),
                        property: MemberProperty::Computed(Box::new(property)),
                        optional: false,
                    },
                    span,
                };
                continue;
            }
            if self.current.is_punct(Punct::LParen) {
                let arguments = self.parse_arguments()?;
                let span = expression.span;
                expression = Expr {
                    kind: ExprKind::Call {
                        callee: Box::new(expression),
                        arguments,
                        optional: false,
                    },
                    span,
                };
                continue;
            }
            if let TokenKind::Template(template) = &self.current.kind {
                let template = template.clone();
                self.advance()?;
                let expressions = self.parse_template_expressions(&template)?;
                let span = expression.span;
                expression = Expr {
                    kind: ExprKind::TaggedTemplate {
                        tag: Box::new(expression),
                        quasis: template.quasis,
                        expressions,
                    },
                    span,
                };
                continue;
            }
            break;
        }
        Ok(expression)
    }

    pub(super) fn parse_new(&mut self) -> Result<Expr, ParseError> {
        let span = self.span();
        self.expect_ident("new")?;
        if self.eat_punct(Punct::Dot)? {
            let name = self.binding_ident()?;
            if name == "target" {
                return Ok(Expr {
                    kind: ExprKind::Literal(Literal::Undefined),
                    span,
                });
            }
            return Err(self.error("Invalid new expression"));
        }
        let callee = if self.current.is_ident("new") {
            self.parse_new()?
        } else {
            self.parse_primary()?
        };
        let mut callee = callee;
        loop {
            if self.eat_punct(Punct::Dot)? {
                let name = self.binding_ident()?;
                let span = callee.span;
                callee = Expr {
                    kind: ExprKind::Member {
                        object: Box::new(callee),
                        property: MemberProperty::Ident(name),
                        optional: false,
                    },
                    span,
                };
            } else if self.eat_punct(Punct::LBracket)? {
                let property = self.parse_expression()?;
                self.expect_punct(Punct::RBracket)?;
                let span = callee.span.join(property.span);
                callee = Expr {
                    kind: ExprKind::Member {
                        object: Box::new(callee),
                        property: MemberProperty::Computed(Box::new(property)),
                        optional: false,
                    },
                    span,
                };
            } else {
                break;
            }
        }
        let arguments = if self.current.is_punct(Punct::LParen) {
            self.parse_arguments()?
        } else {
            Vec::new()
        };
        let span = span.join(callee.span);
        Ok(Expr {
            kind: ExprKind::New {
                callee: Box::new(callee),
                arguments,
            },
            span,
        })
    }

    pub(super) fn parse_arguments(&mut self) -> Result<Vec<Argument>, ParseError> {
        self.expect_punct(Punct::LParen)?;
        let previous_allow_in = self.allow_in;
        self.allow_in = true;
        let mut arguments = Vec::new();
        let result: Result<(), ParseError> = (|| {
            while !self.current.is_punct(Punct::RParen) {
                if self.eat_punct(Punct::Ellipsis)? {
                    arguments.push(Argument::Spread(self.parse_assignment()?));
                } else {
                    arguments.push(Argument::Expr(self.parse_assignment()?));
                }
                if !self.eat_punct(Punct::Comma)? {
                    break;
                }
            }
            self.expect_punct(Punct::RParen)?;
            Ok(())
        })();
        self.allow_in = previous_allow_in;
        result?;
        Ok(arguments)
    }

    pub(super) fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let span = self.span();
        if self.current.is_ident("function") {
            let function = self.parse_function_expression(false)?;
            return Ok(Expr {
                kind: ExprKind::Function(function),
                span,
            });
        }
        if self.current.is_ident("async")
            && let Some(next) = self.peek()
            && next.is_ident("function")
        {
            self.advance()?;
            let function = self.parse_function_expression(true)?;
            return Ok(Expr {
                kind: ExprKind::Function(function),
                span,
            });
        }
        if self.current.is_ident("class") {
            let class = self.parse_class(false)?;
            return Ok(Expr {
                kind: ExprKind::Class(class),
                span,
            });
        }
        match &self.current.kind {
            TokenKind::Number(value) => {
                let value = *value;
                self.advance()?;
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Number(value)),
                    span,
                })
            }
            TokenKind::String(value) => {
                let value = value.clone();
                self.advance()?;
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::String(value)),
                    span,
                })
            }
            TokenKind::Punct(Punct::Slash | Punct::DivAssign) => {
                let kind = self.lexer.rescan_regex(self.current.start)?;
                let TokenKind::Regex { source, flags } = kind else {
                    return Err(self.error("Invalid regular expression"));
                };
                self.current.kind = TokenKind::Regex {
                    source: source.clone(),
                    flags: flags.clone(),
                };
                self.current.end = self.lexer.offset();
                self.advance()?;
                Ok(Expr {
                    kind: ExprKind::Literal(Literal::Regex { source, flags }),
                    span,
                })
            }
            TokenKind::Template(template) => {
                let template = template.clone();
                self.advance()?;
                let expressions = self.parse_template_expressions(&template)?;
                Ok(Expr {
                    kind: ExprKind::Template {
                        quasis: template.quasis,
                        expressions,
                    },
                    span,
                })
            }
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance()?;
                match name.as_str() {
                    "true" => Ok(Expr {
                        kind: ExprKind::Literal(Literal::Bool(true)),
                        span,
                    }),
                    "false" => Ok(Expr {
                        kind: ExprKind::Literal(Literal::Bool(false)),
                        span,
                    }),
                    "null" => Ok(Expr {
                        kind: ExprKind::Literal(Literal::Null),
                        span,
                    }),
                    "this" => Ok(Expr {
                        kind: ExprKind::This,
                        span,
                    }),
                    _ => Ok(Expr {
                        kind: ExprKind::Ident(name),
                        span,
                    }),
                }
            }
            TokenKind::Punct(Punct::LParen) => {
                self.advance()?;
                let previous_allow_in = self.allow_in;
                self.allow_in = true;
                let expression = self.parse_expression();
                self.allow_in = previous_allow_in;
                let expression = expression?;
                self.expect_punct(Punct::RParen)?;
                Ok(expression)
            }
            TokenKind::Punct(Punct::LBracket) => self.parse_array_literal(),
            TokenKind::Punct(Punct::LBrace) => self.parse_object_literal(),
            _ => Err(self.error(format!("Unexpected token {}", self.current.description()))),
        }
    }

    /// QuickJS `js_parse_array_literal` (quickjs.c:26098).
    pub(super) fn parse_array_literal(&mut self) -> Result<Expr, ParseError> {
        let span = self.span();
        self.expect_punct(Punct::LBracket)?;
        let mut elements = Vec::new();
        while !self.current.is_punct(Punct::RBracket) {
            if self.eat_punct(Punct::Comma)? {
                elements.push(ArrayElement::Hole);
                continue;
            }
            if self.eat_punct(Punct::Ellipsis)? {
                elements.push(ArrayElement::Spread(self.parse_assignment()?));
            } else {
                elements.push(ArrayElement::Expr(self.parse_assignment()?));
            }
            if !self.eat_punct(Punct::Comma)? {
                break;
            }
        }
        self.expect_punct(Punct::RBracket)?;
        Ok(Expr {
            kind: ExprKind::Array(elements),
            span,
        })
    }

    /// QuickJS `js_parse_object_literal` (quickjs.c:25264).
    pub(super) fn parse_object_literal(&mut self) -> Result<Expr, ParseError> {
        let span = self.span();
        self.expect_punct(Punct::LBrace)?;
        let mut properties = Vec::new();
        while !self.current.is_punct(Punct::RBrace) {
            if self.eat_punct(Punct::Ellipsis)? {
                properties.push(ObjectProperty::Spread(self.parse_assignment()?));
                if !self.eat_punct(Punct::Comma)? {
                    break;
                }
                continue;
            }
            let mut is_async = false;
            if self.current.is_ident("async")
                && let Some(next) = self.peek()
                && (matches!(
                    next.kind,
                    TokenKind::Ident(_) | TokenKind::String(_) | TokenKind::Number(_)
                ) || next.is_punct(Punct::LBracket)
                    || next.is_punct(Punct::Star))
            {
                self.advance()?;
                is_async = true;
            }
            let mut kind = MethodKind::Method;
            if self.current.is_ident("get") {
                if let Some(next) = self.peek()
                    && !next.is_punct(Punct::LParen)
                    && !next.is_punct(Punct::Colon)
                {
                    self.advance()?;
                    kind = MethodKind::Getter;
                }
            } else if self.current.is_ident("set")
                && let Some(next) = self.peek()
                && !next.is_punct(Punct::LParen)
                && !next.is_punct(Punct::Colon)
            {
                self.advance()?;
                kind = MethodKind::Setter;
            }
            let generator = self.eat_punct(Punct::Star)?;
            let key = self.parse_property_name()?;
            if self.current.is_punct(Punct::LParen) {
                let function = self.parse_method(key.clone(), true, generator, is_async)?;
                properties.push(ObjectProperty::Method {
                    key,
                    function,
                    kind,
                });
            } else if self.eat_punct(Punct::Colon)? {
                let value = self.parse_assignment()?;
                properties.push(ObjectProperty::KeyValue { key, value });
            } else {
                let name = match &key {
                    PropertyName::Ident(name) => name.clone(),
                    _ => return Err(self.error("Invalid shorthand property")),
                };
                properties.push(ObjectProperty::Shorthand(name));
            }
            if !self.eat_punct(Punct::Comma)? {
                break;
            }
        }
        self.expect_punct(Punct::RBrace)?;
        Ok(Expr {
            kind: ExprKind::Object(properties),
            span,
        })
    }

    pub(super) fn parse_template_expressions(
        &mut self,
        template: &TemplateToken,
    ) -> Result<Vec<Expr>, ParseError> {
        let mut expressions = Vec::new();
        for source in &template.expressions {
            let mut parser = Parser::new(source)?;
            let expression = parser.parse_expression()?;
            if !matches!(parser.current.kind, TokenKind::Eof) {
                return Err(parser.error("Unexpected token in template expression"));
            }
            expressions.push(expression);
        }
        Ok(expressions)
    }
}
