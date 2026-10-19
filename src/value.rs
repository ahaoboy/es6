use std::fmt;
use std::rc::Rc;

use crate::gc::{Finalize, Gc, GcRefCell};

use crate::bytecode::FunctionDef;
use crate::object::{Function, ObjRef, Object, ObjectData};
use crate::string::JsString;

/// The internal completion type. A JavaScript exception is represented by the
/// thrown value itself, which keeps unwinding allocation-free and makes the VM
/// loop straightforward.
pub type JsResult<T> = std::result::Result<T, JsValue>;

pub type EnvRef = Gc<Env>;

/// Lexical environment frame. QuickJS represents the same chain with
/// `JSVarRef`/`JSFunctionBytecode.closure_var` and resolves `(depth, slot)`
/// references while walking `JSStackFrame.var_refs` (quickjs.c:427).
pub struct Env {
    /// Captured slots stay inline for the common small closure, avoiding a
    /// heap buffer per call (`JSVarRef` chains in QuickJS are similarly
    /// compact).
    pub slots: GcRefCell<smallvec::SmallVec<[JsValue; 4]>>,
    pub parent: Option<EnvRef>,
}

impl Env {
    pub fn new(size: usize, parent: Option<EnvRef>) -> EnvRef {
        let mut slots = smallvec::SmallVec::new();
        slots.resize(size, JsValue::Undefined);
        Gc::new(Self {
            slots: GcRefCell::new(slots),
            parent,
        })
    }

    #[inline(always)]
    pub fn get(&self, depth: usize, slot: usize) -> JsValue {
        let mut env = self;
        for _ in 0..depth {
            match &env.parent {
                Some(parent) => env = parent,
                None => return JsValue::Undefined,
            }
        }
        env.slots
            .borrow()
            .get(slot)
            .cloned()
            .unwrap_or(JsValue::Undefined)
    }

    #[inline(always)]
    pub fn set(&self, depth: usize, slot: usize, value: JsValue) {
        let mut env = self;
        for _ in 0..depth {
            match &env.parent {
                Some(parent) => env = parent,
                None => return,
            }
        }
        if let Some(slot) = env.slots.borrow_mut().get_mut(slot) {
            *slot = value;
        }
    }
}

#[derive(Clone, Default)]
pub enum JsValue {
    #[default]
    Undefined,
    Null,
    Bool(bool),
    Int(i32),
    Float(f64),
    String(JsString),
    Symbol(u32),
    Object(ObjRef),
}

impl fmt::Debug for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undefined => write!(f, "undefined"),
            Self::Null => write!(f, "null"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::String(v) => write!(f, "\"{v}\""),
            Self::Symbol(v) => write!(f, "Symbol({v})"),
            Self::Object(_) => write!(f, "[object]"),
        }
    }
}

impl JsValue {
    pub fn is_undefined(&self) -> bool {
        matches!(self, Self::Undefined)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn is_nullish(&self) -> bool {
        matches!(self, Self::Undefined | Self::Null)
    }

    pub fn is_bool(&self) -> bool {
        matches!(self, Self::Bool(_))
    }

    pub fn is_number(&self) -> bool {
        matches!(self, Self::Int(_) | Self::Float(_))
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }

    pub fn is_symbol(&self) -> bool {
        matches!(self, Self::Symbol(_))
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }

    pub fn is_function(&self) -> bool {
        matches!(self, Self::Object(obj) if obj.borrow().is_function())
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Int(v) => Some(*v as f64),
            Self::Float(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<JsString> {
        match self {
            Self::String(v) => Some(v.clone()),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<Object> {
        match self {
            Self::Object(obj) => Some(Object::from_ref(obj.clone())),
            _ => None,
        }
    }

    pub fn as_function(&self) -> Option<Function> {
        match self {
            Self::Object(obj) => obj.borrow().function().map(Function::from_ref),
            _ => None,
        }
    }

    pub fn type_of(&self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Null => "object",
            Self::Bool(_) => "boolean",
            Self::Int(_) | Self::Float(_) => "number",
            Self::String(_) => "string",
            Self::Symbol(_) => "symbol",
            Self::Object(obj) => {
                if obj.borrow().is_function() {
                    "function"
                } else {
                    "object"
                }
            }
        }
    }

    #[inline(always)]
    pub fn number(value: f64) -> Self {
        // Saturating cast round-trip: exact for every integral double in i32
        // range, one multiply-free compare for the rest (NaN/inf/huge all
        // fail the equality test). Negative zero must stay a float: collapsing
        // it into `Int(0)` would lose the sign bit (ES2022 6.1.6.1.3,
        // `Object.is(0, -0)`).
        let integer = value as i32;
        if integer as f64 == value {
            if integer == 0 && value.is_sign_negative() {
                Self::Float(value)
            } else {
                Self::Int(integer)
            }
        } else {
            Self::Float(value)
        }
    }

    pub fn string(value: impl Into<JsString>) -> Self {
        Self::String(value.into())
    }

    pub fn from_function(function: Rc<crate::object::JsFunction>) -> Self {
        Self::Object(Gc::new(GcRefCell::new(ObjectData::new_function(function))))
    }

    pub fn from_compiled(def: Rc<FunctionDef>, env: EnvRef) -> Self {
        let function = crate::object::JsFunction {
            name: def.name.clone(),
            length: def.length,
            native: None,
            native_data: None,
            data: Gc::new(GcRefCell::new(Vec::new())),
            def: Some(def),
            env: Some(env),
            bound: None,
            lexical_this: None,
            instance_init: None,
            has_super: false,
            prototype: GcRefCell::new(None),
        };
        Self::from_function(Rc::new(function))
    }
}

pub trait IntoJs {
    fn into_js(self) -> JsValue;
}

pub trait FromJs: Sized {
    fn from_js(value: JsValue) -> Option<Self>;
}

impl IntoJs for JsValue {
    fn into_js(self) -> JsValue {
        self
    }
}

impl IntoJs for () {
    fn into_js(self) -> JsValue {
        JsValue::Undefined
    }
}

impl IntoJs for bool {
    fn into_js(self) -> JsValue {
        JsValue::Bool(self)
    }
}

impl IntoJs for i32 {
    fn into_js(self) -> JsValue {
        JsValue::Int(self)
    }
}

impl IntoJs for u32 {
    fn into_js(self) -> JsValue {
        JsValue::number(self as f64)
    }
}

impl IntoJs for f64 {
    fn into_js(self) -> JsValue {
        JsValue::number(self)
    }
}

impl IntoJs for &str {
    fn into_js(self) -> JsValue {
        JsValue::string(self)
    }
}

impl IntoJs for String {
    fn into_js(self) -> JsValue {
        JsValue::string(self)
    }
}

impl IntoJs for Object {
    fn into_js(self) -> JsValue {
        JsValue::Object(self.into_ref())
    }
}

impl IntoJs for Function {
    fn into_js(self) -> JsValue {
        JsValue::Object(Gc::new(GcRefCell::new(ObjectData::new_function(
            self.to_ref(),
        ))))
    }
}

impl<T: IntoJs> IntoJs for Option<T> {
    fn into_js(self) -> JsValue {
        self.map(IntoJs::into_js).unwrap_or(JsValue::Null)
    }
}

impl<T: IntoJs> IntoJs for Vec<T> {
    fn into_js(self) -> JsValue {
        let objects = self.into_iter().map(IntoJs::into_js).collect();
        JsValue::Object(Gc::new(GcRefCell::new(ObjectData::new_array(objects))))
    }
}

impl FromJs for JsValue {
    fn from_js(value: JsValue) -> Option<Self> {
        Some(value)
    }
}

impl FromJs for bool {
    fn from_js(value: JsValue) -> Option<Self> {
        match value {
            JsValue::Bool(v) => Some(v),
            _ => None,
        }
    }
}

impl FromJs for i32 {
    fn from_js(value: JsValue) -> Option<Self> {
        match value {
            JsValue::Int(v) => Some(v),
            JsValue::Float(v) => Some(v as i32),
            _ => None,
        }
    }
}

impl FromJs for f64 {
    fn from_js(value: JsValue) -> Option<Self> {
        value.as_number()
    }
}

impl FromJs for String {
    fn from_js(value: JsValue) -> Option<Self> {
        value.as_string().map(|v| v.to_string())
    }
}

impl FromJs for Object {
    fn from_js(value: JsValue) -> Option<Self> {
        value.as_object()
    }
}

impl FromJs for Function {
    fn from_js(value: JsValue) -> Option<Self> {
        value.as_function()
    }
}

impl Finalize for Env {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for Env {
    crate::gc::custom_trace!(this, mark, {
        if let Some(parent) = &this.parent {
            mark(parent);
        }
        for value in this
            .slots
            .try_borrow()
            .iter()
            .flat_map(|slots| slots.iter())
        {
            mark(value);
        }
    });
}

impl Finalize for JsValue {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for JsValue {
    crate::gc::custom_trace!(this, mark, {
        if let JsValue::Object(object) = this {
            mark(object);
        }
    });
}
