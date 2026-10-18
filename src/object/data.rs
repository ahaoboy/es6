//! Exotic object payloads: arrays, typed arrays, dates, regexps,
//! promises, maps and sets. Each corresponds to a `JS_CLASS_*` payload of
//! QuickJS `JSObject` (quickjs.c:1125).

use super::*;

#[derive(Clone)]
pub struct ArrayData {
    /// Dense elements; `None` marks a hole. QuickJS fast arrays cannot hold
    /// holes and convert to slow property arrays on `delete`, which this
    /// representation models directly.
    pub items: Vec<Option<JsValue>>,
    pub length: usize,
}

#[derive(Clone)]
pub struct RegExpData {
    pub source: String,
    pub flags: String,
    pub regex: Rc<regress::Regex>,
}

pub type ByteBuffer = Rc<RefCell<Vec<u8>>>;

#[derive(Clone)]
pub struct ArrayBufferData {
    pub bytes: ByteBuffer,
}

#[derive(Clone)]
pub struct DataViewData {
    pub buffer: ObjRef,
    pub bytes: ByteBuffer,
    pub byte_offset: usize,
    pub byte_length: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypedArrayKind {
    Uint8,
    Uint8Clamped,
    Int8,
    Uint16,
    Int16,
    Uint32,
    Int32,
    Float32,
    Float64,
}

impl TypedArrayKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Uint8 => "Uint8Array",
            Self::Uint8Clamped => "Uint8ClampedArray",
            Self::Int8 => "Int8Array",
            Self::Uint16 => "Uint16Array",
            Self::Int16 => "Int16Array",
            Self::Uint32 => "Uint32Array",
            Self::Int32 => "Int32Array",
            Self::Float32 => "Float32Array",
            Self::Float64 => "Float64Array",
        }
    }

    pub fn bytes_per_element(self) -> usize {
        match self {
            Self::Uint8 | Self::Uint8Clamped | Self::Int8 => 1,
            Self::Uint16 | Self::Int16 => 2,
            Self::Uint32 | Self::Int32 | Self::Float32 => 4,
            Self::Float64 => 8,
        }
    }
}

#[derive(Clone)]
pub struct TypedArrayData {
    pub kind: TypedArrayKind,
    pub buffer: ObjRef,
    pub bytes: ByteBuffer,
    pub byte_offset: usize,
    pub length: usize,
}

impl TypedArrayData {
    pub fn byte_length(&self) -> usize {
        self.length * self.kind.bytes_per_element()
    }

    pub fn get(&self, index: usize) -> JsValue {
        if index >= self.length {
            return JsValue::Undefined;
        }
        let width = self.kind.bytes_per_element();
        let start = self.byte_offset + index * width;
        let bytes = self.bytes.borrow();
        let Some(slice) = bytes.get(start..start + width) else {
            return JsValue::Undefined;
        };
        match self.kind {
            TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => JsValue::Int(slice[0] as i32),
            TypedArrayKind::Int8 => JsValue::Int(slice[0] as i8 as i32),
            TypedArrayKind::Uint16 => JsValue::Int(u16::from_le_bytes([slice[0], slice[1]]) as i32),
            TypedArrayKind::Int16 => JsValue::Int(i16::from_le_bytes([slice[0], slice[1]]) as i32),
            TypedArrayKind::Uint32 => {
                JsValue::number(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]) as f64)
            }
            TypedArrayKind::Int32 => {
                JsValue::Int(i32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
            }
            TypedArrayKind::Float32 => {
                JsValue::number(f32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]) as f64)
            }
            TypedArrayKind::Float64 => JsValue::number(f64::from_le_bytes([
                slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
            ])),
        }
    }

    pub fn set(&self, realm: &mut crate::runtime::Realm, index: usize, value: &JsValue) {
        if index >= self.length {
            return;
        }
        let number = crate::vm::to_number(realm, value.clone()).unwrap_or(f64::NAN);
        let width = self.kind.bytes_per_element();
        let start = self.byte_offset + index * width;
        let mut bytes = self.bytes.borrow_mut();
        let Some(slice) = bytes.get_mut(start..start + width) else {
            return;
        };
        match self.kind {
            TypedArrayKind::Uint8 => slice[0] = crate::vm::to_uint32(number) as u8,
            TypedArrayKind::Uint8Clamped => {
                slice[0] = if number.is_nan() {
                    0
                } else {
                    number.round().clamp(0.0, 255.0) as u8
                }
            }
            TypedArrayKind::Int8 => slice[0] = crate::vm::to_int32(number) as i8 as u8,
            TypedArrayKind::Uint16 => {
                slice.copy_from_slice(&(crate::vm::to_uint32(number) as u16).to_le_bytes())
            }
            TypedArrayKind::Int16 => {
                slice.copy_from_slice(&(crate::vm::to_int32(number) as i16).to_le_bytes())
            }
            TypedArrayKind::Uint32 => {
                slice.copy_from_slice(&crate::vm::to_uint32(number).to_le_bytes())
            }
            TypedArrayKind::Int32 => {
                slice.copy_from_slice(&crate::vm::to_int32(number).to_le_bytes())
            }
            TypedArrayKind::Float32 => slice.copy_from_slice(&(number as f32).to_le_bytes()),
            TypedArrayKind::Float64 => slice.copy_from_slice(&number.to_le_bytes()),
        }
    }

    pub fn values(&self) -> Vec<JsValue> {
        (0..self.length).map(|index| self.get(index)).collect()
    }
}

pub struct JsFunction {
    pub name: Rc<str>,
    pub length: u32,
    pub native: Option<NativeFn>,
    /// C-function-data variant: `data_fn` receives the shared `data` cell as
    /// its last argument (quickjs `js_c_function_data`, quickjs.c:6595).
    pub native_data: Option<DataFn>,
    pub data: NativeData,
    pub def: Option<Rc<FunctionDef>>,
    pub env: Option<EnvRef>,
    pub bound: Option<BoundFunction>,
    pub lexical_this: Option<JsValue>,
    pub instance_init: Option<Rc<FunctionDef>>,
}

#[derive(Clone)]
pub struct BoundFunction {
    pub target: JsValue,
    pub this: JsValue,
    pub arguments: Vec<JsValue>,
}

impl JsFunction {
    pub fn native(
        name: &str,
        length: u32,
        function: impl Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue> + 'static,
    ) -> Self {
        Self {
            name: Rc::from(name),
            length,
            native: Some(Rc::new(function)),
            native_data: None,
            data: Rc::new(RefCell::new(Vec::new())),
            def: None,
            env: None,
            bound: None,
            lexical_this: None,
            instance_init: None,
        }
    }

    pub fn is_native(&self) -> bool {
        self.native.is_some() || self.native_data.is_some()
    }
}

pub enum ResolvedProperty {
    Data {
        value: JsValue,
        writable: bool,
        own: bool,
    },
    Accessor {
        get: Option<JsValue>,
        set: Option<JsValue>,
        own: bool,
    },
    Missing,
}

pub enum ObjectKind {
    Ordinary,
    Array(ArrayData),
    Function(Rc<JsFunction>),
    BoxedString(Rc<str>),
    BoxedNumber(f64),
    BoxedBool(bool),
    Date(f64),
    RegExp(Rc<RegExpData>),
    Iterator { items: Vec<JsValue>, index: usize },
    Map(Vec<(JsValue, JsValue)>),
    Set(Vec<JsValue>),
    Promise(Box<PromiseData>),
    ArrayBuffer(ArrayBufferData),
    DataView(DataViewData),
    TypedArray(TypedArrayData),
    /// Suspended `async` frame, the equivalent of the QuickJS `JSAsyncFunction`
    /// object that owns the suspended stack (`JSStackFrame.cur_gc_obj`,
    /// quickjs.c:442). Storing it in an object keeps the whole suspended frame
    /// visible to the GC.
    AsyncFrame(Option<Box<crate::vm::Frame>>),
    Error,
}

impl ObjectKind {
    /// Take the suspended frame out of an async-frame object, like the
    /// async resume path of `JS_CallInternal` (quickjs.c:18093).
    #[inline]
    pub fn take_async_frame(&mut self) -> Option<Box<crate::vm::Frame>> {
        match self {
            ObjectKind::AsyncFrame(frame) => frame.take(),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromiseState {
    Pending,
    Fulfilled,
    Rejected,
}

/// Pending reaction recorded by `then`/`await`. QuickJS keeps the same
/// triple in `JSPromiseReactionData` (quickjs.c, `js_promise_then`).
#[derive(Clone)]
pub struct PromiseReaction {
    pub on_fulfilled: Option<JsValue>,
    pub on_rejected: Option<JsValue>,
    pub promise: Option<ObjRef>,
}

#[derive(Clone)]
pub struct PromiseData {
    pub state: PromiseState,
    pub value: JsValue,
    pub reactions: Vec<PromiseReaction>,
    pub handled: bool,
}
