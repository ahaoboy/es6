//! Statement parsing: `js_parse_statement_or_decl` (quickjs.c:29374).

use super::*;

impl<'a> Parser<'a> {
    /// Static `import` declaration. Dynamic `import()` is not part of the
    /// host-module bridge and is rejected.
    fn parse_import(&mut self, span: Span) -> Result<Stmt, ParseError> {
        self.advance()?;
        let mut specifiers = Vec::new();
        if let TokenKind::String(_) = self.current.kind {
            // Side-effect import: `import "fs";`
        } else {
            if self.eat_punct(Punct::Star)? {
                self.expect_ident("as")?;
                specifiers.push(ImportSpecifier::Namespace(self.binding_ident()?));
            } else if self.current.is_punct(Punct::LBrace) {
                self.advance()?;
                loop {
                    if self.current.is_punct(Punct::RBrace) {
                        self.advance()?;
                        break;
                    }
                    let imported = self.parse_import_name()?;
                    let local = if self.eat_ident("as")? {
                        self.binding_ident()?
                    } else {
                        imported.clone()
                    };
                    specifiers.push(ImportSpecifier::Named { imported, local });
                    if !self.eat_punct(Punct::Comma)? {
                        self.expect_punct(Punct::RBrace)?;
                        break;
                    }
                }
            } else {
                specifiers.push(ImportSpecifier::Default(self.binding_ident()?));
                if self.eat_punct(Punct::Comma)? {
                    if self.eat_punct(Punct::Star)? {
                        self.expect_ident("as")?;
                        specifiers.push(ImportSpecifier::Namespace(self.binding_ident()?));
                    } else if self.eat_punct(Punct::LBrace)? {
                        loop {
                            if self.current.is_punct(Punct::RBrace) {
                                self.advance()?;
                                break;
                            }
                            let imported = self.parse_import_name()?;
                            let local = if self.eat_ident("as")? {
                                self.binding_ident()?
                            } else {
                                imported.clone()
                            };
                            specifiers.push(ImportSpecifier::Named { imported, local });
                            if !self.eat_punct(Punct::Comma)? {
                                self.expect_punct(Punct::RBrace)?;
                                break;
                            }
                        }
                    } else {
                        return Err(self.error("Expected `*` or `{` after default import"));
                    }
                }
            }
            self.expect_ident("from")?;
        }
        let source = match &self.current.kind {
            TokenKind::String(source) => source.clone(),
            _ => return Err(self.error("Expected module specifier string")),
        };
        self.advance()?;
        self.consume_semicolon()?;
        Ok(Stmt {
            kind: StmtKind::Import(ImportDecl { source, specifiers }),
            span,
        })
    }

    fn parse_import_name(&mut self) -> Result<String, ParseError> {
        let name = match &self.current.kind {
            TokenKind::Ident(name) | TokenKind::String(name) => name.clone(),
            _ => return Err(self.error("Expected import name")),
        };
        self.advance()?;
        Ok(name)
    }

    /// ES module export declaration (`ModuleItem`, ES2022 16.2.3).
    ///
    /// The module compiler turns these into assignments on the exports
    /// namespace; the parser only records what is exported.
    fn parse_export(&mut self, span: Span) -> Result<Stmt, ParseError> {
        self.advance()?;
        let kind = if self.eat_ident("default")? {
            let expression = if self.current.is_ident("function") {
                Expr {
                    kind: ExprKind::Function(self.parse_function_declaration(false)?),
                    span: self.span(),
                }
            } else if self.current.is_ident("async")
                && self.peek().is_some_and(|t| t.is_ident("function"))
            {
                self.advance()?;
                Expr {
                    kind: ExprKind::Function(self.parse_function_declaration(true)?),
                    span: self.span(),
                }
            } else if self.current.is_ident("class") {
                Expr {
                    kind: ExprKind::Class(self.parse_class(false)?),
                    span: self.span(),
                }
            } else {
                self.parse_assignment()?
            };
            self.consume_semicolon()?;
            return Ok(Stmt {
                kind: StmtKind::Export(ExportDecl {
                    kind: ExportKind::Default(Box::new(expression)),
                    source: None,
                }),
                span,
            });
        } else if self.current.is_punct(Punct::Star) {
            self.advance()?;
            let alias = if self.eat_ident("as")? {
                Some(self.parse_import_name()?)
            } else {
                None
            };
            self.expect_ident("from")?;
            let source = self.module_specifier()?;
            self.consume_semicolon()?;
            return Ok(Stmt {
                kind: StmtKind::Export(ExportDecl {
                    kind: ExportKind::All { alias },
                    source: Some(source),
                }),
                span,
            });
        } else if self.current.is_punct(Punct::LBrace) {
            self.advance()?;
            let mut specifiers = Vec::new();
            loop {
                if self.current.is_punct(Punct::RBrace) {
                    self.advance()?;
                    break;
                }
                let local = self.parse_import_name()?;
                let exported = if self.eat_ident("as")? {
                    self.parse_import_name()?
                } else {
                    local.clone()
                };
                specifiers.push(ExportSpecifier { local, exported });
                if !self.eat_punct(Punct::Comma)? {
                    self.expect_punct(Punct::RBrace)?;
                    break;
                }
            }
            let source = if self.eat_ident("from")? {
                Some(self.module_specifier()?)
            } else {
                None
            };
            self.consume_semicolon()?;
            return Ok(Stmt {
                kind: StmtKind::Export(ExportDecl {
                    kind: ExportKind::Named(specifiers),
                    source,
                }),
                span,
            });
        } else if self.current.is_ident("var")
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
            self.advance()?;
            let declaration = self.parse_var_declarations(kind)?;
            self.consume_semicolon()?;
            StmtKind::Export(ExportDecl {
                kind: ExportKind::Declaration(Box::new(Stmt {
                    kind: StmtKind::Var(declaration),
                    span,
                })),
                source: None,
            })
        } else if self.current.is_ident("function") {
            let function = self.parse_function_declaration(false)?;
            StmtKind::Export(ExportDecl {
                kind: ExportKind::Declaration(Box::new(Stmt {
                    kind: StmtKind::Function(function),
                    span,
                })),
                source: None,
            })
        } else if self.current.is_ident("async")
            && let Some(next) = self.peek()
            && next.is_ident("function")
        {
            self.advance()?;
            let function = self.parse_function_declaration(true)?;
            StmtKind::Export(ExportDecl {
                kind: ExportKind::Declaration(Box::new(Stmt {
                    kind: StmtKind::Function(function),
                    span,
                })),
                source: None,
            })
        } else if self.current.is_ident("class") {
            let class = self.parse_class(true)?;
            StmtKind::Export(ExportDecl {
                kind: ExportKind::Declaration(Box::new(Stmt {
                    kind: StmtKind::Class(class),
                    span,
                })),
                source: None,
            })
        } else {
            return Err(self.error(format!(
                "Unexpected token after `export`: {}",
                self.current.description()
            )));
        };
        Ok(Stmt { kind, span })
    }

    fn module_specifier(&mut self) -> Result<String, ParseError> {
        let source = match &self.current.kind {
            TokenKind::String(source) => source.clone(),
            _ => return Err(self.error("Expected module specifier string")),
        };
        self.advance()?;
        Ok(source)
    }

    /// QuickJS `js_parse_statement` (quickjs.c:28756).
    pub(super) fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        let span = self.span();
        if self.current.is_ident("import") {
            return self.parse_import(span);
        }
        if self.current.is_ident("export") {
            return self.parse_export(span);
        }
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
            // The trailing semicolon of `do ... while (...)` is optional:
            // automatic semicolon insertion applies even without a line
            // terminator before the next statement.
            if self.current.is_punct(Punct::Semicolon) {
                self.advance()?;
            }
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
