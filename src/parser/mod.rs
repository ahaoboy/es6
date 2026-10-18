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
    /// `yield` parses as an operator inside generator function bodies
    /// (ES2022 15.5.2), tracked like QuickJS' func_kind checks.
    yield_allowed: bool,
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
            yield_allowed: false,
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
}

mod class;
mod expr;
mod function;
mod helpers;
mod stmt;

use helpers::*;
