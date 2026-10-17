use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Punct {
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Dot,
    Ellipsis,
    Semicolon,
    Comma,
    Question,
    QuestionDot,
    Colon,
    Arrow,
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
    ExpAssign,
    AndAssign,
    OrAssign,
    NullishAssign,
    ShlAssign,
    ShrAssign,
    UShrAssign,
    BitAndAssign,
    BitOrAssign,
    BitXorAssign,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Exp,
    And,
    Or,
    Nullish,
    Not,
    BitNot,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    Lt,
    Le,
    Gt,
    Ge,
    Inc,
    Dec,
}

impl Punct {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LBrace => "{",
            Self::RBrace => "}",
            Self::LParen => "(",
            Self::RParen => ")",
            Self::LBracket => "[",
            Self::RBracket => "]",
            Self::Dot => ".",
            Self::Ellipsis => "...",
            Self::Semicolon => ";",
            Self::Comma => ",",
            Self::Question => "?",
            Self::QuestionDot => "?.",
            Self::Colon => ":",
            Self::Arrow => "=>",
            Self::Assign => "=",
            Self::AddAssign => "+=",
            Self::SubAssign => "-=",
            Self::MulAssign => "*=",
            Self::DivAssign => "/=",
            Self::ModAssign => "%=",
            Self::ExpAssign => "**=",
            Self::AndAssign => "&&=",
            Self::OrAssign => "||=",
            Self::NullishAssign => "??=",
            Self::ShlAssign => "<<=",
            Self::ShrAssign => ">>=",
            Self::UShrAssign => ">>>=",
            Self::BitAndAssign => "&=",
            Self::BitOrAssign => "|=",
            Self::BitXorAssign => "^=",
            Self::Plus => "+",
            Self::Minus => "-",
            Self::Star => "*",
            Self::Slash => "/",
            Self::Percent => "%",
            Self::Exp => "**",
            Self::And => "&&",
            Self::Or => "||",
            Self::Nullish => "??",
            Self::Not => "!",
            Self::BitNot => "~",
            Self::BitAnd => "&",
            Self::BitOr => "|",
            Self::BitXor => "^",
            Self::Shl => "<<",
            Self::Shr => ">>",
            Self::UShr => ">>>",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::StrictEq => "===",
            Self::StrictNe => "!==",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Inc => "++",
            Self::Dec => "--",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Eof,
    Ident(String),
    Number(f64),
    String(String),
    Template(TemplateToken),
    Regex { source: String, flags: String },
    Punct(Punct),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TemplateToken {
    pub quasis: Vec<String>,
    pub expressions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
}

impl Token {
    pub fn is_punct(&self, punct: Punct) -> bool {
        self.kind == TokenKind::Punct(punct)
    }

    pub fn is_ident(&self, name: &str) -> bool {
        matches!(&self.kind, TokenKind::Ident(value) if value == name)
    }

    pub fn ident(&self) -> Option<&str> {
        match &self.kind {
            TokenKind::Ident(value) => Some(value),
            _ => None,
        }
    }

    pub fn description(&self) -> String {
        match &self.kind {
            TokenKind::Eof => "end of input".into(),
            TokenKind::Ident(value) => format!("`{value}`"),
            TokenKind::Number(value) => format!("number `{value}`"),
            TokenKind::String(_) => "string literal".into(),
            TokenKind::Template(_) => "template literal".into(),
            TokenKind::Regex { .. } => "regular expression".into(),
            TokenKind::Punct(punct) => format!("`{}`", punct.as_str()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LexError {
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub offset: usize,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}:{}", self.message, self.line, self.column)
    }
}

/// Byte-oriented scanner. Mirrors QuickJS' `next_token` dispatch table
/// (`quickjs.c:22607`), with string/template/regexp readers corresponding to
/// `js_parse_string` (22729), `js_parse_template_part` (22660) and
/// `js_parse_regexp` (22873).
#[derive(Clone)]
pub struct Lexer<'a> {
    source: &'a str,
    pos: usize,
    line: usize,
    column: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            pos: 0,
            line: 1,
            column: 1,
        }
    }

    pub fn offset(&self) -> usize {
        self.pos
    }

    pub fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_whitespace_and_comments()?;
        let start = self.pos;
        let line = self.line;
        let column = self.column;
        let Some(ch) = self.peek() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                start,
                end: start,
                line,
                column,
            });
        };
        let kind = match ch {
            '"' | '\'' => TokenKind::String(self.read_string(ch)?),
            '`' => TokenKind::Template(self.read_template()?),
            '0'..='9' => TokenKind::Number(self.read_number()?),
            '.' if self.peek_n(1).is_some_and(|c| c.is_ascii_digit()) => {
                TokenKind::Number(self.read_number()?)
            }
            _ if is_ident_start(ch) || ch == '\\' => TokenKind::Ident(self.read_identifier()?),
            _ => TokenKind::Punct(self.read_punct()?),
        };
        Ok(Token {
            kind,
            start,
            end: self.pos,
            line,
            column,
        })
    }

    /// Re-scans the current `/` token as a regular expression. The parser uses
    /// this when a slash appears in prefix position.
    pub fn rescan_regex(&mut self, start: usize) -> Result<TokenKind, LexError> {
        self.pos = start;
        self.line = 1;
        self.column = 1;
        for (index, ch) in self.source[..start].char_indices() {
            if ch == '\n' {
                self.line += 1;
                self.column = 1;
                let _ = index;
            } else {
                self.column += 1;
            }
        }
        self.bump();
        let mut source = String::new();
        let mut in_class = false;
        let mut escaped = false;
        loop {
            let Some(ch) = self.peek() else {
                return Err(self.error("Unterminated regular expression"));
            };
            if ch == '\n' {
                return Err(self.error("Unterminated regular expression"));
            }
            self.bump();
            if escaped {
                source.push('\\');
                source.push(ch);
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '[' => {
                    in_class = true;
                    source.push(ch);
                }
                ']' => {
                    in_class = false;
                    source.push(ch);
                }
                '/' if !in_class => break,
                _ => source.push(ch),
            }
        }
        let mut flags = String::new();
        while let Some(ch) = self.peek() {
            if is_ident_continue(ch) {
                flags.push(ch);
                self.bump();
            } else {
                break;
            }
        }
        Ok(TokenKind::Regex { source, flags })
    }

    fn peek(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    fn peek_n(&self, n: usize) -> Option<char> {
        self.source[self.pos..].chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn error(&self, message: impl Into<String>) -> LexError {
        LexError {
            message: message.into(),
            line: self.line,
            column: self.column,
            offset: self.pos,
        }
    }

    fn skip_whitespace_and_comments(&mut self) -> Result<(), LexError> {
        loop {
            while let Some(ch) = self.peek() {
                if ch.is_whitespace() {
                    self.bump();
                } else {
                    break;
                }
            }
            if self.peek() == Some('/') && self.peek_n(1) == Some('/') {
                while let Some(ch) = self.bump() {
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            if self.peek() == Some('/') && self.peek_n(1) == Some('*') {
                self.bump();
                self.bump();
                let mut closed = false;
                while let Some(ch) = self.bump() {
                    if ch == '*' && self.peek() == Some('/') {
                        self.bump();
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err(self.error("Unterminated comment"));
                }
                continue;
            }
            return Ok(());
        }
    }

    fn read_identifier(&mut self) -> Result<String, LexError> {
        let mut result = String::new();
        let mut first = true;
        while let Some(ch) = self.peek() {
            if ch == '\\' {
                self.bump();
                if self.peek() != Some('u') {
                    return Err(self.error("Invalid identifier escape"));
                }
                self.bump();
                let value = if self.peek() == Some('{') {
                    self.bump();
                    let mut hex = String::new();
                    while let Some(ch) = self.peek() {
                        if ch == '}' {
                            break;
                        }
                        hex.push(ch);
                        self.bump();
                    }
                    if self.peek() != Some('}') {
                        return Err(self.error("Unterminated unicode escape"));
                    }
                    self.bump();
                    u32::from_str_radix(&hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| self.error("Invalid unicode escape"))?
                } else {
                    let hex = self.read_hex(4)?;
                    char::from_u32(hex).ok_or_else(|| self.error("Invalid unicode escape"))?
                };
                if (first && !is_ident_start(value)) || (!first && !is_ident_continue(value)) {
                    return Err(self.error("Invalid identifier escape"));
                }
                result.push(value);
            } else if (first && is_ident_start(ch)) || (!first && is_ident_continue(ch)) {
                result.push(ch);
                self.bump();
            } else {
                break;
            }
            first = false;
        }
        Ok(result)
    }

    fn read_number(&mut self) -> Result<f64, LexError> {
        let start = self.pos;
        if self.peek() == Some('0') {
            match self.peek_n(1) {
                Some('x' | 'X') => {
                    self.bump();
                    self.bump();
                    return self.read_radix(16, start);
                }
                Some('b' | 'B') => {
                    self.bump();
                    self.bump();
                    return self.read_radix(2, start);
                }
                Some('o' | 'O') => {
                    self.bump();
                    self.bump();
                    return self.read_radix(8, start);
                }
                _ => {}
            }
        }
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() || ch == '_' {
                self.bump();
            } else {
                break;
            }
        }
        if self.peek() == Some('.') {
            self.bump();
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() || ch == '_' {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            let mut digits = 0;
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() || ch == '_' {
                    self.bump();
                    digits += 1;
                } else {
                    break;
                }
            }
            if digits == 0 {
                return Err(self.error("Invalid number"));
            }
        }
        let raw = self.source[start..self.pos].replace('_', "");
        raw.parse()
            .map_err(|_| self.error("Invalid number literal"))
    }

    fn read_radix(&mut self, radix: u32, start: usize) -> Result<f64, LexError> {
        while let Some(ch) = self.peek() {
            if ch.is_digit(radix) || ch == '_' {
                self.bump();
            } else {
                break;
            }
        }
        let raw = self.source[start + 2..self.pos].replace('_', "");
        if raw.is_empty() {
            return Err(self.error("Invalid number literal"));
        }
        u64::from_str_radix(&raw, radix)
            .map(|value| value as f64)
            .map_err(|_| self.error("Invalid number literal"))
    }

    fn read_hex(&mut self, count: usize) -> Result<u32, LexError> {
        let mut value = 0;
        for _ in 0..count {
            let Some(ch) = self.peek() else {
                return Err(self.error("Invalid hexadecimal escape"));
            };
            let digit = ch
                .to_digit(16)
                .ok_or_else(|| self.error("Invalid hexadecimal escape"))?;
            value = value * 16 + digit;
            self.bump();
        }
        Ok(value)
    }

    fn read_string(&mut self, quote: char) -> Result<String, LexError> {
        self.bump();
        let mut result = String::new();
        loop {
            let Some(ch) = self.bump() else {
                return Err(self.error("Unterminated string"));
            };
            if ch == quote {
                break;
            }
            if ch == '\n' {
                return Err(self.error("Unterminated string"));
            }
            if ch == '\\' {
                self.read_escape(&mut result)?;
            } else {
                result.push(ch);
            }
        }
        Ok(result)
    }

    fn read_escape(&mut self, result: &mut String) -> Result<(), LexError> {
        let Some(ch) = self.bump() else {
            return Err(self.error("Invalid escape sequence"));
        };
        match ch {
            'n' => result.push('\n'),
            'r' => result.push('\r'),
            't' => result.push('\t'),
            'b' => result.push('\u{0008}'),
            'f' => result.push('\u{000C}'),
            'v' => result.push('\u{000B}'),
            '0' => result.push('\0'),
            '\n' => {}
            '\r' => {
                if self.peek() == Some('\n') {
                    self.bump();
                }
            }
            'x' => {
                let value = self.read_hex(2)?;
                result.push(char::from_u32(value).unwrap_or('\u{FFFD}'));
            }
            'u' => {
                let value = if self.peek() == Some('{') {
                    self.bump();
                    let mut hex = String::new();
                    while let Some(ch) = self.peek() {
                        if ch == '}' {
                            break;
                        }
                        hex.push(ch);
                        self.bump();
                    }
                    if self.peek() != Some('}') {
                        return Err(self.error("Unterminated unicode escape"));
                    }
                    self.bump();
                    u32::from_str_radix(&hex, 16)
                        .map_err(|_| self.error("Invalid unicode escape"))?
                } else {
                    self.read_hex(4)?
                };
                result.push(char::from_u32(value).unwrap_or('\u{FFFD}'));
            }
            other => result.push(other),
        }
        Ok(())
    }

    fn read_template(&mut self) -> Result<TemplateToken, LexError> {
        self.bump();
        let mut quasis = Vec::new();
        let mut expressions = Vec::new();
        let mut cooked = String::new();
        loop {
            let Some(ch) = self.peek() else {
                return Err(self.error("Unterminated template literal"));
            };
            match ch {
                '`' => {
                    self.bump();
                    quasis.push(cooked);
                    break;
                }
                '$' if self.peek_n(1) == Some('{') => {
                    self.bump();
                    self.bump();
                    quasis.push(std::mem::take(&mut cooked));
                    let start = self.pos;
                    self.skip_template_expression()?;
                    let end = self.pos.saturating_sub(1);
                    expressions.push(self.source[start..end].to_owned());
                }
                '\\' => {
                    self.bump();
                    self.read_escape(&mut cooked)?;
                }
                '\r' => {
                    self.bump();
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    cooked.push('\n');
                }
                _ => {
                    cooked.push(ch);
                    self.bump();
                }
            }
        }
        Ok(TemplateToken {
            quasis,
            expressions,
        })
    }

    fn skip_template_expression(&mut self) -> Result<(), LexError> {
        let mut depth = 1usize;
        while depth > 0 {
            let Some(ch) = self.bump() else {
                return Err(self.error("Unterminated template expression"));
            };
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                '\'' | '"' => self.skip_quoted(ch)?,
                '`' => self.skip_nested_template()?,
                '/' if self.peek() == Some('/') => {
                    while let Some(ch) = self.bump() {
                        if ch == '\n' {
                            break;
                        }
                    }
                }
                '/' if self.peek() == Some('*') => {
                    self.bump();
                    let mut closed = false;
                    while let Some(ch) = self.bump() {
                        if ch == '*' && self.peek() == Some('/') {
                            self.bump();
                            closed = true;
                            break;
                        }
                    }
                    if !closed {
                        return Err(self.error("Unterminated comment"));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn skip_quoted(&mut self, quote: char) -> Result<(), LexError> {
        loop {
            let Some(ch) = self.bump() else {
                return Err(self.error("Unterminated string"));
            };
            if ch == '\\' {
                self.bump();
            } else if ch == quote {
                return Ok(());
            }
        }
    }

    fn skip_nested_template(&mut self) -> Result<(), LexError> {
        loop {
            let Some(ch) = self.bump() else {
                return Err(self.error("Unterminated template literal"));
            };
            match ch {
                '\\' => {
                    self.bump();
                }
                '`' => return Ok(()),
                '$' if self.peek() == Some('{') => {
                    self.bump();
                    self.skip_template_expression()?;
                }
                _ => {}
            }
        }
    }

    fn read_punct(&mut self) -> Result<Punct, LexError> {
        const PUNCTS: &[(&str, Punct)] = &[
            (">>>=", Punct::UShrAssign),
            ("===", Punct::StrictEq),
            ("!==", Punct::StrictNe),
            (">>>", Punct::UShr),
            ("...", Punct::Ellipsis),
            ("**=", Punct::ExpAssign),
            ("&&=", Punct::AndAssign),
            ("||=", Punct::OrAssign),
            ("??=", Punct::NullishAssign),
            ("<<=", Punct::ShlAssign),
            (">>=", Punct::ShrAssign),
            ("?.", Punct::QuestionDot),
            ("=>", Punct::Arrow),
            ("==", Punct::Eq),
            ("!=", Punct::Ne),
            ("<=", Punct::Le),
            (">=", Punct::Ge),
            ("++", Punct::Inc),
            ("--", Punct::Dec),
            ("&&", Punct::And),
            ("||", Punct::Or),
            ("??", Punct::Nullish),
            ("**", Punct::Exp),
            ("<<", Punct::Shl),
            (">>", Punct::Shr),
            ("+=", Punct::AddAssign),
            ("-=", Punct::SubAssign),
            ("*=", Punct::MulAssign),
            ("/=", Punct::DivAssign),
            ("%=", Punct::ModAssign),
            ("&=", Punct::BitAndAssign),
            ("|=", Punct::BitOrAssign),
            ("^=", Punct::BitXorAssign),
            ("{", Punct::LBrace),
            ("}", Punct::RBrace),
            ("(", Punct::LParen),
            (")", Punct::RParen),
            ("[", Punct::LBracket),
            ("]", Punct::RBracket),
            (".", Punct::Dot),
            (";", Punct::Semicolon),
            (",", Punct::Comma),
            ("?", Punct::Question),
            (":", Punct::Colon),
            ("=", Punct::Assign),
            ("+", Punct::Plus),
            ("-", Punct::Minus),
            ("*", Punct::Star),
            ("/", Punct::Slash),
            ("%", Punct::Percent),
            ("!", Punct::Not),
            ("~", Punct::BitNot),
            ("&", Punct::BitAnd),
            ("|", Punct::BitOr),
            ("^", Punct::BitXor),
            ("<", Punct::Lt),
            (">", Punct::Gt),
        ];
        let remaining = &self.source[self.pos..];
        for (text, punct) in PUNCTS {
            if remaining.starts_with(text) {
                for _ in 0..text.chars().count() {
                    self.bump();
                }
                return Ok(*punct);
            }
        }
        Err(self.error(format!("Unexpected character `{remaining}`")))
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '$' || ch == '_' || unicode_ident::is_xid_start(ch)
}

fn is_ident_continue(ch: char) -> bool {
    ch == '$'
        || ch == '_'
        || ch == '\u{200C}'
        || ch == '\u{200D}'
        || unicode_ident::is_xid_continue(ch)
}
