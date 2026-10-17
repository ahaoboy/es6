use std::fmt;
use std::rc::Rc;

use crate::ast::*;
use crate::lexer::{LexError, Lexer, Punct, TemplateToken, Token, TokenKind};

#[derive(Clone, Debug)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub offset: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "{} at {}:{}", self.message, self.line, self.column)
        } else {
            f.write_str(&self.message)
        }
    }
}

impl From<LexError> for ParseError {
    fn from(value: LexError) -> Self {
        Self {
            message: value.message,
            line: value.line,
            column: value.column,
            offset: value.offset,
        }
    }
}

pub fn parse(source: &str) -> Result<Program, ParseError> {
    let mut parser = Parser::new(source)?;
    parser.parse_program()
}

struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
    previous_line: usize,
    allow_in: bool,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Result<Self, ParseError> {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token()?;
        Ok(Self {
            lexer,
            current,
            previous_line: 1,
            allow_in: true,
        })
    }

    /// QuickJS `js_parse_program` (quickjs.c:37990).
    fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut body = Vec::new();
        while !matches!(self.current.kind, TokenKind::Eof) {
            body.push(self.parse_statement()?);
        }
        Ok(Program { body })
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            message: message.into(),
            line: self.current.line,
            column: self.current.column,
            offset: self.current.start,
        }
    }

    fn advance(&mut self) -> Result<Token, ParseError> {
        self.previous_line = self.current.line;
        let previous = self.current.clone();
        self.current = self.lexer.next_token()?;
        Ok(previous)
    }

    fn peek(&mut self) -> Option<Token> {
        let mut lexer = self.lexer.clone();
        lexer.next_token().ok()
    }

    fn span(&self) -> Span {
        Span {
            start: self.current.start,
            end: self.current.end,
        }
    }

    fn eat_punct(&mut self, punct: Punct) -> Result<bool, ParseError> {
        if self.current.is_punct(punct) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect_punct(&mut self, punct: Punct) -> Result<(), ParseError> {
        if self.current.is_punct(punct) {
            self.advance()?;
            Ok(())
        } else {
            Err(self.error(format!(
                "Expected `{}`, found {}",
                punct.as_str(),
                self.current.description()
            )))
        }
    }

    fn eat_ident(&mut self, name: &str) -> Result<bool, ParseError> {
        if self.current.is_ident(name) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect_ident(&mut self, name: &str) -> Result<(), ParseError> {
        if self.current.is_ident(name) {
            self.advance()?;
            Ok(())
        } else {
            Err(self.error(format!(
                "Expected `{name}`, found {}",
                self.current.description()
            )))
        }
    }

    fn binding_ident(&mut self) -> Result<String, ParseError> {
        match &self.current.kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance()?;
                Ok(name)
            }
            _ => Err(self.error(format!(
                "Expected identifier, found {}",
                self.current.description()
            ))),
        }
    }

    fn consume_semicolon(&mut self) -> Result<(), ParseError> {
        if self.eat_punct(Punct::Semicolon)? {
            return Ok(());
        }
        if matches!(self.current.kind, TokenKind::Eof)
            || self.current.is_punct(Punct::RBrace)
            || self.current.line > self.previous_line
        {
            return Ok(());
        }
        Err(self.error(format!(
            "Expected `;`, found {}",
            self.current.description()
        )))
    }

    /// QuickJS `js_parse_statement` (quickjs.c:28756).
    fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
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

    fn parse_expression_statement(&mut self) -> Result<Stmt, ParseError> {
        let span = self.span();
        let expression = self.parse_expression()?;
        self.consume_semicolon()?;
        Ok(Stmt {
            kind: StmtKind::Expr(expression),
            span,
        })
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
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

    fn parse_var_declarations(&mut self, kind: VarKind) -> Result<VarDecl, ParseError> {
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

    fn parse_for(&mut self, span: Span) -> Result<Stmt, ParseError> {
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

    fn parse_function_declaration(&mut self, is_async: bool) -> Result<Rc<Function>, ParseError> {
        let span = self.span();
        self.expect_ident("function")?;
        let is_generator = self.eat_punct(Punct::Star)?;
        let name = if let TokenKind::Ident(name) = &self.current.kind {
            let name = name.clone();
            self.advance()?;
            Some(name)
        } else {
            None
        };
        let params = self.parse_parameters()?;
        let body = FunctionBody::Block(self.parse_block()?);
        Ok(Rc::new(Function {
            name,
            params,
            body,
            is_arrow: false,
            is_async,
            is_generator,
            span,
        }))
    }

    fn parse_function_expression(&mut self, is_async: bool) -> Result<Rc<Function>, ParseError> {
        let span = self.span();
        self.expect_ident("function")?;
        let is_generator = self.eat_punct(Punct::Star)?;
        let name = if let TokenKind::Ident(name) = &self.current.kind {
            let name = name.clone();
            self.advance()?;
            Some(name)
        } else {
            None
        };
        let params = self.parse_parameters()?;
        let body = FunctionBody::Block(self.parse_block()?);
        Ok(Rc::new(Function {
            name,
            params,
            body,
            is_arrow: false,
            is_async,
            is_generator,
            span,
        }))
    }

    fn parse_parameters(&mut self) -> Result<Vec<Pattern>, ParseError> {
        self.expect_punct(Punct::LParen)?;
        let mut params = Vec::new();
        while !self.current.is_punct(Punct::RParen) {
            if self.eat_punct(Punct::Ellipsis)? {
                let pattern = Pattern::Rest(Box::new(self.parse_binding_pattern()?));
                params.push(pattern);
                break;
            }
            params.push(self.parse_binding_pattern()?);
            if !self.eat_punct(Punct::Comma)? {
                break;
            }
        }
        self.expect_punct(Punct::RParen)?;
        Ok(params)
    }

    fn parse_binding_pattern(&mut self) -> Result<Pattern, ParseError> {
        let mut pattern = if self.eat_punct(Punct::LBracket)? {
            let mut elements = Vec::new();
            while !self.current.is_punct(Punct::RBracket) {
                if self.eat_punct(Punct::Comma)? {
                    elements.push(None);
                    continue;
                }
                if self.eat_punct(Punct::Ellipsis)? {
                    elements.push(Some(Pattern::Rest(Box::new(self.parse_binding_pattern()?))));
                    break;
                }
                elements.push(Some(self.parse_binding_pattern()?));
                if !self.eat_punct(Punct::Comma)? {
                    break;
                }
            }
            self.expect_punct(Punct::RBracket)?;
            Pattern::Array(elements)
        } else if self.eat_punct(Punct::LBrace)? {
            let mut properties = Vec::new();
            while !self.current.is_punct(Punct::RBrace) {
                if self.eat_punct(Punct::Ellipsis)? {
                    let value = self.parse_binding_pattern()?;
                    properties.push(ObjectPatternProperty {
                        key: PropertyName::Ident("...".into()),
                        value,
                    });
                    self.eat_punct(Punct::Comma)?;
                    continue;
                }
                let key = self.parse_property_name()?;
                let value = if self.eat_punct(Punct::Colon)? {
                    self.parse_binding_pattern()?
                } else {
                    match &key {
                        PropertyName::Ident(name) => Pattern::Ident(name.clone()),
                        _ => return Err(self.error("Invalid shorthand property pattern")),
                    }
                };
                let value = if self.eat_punct(Punct::Assign)? {
                    Pattern::Assign {
                        target: Box::new(value),
                        default: Box::new(self.parse_assignment()?),
                    }
                } else {
                    value
                };
                properties.push(ObjectPatternProperty { key, value });
                if !self.eat_punct(Punct::Comma)? {
                    break;
                }
            }
            self.expect_punct(Punct::RBrace)?;
            Pattern::Object(properties)
        } else {
            Pattern::Ident(self.binding_ident()?)
        };
        if self.eat_punct(Punct::Assign)? {
            pattern = Pattern::Assign {
                target: Box::new(pattern),
                default: Box::new(self.parse_assignment()?),
            };
        }
        Ok(pattern)
    }

    /// QuickJS `js_parse_class` (quickjs.c:25571).
    fn parse_class(&mut self, _declaration: bool) -> Result<Rc<ClassDecl>, ParseError> {
        let span = self.span();
        self.expect_ident("class")?;
        let name = if let TokenKind::Ident(name) = &self.current.kind {
            if !self.current.is_ident("extends") {
                let name = name.clone();
                self.advance()?;
                Some(name)
            } else {
                None
            }
        } else {
            None
        };
        let super_class = if self.eat_ident("extends")? {
            Some(Box::new(self.parse_left_hand_side()?))
        } else {
            None
        };
        self.expect_punct(Punct::LBrace)?;
        let mut elements = Vec::new();
        while !self.current.is_punct(Punct::RBrace) {
            if self.eat_punct(Punct::Semicolon)? {
                continue;
            }
            let mut is_static = false;
            if self.current.is_ident("static") {
                let next = self.peek();
                if next.as_ref().is_some_and(|token| {
                    !token.is_punct(Punct::LParen) && !token.is_punct(Punct::Assign)
                }) {
                    self.advance()?;
                    is_static = true;
                }
            }
            let mut kind = ClassElementKind::Method;
            if self.current.is_ident("get") {
                if let Some(next) = self.peek()
                    && !next.is_punct(Punct::LParen)
                {
                    self.advance()?;
                    kind = ClassElementKind::Getter;
                }
            } else if self.current.is_ident("set")
                && let Some(next) = self.peek()
                && !next.is_punct(Punct::LParen)
            {
                self.advance()?;
                kind = ClassElementKind::Setter;
            }
            let generator = self.eat_punct(Punct::Star)?;
            let key = self.parse_property_name()?;
            if self.current.is_punct(Punct::LParen) {
                let function =
                    self.parse_method(key.clone(), kind == ClassElementKind::Method, generator)?;
                elements.push(ClassElement {
                    key,
                    kind,
                    is_static,
                    function: Some(function),
                    value: None,
                });
            } else {
                let value = if self.eat_punct(Punct::Assign)? {
                    Some(self.parse_assignment()?)
                } else {
                    None
                };
                self.consume_semicolon()?;
                elements.push(ClassElement {
                    key,
                    kind: ClassElementKind::Field,
                    is_static,
                    function: None,
                    value,
                });
            }
        }
        self.expect_punct(Punct::RBrace)?;
        Ok(Rc::new(ClassDecl {
            name,
            super_class,
            elements,
            span,
        }))
    }

    fn parse_method(
        &mut self,
        key: PropertyName,
        is_method: bool,
        generator: bool,
    ) -> Result<Rc<Function>, ParseError> {
        let span = self.span();
        let name = property_name_string(&key);
        let params = self.parse_parameters()?;
        let body = FunctionBody::Block(self.parse_block()?);
        let _ = is_method;
        Ok(Rc::new(Function {
            name: Some(name),
            params,
            body,
            is_arrow: false,
            is_async: false,
            is_generator: generator,
            span,
        }))
    }

    fn parse_property_name(&mut self) -> Result<PropertyName, ParseError> {
        match &self.current.kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance()?;
                Ok(PropertyName::Ident(name))
            }
            TokenKind::String(value) => {
                let value = value.clone();
                self.advance()?;
                Ok(PropertyName::String(value))
            }
            TokenKind::Number(value) => {
                let value = *value;
                self.advance()?;
                Ok(PropertyName::Number(value))
            }
            TokenKind::Punct(Punct::LBracket) => {
                self.advance()?;
                let expression = self.parse_expression()?;
                self.expect_punct(Punct::RBracket)?;
                Ok(PropertyName::Computed(Box::new(expression)))
            }
            _ => Err(self.error(format!(
                "Expected property name, found {}",
                self.current.description()
            ))),
        }
    }

    fn parse_expression(&mut self) -> Result<Expr, ParseError> {
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
    fn parse_assignment(&mut self) -> Result<Expr, ParseError> {
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

    fn try_parse_arrow(&mut self) -> Result<Option<Expr>, ParseError> {
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

    fn try_parse_arrow_inner(
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
                params.push(self.parse_binding_pattern()?);
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

    fn parse_conditional(&mut self) -> Result<Expr, ParseError> {
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
    fn parse_binary(&mut self, minimum_precedence: u8) -> Result<Expr, ParseError> {
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
    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
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

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
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
    fn parse_left_hand_side(&mut self) -> Result<Expr, ParseError> {
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

    fn parse_new(&mut self) -> Result<Expr, ParseError> {
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

    fn parse_arguments(&mut self) -> Result<Vec<Argument>, ParseError> {
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

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
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
    fn parse_array_literal(&mut self) -> Result<Expr, ParseError> {
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
    fn parse_object_literal(&mut self) -> Result<Expr, ParseError> {
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
                let function = self.parse_method(key.clone(), true, generator)?;
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

    fn parse_template_expressions(
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

#[derive(PartialEq)]
enum BinaryKind {
    Binary(BinaryOp),
    Logical(LogicalOp),
}

fn binary_operator(token: &Token) -> Option<(u8, BinaryKind)> {
    let result = match &token.kind {
        TokenKind::Punct(punct) => match punct {
            Punct::Nullish => (1, BinaryKind::Logical(LogicalOp::Nullish)),
            Punct::Or => (2, BinaryKind::Logical(LogicalOp::Or)),
            Punct::And => (3, BinaryKind::Logical(LogicalOp::And)),
            Punct::BitOr => (4, BinaryKind::Binary(BinaryOp::BitOr)),
            Punct::BitXor => (5, BinaryKind::Binary(BinaryOp::BitXor)),
            Punct::BitAnd => (6, BinaryKind::Binary(BinaryOp::BitAnd)),
            Punct::Eq => (7, BinaryKind::Binary(BinaryOp::Eq)),
            Punct::Ne => (7, BinaryKind::Binary(BinaryOp::Ne)),
            Punct::StrictEq => (7, BinaryKind::Binary(BinaryOp::StrictEq)),
            Punct::StrictNe => (7, BinaryKind::Binary(BinaryOp::StrictNe)),
            Punct::Lt => (8, BinaryKind::Binary(BinaryOp::Lt)),
            Punct::Le => (8, BinaryKind::Binary(BinaryOp::Le)),
            Punct::Gt => (8, BinaryKind::Binary(BinaryOp::Gt)),
            Punct::Ge => (8, BinaryKind::Binary(BinaryOp::Ge)),
            Punct::Shl => (9, BinaryKind::Binary(BinaryOp::Shl)),
            Punct::Shr => (9, BinaryKind::Binary(BinaryOp::Shr)),
            Punct::UShr => (9, BinaryKind::Binary(BinaryOp::UShr)),
            Punct::Plus => (10, BinaryKind::Binary(BinaryOp::Add)),
            Punct::Minus => (10, BinaryKind::Binary(BinaryOp::Sub)),
            Punct::Star => (11, BinaryKind::Binary(BinaryOp::Mul)),
            Punct::Slash => (11, BinaryKind::Binary(BinaryOp::Div)),
            Punct::Percent => (11, BinaryKind::Binary(BinaryOp::Mod)),
            Punct::Exp => (12, BinaryKind::Binary(BinaryOp::Exp)),
            _ => return None,
        },
        TokenKind::Ident(name) if name == "in" => (8, BinaryKind::Binary(BinaryOp::In)),
        TokenKind::Ident(name) if name == "instanceof" => {
            (8, BinaryKind::Binary(BinaryOp::InstanceOf))
        }
        _ => return None,
    };
    Some(result)
}

fn is_assignment_target(expression: &Expr) -> bool {
    matches!(
        expression.kind,
        ExprKind::Ident(_) | ExprKind::Member { .. } | ExprKind::Array(_) | ExprKind::Object(_)
    )
}

fn property_name_string(name: &PropertyName) -> String {
    match name {
        PropertyName::Ident(value) | PropertyName::String(value) => value.clone(),
        PropertyName::Number(value) => {
            if value.fract() == 0.0 {
                format!("{}", *value as i64)
            } else {
                value.to_string()
            }
        }
        PropertyName::Computed(_) => "[computed]".into(),
    }
}
