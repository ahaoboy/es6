use std::cell::RefCell;
use std::rc::Rc;

use crate::string::JsString;

#[derive(Clone)]
pub enum Constant {
    /// Integral literal that fits `i32`; pushed as `JsValue::Int` without
    /// running the float classification in the hot path.
    Int(i32),
    Number(f64),
    String(JsString),
    Function(Rc<FunctionDef>),
    Class(Rc<ClassDef>),
    Strings(Rc<[JsString]>),
    Regex {
        source: Rc<str>,
        flags: Rc<str>,
    },
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
/// `JSFunctionBytecode` (quickjs.c:855) with `byte_code_buf`, `cpool` and
/// `closure_var`.
/// High bit of a bytecode slot operand: the binding lives in the
/// environment chain instead of the flat frame (QuickJS compiles separate
/// OP_get_var_ref/OP_get_loc opcodes for the same distinction).
pub const ENV_SLOT: u16 = 0x8000;
pub struct FunctionDef {
    pub name: Rc<str>,
    pub code: Vec<Op>,
    /// Source byte offset for each opcode, used for diagnostics. QuickJS
    /// builds the equivalent `pc2line` table in `resolve_labels`
    /// (quickjs.c:35751).
    pub positions: Vec<u32>,
    pub constants: Vec<Constant>,
    pub prop_caches: Vec<RefCell<crate::object::PropCache>>,
    pub env_size: u16,
    /// True when this function creates closures/classes that can capture its
    /// frame. QuickJS likewise only materializes `var_refs` for captured
    /// variables; uncaptured locals stay in stack slots (`JSStackFrame`,
    /// quickjs.c:427).
    pub use_env: bool,
    pub length: u32,
    pub is_arrow: bool,
    pub is_async: bool,
    pub is_generator: bool,
    pub strict: bool,
    /// Synthetic `<class_fields>` initializer, which copies `this` slots into
    /// the frame at entry. Precomputed so the call path does not compare
    /// function names.
    pub is_class_fields: bool,
    /// Leading simple parameters pre-filled into slots 0..simple_params by
    /// the frame prologue (QuickJS copies arguments into arg_buf the same
    /// way, quickjs.c:18132).
    pub simple_params: u16,
    pub source: String,
    /// Lazily built `name` property atom: every closure creation would
    /// otherwise allocate a fresh string for the same bytecode name.
    pub name_atom: RefCell<Option<crate::string::JsString>>,
}

impl FunctionDef {
    /// The cached `name` atom for this function's `.name` property.
    pub fn name_atom(&self) -> crate::string::JsString {
        if let Some(atom) = &*self.name_atom.borrow() {
            return atom.clone();
        }
        let atom = crate::string::JsString::from(self.name.as_ref());
        *self.name_atom.borrow_mut() = Some(atom.clone());
        atom
    }
    /// Shared placeholder used by recycled activation records.
    pub fn empty() -> Rc<Self> {
        thread_local! {
            static EMPTY: Rc<FunctionDef> = Rc::new(FunctionDef {
                name: Rc::from(""),
                code: Vec::new(),
                positions: Vec::new(),
                constants: Vec::new(),
                prop_caches: Vec::new(),
                env_size: 0,
                use_env: false,
                length: 0,
                is_arrow: false,
                is_async: false,
                is_generator: false,
                strict: false,
                is_class_fields: false,
                simple_params: 0,
                source: String::new(),
                name_atom: RefCell::new(None),
            });
        }
        EMPTY.with(|empty| empty.clone())
    }
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
    /// Push the current callee, used for named function expression
    /// self-reference, like QuickJS' function-scope `FVAR` binding
    /// (quickjs.c:37379).
    Callee,
    PushConst(u32),
    Pop,
    Dup,
    Dup2,
    Swap,

    LoadArg(u16),
    /// `arg -> local slot` in one dispatch, used for simple parameters.
    LoadArgInto {
        arg: u16,
        depth: u16,
        slot: u16,
    },
    RestArgs(u16),
    Arguments,
    /// `local.prop` — fuses `GetVar`/`This` with a cached constant get.
    GetVarPropConst {
        depth: u16,
        slot: u16,
        key: u32,
        cache: u32,
    },
    GetThisPropConst {
        key: u32,
        cache: u32,
    },
    /// `local.prop = value` — the value is on the stack; pushes it back.
    SetVarPropConstKeep {
        depth: u16,
        slot: u16,
        key: u32,
        cache: u32,
    },
    SetThisPropConstKeep {
        key: u32,
        cache: u32,
    },
    /// `local.method(args)` / `this.method(args)` with cached constant key;
    /// arguments are on the stack.
    CallVarMethodConst {
        depth: u16,
        slot: u16,
        key: u32,
        cache: u32,
        argc: u16,
    },
    CallVarMethodConstDiscard {
        depth: u16,
        slot: u16,
        key: u32,
        cache: u32,
        argc: u16,
    },
    /// global.prop in one dispatch: the global read and the constant-key
    /// property read each keep their own inline cache.
    GetGlobalPropConst {
        name: u32,
        key: u32,
        global_cache: u16,
        prop_cache: u16,
    },
    CallThisMethodConst {
        key: u32,
        cache: u32,
        argc: u16,
    },
    CallThisMethodConstDiscard {
        key: u32,
        cache: u32,
        argc: u16,
    },
    GetVar {
        depth: u16,
        slot: u16,
    },
    SetVar {
        depth: u16,
        slot: u16,
    },
    GetGlobal {
        name: u32,
        cache: u32,
    },
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
        /// Per-site property-map template cache, so repeated evaluation of
        /// the literal clones a prepared map instead of re-hashing each key
        /// (QuickJS reuses a shape the same way).
        cache: u32,
    },
    NewArray(u32),
    ArrayPush,
    ArraySpread,
    ObjectSpread,
    ArrayRest(u16),
    ObjectRest(u32),

    Call(u16),
    /// Call in statement position: the result is dropped instead of pushed.
    CallDiscard(u16),
    CallMethod(u16),
    CallMethodDiscard(u16),
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
    /// Generator suspension with an absolute resume label
    /// (`OP_yield`, quickjs.c:20871).
    Yield(u32),
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfTrue(u32),
    JumpIfNotUndefined(u32),
    JumpIfNullish(u32),
    /// Peeks the duplicate on top: jumps when nullish, otherwise pops it.
    JumpIfNullishKeep(u32),

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

    /// `dst += src` as an expression statement: like `AddVar` without the
    /// result push (`OP_add_loc` + `OP_drop` in QuickJS).
    AddVarDiscard {
        dst_depth: u16,
        dst_slot: u16,
        src_depth: u16,
        src_slot: u16,
    },
    /// `local++`/`local--` with the result discarded.
    UpdateVarDiscard {
        depth: u16,
        slot: u16,
        delta: i8,
    },
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
    /// Fused compare + conditional branch: the boolean never reaches the
    /// value stack. Conditions dominate loop bytecode, so this removes one
    /// opcode and one stack round-trip per iteration.
    CompareVarsJump {
        op: CompareOp,
        left_depth: u16,
        left_slot: u16,
        right_depth: u16,
        right_slot: u16,
        jump_if_true: bool,
        target: u32,
    },
    CompareVarConstJump {
        op: CompareOp,
        depth: u16,
        slot: u16,
        value: i32,
        jump_if_true: bool,
        target: u32,
    },
    /// `if (local & constant)` fused with its branch, used by flag tests in
    /// hot loops.
    BitAndVarConstJump {
        depth: u16,
        slot: u16,
        value: i32,
        jump_if_true: bool,
        target: u32,
    },
    /// Increment/decrement a local and jump, with the result discarded.
    UpdateVarJump {
        depth: u16,
        slot: u16,
        delta: i8,
        target: u32,
    },
    /// `for` loop tail: increment `slot`, compare it with a limit and either
    /// jump back to the loop body or fall through to the exit. Collapses the
    /// `UpdateVarJump` + `CompareVarsJump` pair QuickJS keeps separate.
    ForNext {
        depth: u16,
        slot: u16,
        delta: i8,
        limit_depth: u16,
        limit_slot: u16,
        op: CompareOp,
        body: u32,
    },
    ForNextConst {
        depth: u16,
        slot: u16,
        delta: i8,
        value: i32,
        op: CompareOp,
        body: u32,
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
    /// `export * from "m"`: copy every own export of the module on top of the
    /// stack into the exports object below it, skipping `default`
    /// (`ExportEntriesForModule`, ES2022 16.2.3.5).
    ExportAll,
    TryStart(u32),
    TryEnd,
    IterInit,
    ForInInit,
    IterNext(u32),
    IterClose,
    CreateClass(u32),
    SuperCall(u16),
}
