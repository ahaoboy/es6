use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
pub enum Constant {
    Number(f64),
    String(Rc<str>),
    Function(Rc<FunctionDef>),
    Class(Rc<ClassDef>),
    Strings(Rc<[Rc<str>]>),
    Regex { source: Rc<str>, flags: Rc<str> },
}

pub struct ClassDef {
    pub name: Rc<str>,
    pub constructor: Option<Rc<FunctionDef>>,
    pub methods: Vec<ClassMethod>,
    pub instance_init: Option<Rc<FunctionDef>>,
    pub static_init: Option<Rc<FunctionDef>>,
    pub has_super: bool,
}

pub struct ClassMethod {
    pub name: Rc<str>,
    pub function: Rc<FunctionDef>,
    pub is_static: bool,
    pub is_getter: bool,
    pub is_setter: bool,
}

/// Compiled function body. QuickJS stores the same information in
/// `JSFunctionBytecode` (quickjs.c:17200 area) with `byte_code_buf`,
/// `cpool` and `closure_var`.
pub struct FunctionDef {
    pub name: Rc<str>,
    pub code: Vec<Op>,
    /// Source byte offset for each opcode, used for diagnostics.
    pub positions: Vec<u32>,
    pub constants: Vec<Constant>,
    pub prop_caches: Vec<RefCell<crate::object::PropCache>>,
    pub env_size: u16,
    /// True when this function creates closures/classes that can capture its
    /// frame. QuickJS likewise only materializes `var_refs` for captured
    /// variables; uncaptured locals stay in stack slots (`JSStackFrame`).
    pub use_env: bool,
    pub length: u32,
    pub is_arrow: bool,
    pub is_async: bool,
    pub strict: bool,
    pub source: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
}

/// Stack-machine bytecode modeled after QuickJS' opcode set
/// (`quickjs-opcode.h`, `DEF(...)` table). Like QuickJS' short opcodes, the
/// common cases avoid atom/constant indirection; operands are immediates.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    Nop,
    Undefined,
    Null,
    True,
    False,
    This,
    PushConst(u32),
    Pop,
    Dup,
    Dup2,
    Swap,

    LoadArg(u16),
    RestArgs(u16),
    Arguments,
    GetVar {
        depth: u16,
        slot: u16,
    },
    SetVar {
        depth: u16,
        slot: u16,
    },
    GetGlobal(u32),
    SetGlobal(u32),
    DeleteGlobal(u32),

    GetProp,
    SetProp,
    SetPropKeep,
    DeleteProp,
    /// `object[index]` where both operands are resolved locals.
    GetIndexVar {
        object_depth: u16,
        object_slot: u16,
        index_depth: u16,
        index_slot: u16,
    },
    /// `object[index] = value` where both targets are resolved locals.
    SetIndexVar {
        object_depth: u16,
        object_slot: u16,
        index_depth: u16,
        index_slot: u16,
    },
    /// `object[++index]` / `object[index++]`.
    GetIndexUpdate {
        object_depth: u16,
        object_slot: u16,
        index_depth: u16,
        index_slot: u16,
        delta: i8,
        prefix: bool,
    },
    GetPropConst {
        key: u32,
        cache: u32,
    },
    SetPropConst {
        key: u32,
        cache: u32,
    },
    SetPropConstKeep {
        key: u32,
        cache: u32,
    },
    DefineAccessor {
        is_getter: bool,
    },

    NewObject,
    /// Object literal with static keys: `count` values are on the stack.
    NewObjectFrom {
        keys: u32,
        count: u16,
    },
    NewArray(u32),
    ArrayPush,
    ArraySpread,
    ObjectSpread,
    ArrayRest(u16),
    ObjectRest(u32),

    Call(u16),
    CallMethod(u16),
    CallSpread,
    CallMethodSpread,
    CallSuperMethod(u16),
    CallSuperMethodSpread,
    New(u16),
    NewSpread,
    SuperGetProp,

    Return,
    ReturnUndefined,
    Throw,
    Jump(usize),
    JumpIfFalse(usize),
    JumpIfTrue(usize),
    JumpIfNotUndefined(usize),
    JumpIfNullish(usize),
    /// Peeks the duplicate on top: jumps when nullish, otherwise pops it.
    JumpIfNullishKeep(usize),

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

    Not,
    Neg,
    Pos,
    BitNot,
    TypeOf,
    TypeOfGlobal(u32),
    Void,
    Await,

    /// `dst += src` for two resolved variables, fused from
    /// `GetVar/GetVar/Add/Dup/SetVar`.
    AddVar {
        dst_depth: u16,
        dst_slot: u16,
        src_depth: u16,
        src_slot: u16,
    },
    /// `left + right` for two resolved locals, pushing the result.
    AddVars {
        left_depth: u16,
        left_slot: u16,
        right_depth: u16,
        right_slot: u16,
    },
    /// Comparison of a local with a constant, pushing a boolean.
    CompareVarConst {
        op: CompareOp,
        depth: u16,
        slot: u16,
        value: u32,
    },
    /// Comparison of two locals, pushing a boolean.
    CompareVars {
        op: CompareOp,
        left_depth: u16,
        left_slot: u16,
        right_depth: u16,
        right_slot: u16,
    },
    /// Increment/decrement a local and jump, with the result discarded.
    UpdateVarJump {
        depth: u16,
        slot: u16,
        delta: i8,
        target: usize,
    },
    /// `local + constant`, pushing the result.
    AddVarConst {
        depth: u16,
        slot: u16,
        value: u32,
    },
    IncVar {
        depth: u16,
        slot: u16,
        prefix: bool,
    },
    DecVar {
        depth: u16,
        slot: u16,
        prefix: bool,
    },
    IncGlobal(u32, bool),
    DecGlobal(u32, bool),
    UpdateProp {
        delta: f64,
        prefix: bool,
    },

    PushEnv(u16),
    PopEnv,
    Closure(u32),
    TryStart(usize),
    TryEnd,
    IterInit,
    ForInInit,
    IterNext(usize),
    IterClose,
    CreateClass(u32),
    SuperCall(u16),
}
