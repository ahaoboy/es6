use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn join(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

pub struct Program {
    pub body: Vec<Stmt>,
}

#[derive(Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone)]
pub enum StmtKind {
    Empty,
    Expr(Expr),
    Block(Vec<Stmt>),
    Var(VarDecl),
    Function(Rc<Function>),
    Class(Rc<ClassDecl>),
    If {
        test: Expr,
        consequent: Box<Stmt>,
        alternate: Option<Box<Stmt>>,
    },
    While {
        test: Expr,
        body: Box<Stmt>,
    },
    DoWhile {
        body: Box<Stmt>,
        test: Expr,
    },
    For {
        init: Option<ForInit>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Box<Stmt>,
    },
    ForIn {
        left: ForInit,
        right: Expr,
        body: Box<Stmt>,
    },
    ForOf {
        left: ForInit,
        right: Expr,
        body: Box<Stmt>,
    },
    Return(Option<Expr>),
    Break(Option<String>),
    Continue(Option<String>),
    Throw(Expr),
    Try {
        block: Vec<Stmt>,
        handler: Option<CatchClause>,
        finalizer: Option<Vec<Stmt>>,
    },
    Switch {
        discriminant: Expr,
        cases: Vec<SwitchCase>,
    },
    Labeled {
        label: String,
        body: Box<Stmt>,
    },
    Debugger,
}

#[derive(Clone)]
pub struct CatchClause {
    pub param: Option<Pattern>,
    pub body: Vec<Stmt>,
}

#[derive(Clone)]
pub struct SwitchCase {
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Clone)]
pub enum ForInit {
    Var(VarDecl),
    Expr(Expr),
}

#[derive(Clone)]
pub struct VarDecl {
    pub kind: VarKind,
    pub declarations: Vec<VarDeclarator>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarKind {
    Var,
    Let,
    Const,
}

#[derive(Clone)]
pub struct VarDeclarator {
    pub pattern: Pattern,
    pub init: Option<Expr>,
}

#[derive(Clone)]
pub enum Pattern {
    Ident(String),
    Array(Vec<Option<Pattern>>),
    Object(Vec<ObjectPatternProperty>),
    Rest(Box<Pattern>),
    Assign {
        target: Box<Pattern>,
        default: Box<Expr>,
    },
}

#[derive(Clone)]
pub struct ObjectPatternProperty {
    pub key: PropertyName,
    pub value: Pattern,
}

#[derive(Clone)]
pub enum PropertyName {
    Ident(String),
    String(String),
    Number(f64),
    Computed(Box<Expr>),
}

pub struct Function {
    pub name: Option<String>,
    pub params: Vec<Pattern>,
    pub body: FunctionBody,
    pub is_arrow: bool,
    pub is_async: bool,
    pub is_generator: bool,
    pub span: Span,
}

pub enum FunctionBody {
    Block(Vec<Stmt>),
    Expr(Box<Expr>),
}

pub struct ClassDecl {
    pub name: Option<String>,
    pub super_class: Option<Box<Expr>>,
    pub elements: Vec<ClassElement>,
    pub span: Span,
}

#[derive(Clone)]
pub struct ClassElement {
    pub key: PropertyName,
    pub kind: ClassElementKind,
    pub is_static: bool,
    pub function: Option<Rc<Function>>,
    pub value: Option<Expr>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClassElementKind {
    Method,
    Getter,
    Setter,
    Field,
}

#[derive(Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone)]
pub enum ExprKind {
    Literal(Literal),
    Ident(String),
    This,
    Array(Vec<ArrayElement>),
    Object(Vec<ObjectProperty>),
    Function(Rc<Function>),
    Arrow(Rc<Function>),
    Class(Rc<ClassDecl>),
    Unary {
        op: UnaryOp,
        argument: Box<Expr>,
    },
    Update {
        op: UpdateOp,
        prefix: bool,
        argument: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Logical {
        op: LogicalOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Assign {
        op: AssignOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    Conditional {
        test: Box<Expr>,
        consequent: Box<Expr>,
        alternate: Box<Expr>,
    },
    Sequence(Vec<Expr>),
    Member {
        object: Box<Expr>,
        property: MemberProperty,
        optional: bool,
    },
    Call {
        callee: Box<Expr>,
        arguments: Vec<Argument>,
        optional: bool,
    },
    New {
        callee: Box<Expr>,
        arguments: Vec<Argument>,
    },
    Template {
        quasis: Vec<String>,
        expressions: Vec<Expr>,
    },
    TaggedTemplate {
        tag: Box<Expr>,
        quasis: Vec<String>,
        expressions: Vec<Expr>,
    },
    Spread(Box<Expr>),
    Await(Box<Expr>),
    Yield(Option<Box<Expr>>),
}

#[derive(Clone)]
pub enum ArrayElement {
    Hole,
    Expr(Expr),
    Spread(Expr),
}

#[derive(Clone)]
pub enum Argument {
    Expr(Expr),
    Spread(Expr),
}

#[derive(Clone)]
pub enum MemberProperty {
    Ident(String),
    Computed(Box<Expr>),
}

#[derive(Clone)]
pub enum ObjectProperty {
    KeyValue {
        key: PropertyName,
        value: Expr,
    },
    Shorthand(String),
    Method {
        key: PropertyName,
        function: Rc<Function>,
        kind: MethodKind,
    },
    Spread(Expr),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    Method,
    Getter,
    Setter,
}

#[derive(Clone)]
pub enum Literal {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Regex { source: String, flags: String },
}

#[derive(Clone, Copy)]
pub enum UnaryOp {
    Plus,
    Minus,
    Not,
    BitNot,
    TypeOf,
    Void,
    Delete,
}

#[derive(Clone, Copy)]
pub enum UpdateOp {
    Increment,
    Decrement,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
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
    In,
    InstanceOf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogicalOp {
    And,
    Or,
    Nullish,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    And,
    Or,
    Nullish,
}

impl BinaryOp {
    pub fn assign_op(self) -> Option<AssignOp> {
        Some(match self {
            Self::Add => AssignOp::Add,
            Self::Sub => AssignOp::Sub,
            Self::Mul => AssignOp::Mul,
            Self::Div => AssignOp::Div,
            Self::Mod => AssignOp::Mod,
            Self::Exp => AssignOp::Exp,
            Self::BitAnd => AssignOp::BitAnd,
            Self::BitOr => AssignOp::BitOr,
            Self::BitXor => AssignOp::BitXor,
            Self::Shl => AssignOp::Shl,
            Self::Shr => AssignOp::Shr,
            Self::UShr => AssignOp::UShr,
            _ => return None,
        })
    }
}
