//! Token types. The token set mirrors the `TOK_*` enum of QuickJS
//! (quickjs.c:22100).

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
    /// Private class element name such as `#resolve`. The value is the internal
    /// property key (`\0#resolve`) used by the object model.
    PrivateIdent(String),
    Number(f64),
    String(String),
    Template(TemplateToken),
    Regex {
        source: String,
        flags: String,
    },
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
            TokenKind::PrivateIdent(value) => {
                format!("`#{}`", value.trim_start_matches("\0#"))
            }
            TokenKind::Number(value) => format!("number `{value}`"),
            TokenKind::String(_) => "string literal".into(),
            TokenKind::Template(_) => "template literal".into(),
            TokenKind::Regex { .. } => "regular expression".into(),
            TokenKind::Punct(punct) => format!("`{}`", punct.as_str()),
        }
    }
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{message} at {line}:{column}")]
pub struct LexError {
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub offset: usize,
}
