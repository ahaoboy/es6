//! Statement parsing: `js_parse_statement_or_decl` (quickjs.c:29374).

use super::*;

impl<'a> Parser<'a> {
    /// QuickJS `js_parse_statement` (quickjs.c:28756).
    pub(super) fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        let span = self.span();
        if self.current.is_punct(Punct::Semicolon) {
            self.advance()?;
            return Ok(Stmt {
                kind: StmtKind::Empty,
                span,
            });
        }
        if self.current.is_punct(Punct::LBrace) {
            let body = self.parse_block()?;
            return Ok(Stmt {
                kind: StmtKind::Block(body),
                span,
            });
        }
        if self.current.is_ident("var")
            || self.current.is_ident("let")
            || self.current.is_ident("const")
        {
            let kind = if self.current.is_ident("var") {
                VarKind::Var
            } else if self.current.is_ident("let") {
                VarKind::Let
            } else {
                VarKind::Const
            };
            if kind == VarKind::Let {
                let next = self.peek();
                let declaration = next
                    .as_ref()
                    .is_some_and(|token| matches!(token.kind, TokenKind::Ident(_)))
                    || next.as_ref().is_some_and(|token| {
                        token.is_punct(Punct::LBracket) || token.is_punct(Punct::LBrace)
                    });
                if !declaration {
                    return self.parse_expression_statement();
                }
            }
            self.advance()?;
            let declaration = self.parse_var_declarations(kind)?;
            self.consume_semicolon()?;
            return Ok(Stmt {
                kind: StmtKind::Var(declaration),
                span,
            });
        }
        if self.current.is_ident("function") {
            let function = self.parse_function_declaration(false)?;
            return Ok(Stmt {
                kind: StmtKind::Function(function),
                span,
            });
        }
        if self.current.is_ident("async")
            && let Some(next) = self.peek()
            && next.is_ident("function")
        {
            self.advance()?;
            let function = self.parse_function_declaration(true)?;
            return Ok(Stmt {
                kind: StmtKind::Function(function),
                span,
            });
        }
        if self.current.is_ident("class") {
            let class = self.parse_class(true)?;
            return Ok(Stmt {
                kind: StmtKind::Class(class),
                span,
            });
        }
        if self.current.is_ident("if") {
            self.advance()?;
            self.expect_punct(Punct::LParen)?;
            let test = self.parse_expression()?;
            self.expect_punct(Punct::RParen)?;
            let consequent = Box::new(self.parse_statement()?);
            let alternate = if self.eat_ident("else")? {
                Some(Box::new(self.parse_statement()?))
            } else {
                None
            };
            return Ok(Stmt {
                kind: StmtKind::If {
                    test,
                    consequent,
                    alternate,
                },
                span,
            });
        }
        if self.current.is_ident("while") {
            self.advance()?;
            self.expect_punct(Punct::LParen)?;
            let test = self.parse_expression()?;
            self.expect_punct(Punct::RParen)?;
            let body = Box::new(self.parse_statement()?);
            return Ok(Stmt {
                kind: StmtKind::While { test, body },
                span,
            });
        }
        if self.current.is_ident("do") {
            self.advance()?;
            let body = Box::new(self.parse_statement()?);
            self.expect_ident("while")?;
            self.expect_punct(Punct::LParen)?;
            let test = self.parse_expression()?;
            self.expect_punct(Punct::RParen)?;
            self.consume_semicolon()?;
            return Ok(Stmt {
                kind: StmtKind::DoWhile { body, test },
                span,
            });
        }
        if self.current.is_ident("for") {
            return self.parse_for(span);
        }
        if self.current.is_ident("return") {
            self.advance()?;
            let argument = if self.current.is_punct(Punct::Semicolon)
                || self.current.is_punct(Punct::RBrace)
                || matches!(self.current.kind, TokenKind::Eof)
                || self.current.line > self.previous_line
            {
                None
            } else {
                Some(self.parse_expression()?)
            };
            self.consume_semicolon()?;
            return Ok(Stmt {
                kind: StmtKind::Return(argument),
                span,
            });
        }
        if self.current.is_ident("break") || self.current.is_ident("continue") {
            let is_break = self.current.is_ident("break");
            self.advance()?;
            let label = if let TokenKind::Ident(name) = &self.current.kind {
                if self.current.line == self.previous_line {
                    let name = name.clone();
                    self.advance()?;
                    Some(name)
                } else {
                    None
                }
            } else {
                None
            };
            self.consume_semicolon()?;
            return Ok(Stmt {
                kind: if is_break {
                    StmtKind::Break(label)
                } else {
                    StmtKind::Continue(label)
                },
                span,
            });
        }
        if self.current.is_ident("throw") {
            self.advance()?;
            let argument = self.parse_expression()?;
            self.consume_semicolon()?;
            return Ok(Stmt {
                kind: StmtKind::Throw(argument),
                span,
            });
        }
        if self.current.is_ident("try") {
            self.advance()?;
            let block = self.parse_block()?;
            let handler = if self.eat_ident("catch")? {
                let param = if self.eat_punct(Punct::LParen)? {
                    let pattern = self.parse_binding_pattern()?;
                    self.expect_punct(Punct::RParen)?;
                    Some(pattern)
                } else {
                    None
                };
                let body = self.parse_block()?;
                Some(CatchClause { param, body })
            } else {
                None
            };
            let finalizer = if self.eat_ident("finally")? {
                Some(self.parse_block()?)
            } else {
                None
            };
            if handler.is_none() && finalizer.is_none() {
                return Err(self.error("Missing catch or finally"));
            }
            return Ok(Stmt {
                kind: StmtKind::Try {
                    block,
                    handler,
                    finalizer,
                },
                span,
            });
        }
        if self.current.is_ident("switch") {
            self.advance()?;
            self.expect_punct(Punct::LParen)?;
            let discriminant = self.parse_expression()?;
            self.expect_punct(Punct::RParen)?;
            self.expect_punct(Punct::LBrace)?;
            let mut cases = Vec::new();
            while !self.current.is_punct(Punct::RBrace) {
                let test = if self.eat_ident("case")? {
                    let test = self.parse_expression()?;
                    self.expect_punct(Punct::Colon)?;
                    Some(test)
                } else if self.eat_ident("default")? {
                    self.expect_punct(Punct::Colon)?;
                    None
                } else {
                    return Err(self.error("Expected `case` or `default`"));
                };
                let mut body = Vec::new();
                while !self.current.is_punct(Punct::RBrace)
                    && !self.current.is_ident("case")
                    && !self.current.is_ident("default")
                {
                    body.push(self.parse_statement()?);
                }
                cases.push(SwitchCase { test, body });
            }
            self.expect_punct(Punct::RBrace)?;
            return Ok(Stmt {
                kind: StmtKind::Switch {
                    discriminant,
                    cases,
                },
                span,
            });
        }
        if self.current.is_ident("debugger") {
            self.advance()?;
            self.consume_semicolon()?;
            return Ok(Stmt {
                kind: StmtKind::Debugger,
                span,
            });
        }
        if let TokenKind::Ident(name) = &self.current.kind {
            let name = name.clone();
            if let Some(next) = self.peek()
                && next.is_punct(Punct::Colon)
            {
                self.advance()?;
                self.advance()?;
                let body = Box::new(self.parse_statement()?);
                return Ok(Stmt {
                    kind: StmtKind::Labeled { label: name, body },
                    span,
                });
            }
        }
        self.parse_expression_statement()
    }

    pub(super) fn parse_expression_statement(&mut self) -> Result<Stmt, ParseError> {
        let span = self.span();
        let expression = self.parse_expression()?;
        self.consume_semicolon()?;
        Ok(Stmt {
            kind: StmtKind::Expr(expression),
            span,
        })
    }

    pub(super) fn parse_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect_punct(Punct::LBrace)?;
        let mut body = Vec::new();
        while !self.current.is_punct(Punct::RBrace) {
            if matches!(self.current.kind, TokenKind::Eof) {
                return Err(self.error("Unterminated block"));
            }
            body.push(self.parse_statement()?);
        }
        self.expect_punct(Punct::RBrace)?;
        Ok(body)
    }

    pub(super) fn parse_var_declarations(&mut self, kind: VarKind) -> Result<VarDecl, ParseError> {
        let mut declarations = Vec::new();
        loop {
            let pattern = self.parse_binding_pattern()?;
            let init = if self.eat_punct(Punct::Assign)? {
                Some(self.parse_assignment()?)
            } else {
                None
            };
            declarations.push(VarDeclarator { pattern, init });
            if !self.eat_punct(Punct::Comma)? {
                break;
            }
        }
        Ok(VarDecl { kind, declarations })
    }

    pub(super) fn parse_for(&mut self, span: Span) -> Result<Stmt, ParseError> {
        self.advance()?;
        self.eat_ident("await")?;
        self.expect_punct(Punct::LParen)?;
        if self.current.is_punct(Punct::Semicolon) {
            self.advance()?;
            let test = if self.current.is_punct(Punct::Semicolon) {
                None
            } else {
                Some(self.parse_expression()?)
            };
            self.expect_punct(Punct::Semicolon)?;
            let update = if self.current.is_punct(Punct::RParen) {
                None
            } else {
                Some(self.parse_expression()?)
            };
            self.expect_punct(Punct::RParen)?;
            let body = Box::new(self.parse_statement()?);
            return Ok(Stmt {
                kind: StmtKind::For {
                    init: None,
                    test,
                    update,
                    body,
                },
                span,
            });
        }
        let mut declaration_kind = None;
        if self.current.is_ident("var")
            || self.current.is_ident("let")
            || self.current.is_ident("const")
        {
            declaration_kind = Some(if self.current.is_ident("var") {
                VarKind::Var
            } else if self.current.is_ident("let") {
                VarKind::Let
            } else {
                VarKind::Const
            });
            self.advance()?;
        }
        let init = if let Some(kind) = declaration_kind {
            let declaration = self.parse_var_declarations(kind)?;
            if self.eat_ident("in")? {
                let left = ForInit::Var(declaration);
                let right = self.parse_expression()?;
                self.expect_punct(Punct::RParen)?;
                let body = Box::new(self.parse_statement()?);
                return Ok(Stmt {
                    kind: StmtKind::ForIn { left, right, body },
                    span,
                });
            }
            if self.eat_ident("of")? {
                let left = ForInit::Var(declaration);
                let right = self.parse_expression()?;
                self.expect_punct(Punct::RParen)?;
                let body = Box::new(self.parse_statement()?);
                return Ok(Stmt {
                    kind: StmtKind::ForOf { left, right, body },
                    span,
                });
            }
            ForInit::Var(declaration)
        } else {
            let previous_allow_in = self.allow_in;
            self.allow_in = false;
            let expression = self.parse_expression()?;
            self.allow_in = previous_allow_in;
            if self.eat_ident("in")? {
                let left = ForInit::Expr(expression);
                let right = self.parse_expression()?;
                self.expect_punct(Punct::RParen)?;
                let body = Box::new(self.parse_statement()?);
                return Ok(Stmt {
                    kind: StmtKind::ForIn { left, right, body },
                    span,
                });
            }
            if self.eat_ident("of")? {
                let left = ForInit::Expr(expression);
                let right = self.parse_expression()?;
                self.expect_punct(Punct::RParen)?;
                let body = Box::new(self.parse_statement()?);
                return Ok(Stmt {
                    kind: StmtKind::ForOf { left, right, body },
                    span,
                });
            }
            ForInit::Expr(expression)
        };
        self.expect_punct(Punct::Semicolon)?;
        let test = if self.current.is_punct(Punct::Semicolon) {
            None
        } else {
            Some(self.parse_expression()?)
        };
        self.expect_punct(Punct::Semicolon)?;
        let update = if self.current.is_punct(Punct::RParen) {
            None
        } else {
            Some(self.parse_expression()?)
        };
        self.expect_punct(Punct::RParen)?;
        let body = Box::new(self.parse_statement()?);
        Ok(Stmt {
            kind: StmtKind::For {
                init: Some(init),
                test,
                update,
                body,
            },
            span,
        })
    }
}
