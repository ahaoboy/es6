//! Exotic object payloads: arrays, typed arrays, dates, regexps,
//! promises, maps and sets. Each corresponds to a `JS_CLASS_*` payload of
//! QuickJS `JSObject` (quickjs.c:1125).

use super::*;
use indexmap::{IndexMap, IndexSet};
use std::hash::Hash;
use std::rc::Rc;

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
    /// True for a derived class constructor: its field initializer runs after
    /// `super()`, not before the constructor body.
    pub has_super: bool,
    /// Lazily created `.prototype` object. QuickJS materializes the prototype
    /// when the function object is built; most closures are never used as
    /// constructors, so creating it on first access removes an allocation and
    /// two property insertions from every closure creation.
    pub prototype: GcRefCell<Option<ObjRef>>,
}

impl JsFunction {
    /// Whether this function can carry a `.prototype` object
    /// (ES2022 10.2.5 `MakeConstructor` applies to normal functions).
    pub fn is_constructable(&self) -> bool {
        self.bound.is_none()
            && self.native_data.is_none()
            && self
                .def
                .as_ref()
                .is_some_and(|def| !def.is_arrow && !def.is_async && !def.is_generator)
    }
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
            data: Gc::new(GcRefCell::new(Vec::new())),
            def: None,
            env: None,
            bound: None,
            lexical_this: None,
            instance_init: None,
            has_super: false,
            prototype: GcRefCell::new(None),
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

/// Object kinds. The `Frame`-carrying arms hold QuickJS-internal suspended
/// activations; the `private_interfaces` allowance keeps the frame type
/// crate-internal while the kind enum stays public.
#[allow(private_interfaces)]
/// Hashable value with SameValueZero semantics (ES2022 7.2.11), used as the
/// `Map`/`Set` key: `-0` and `0` collide, all NaNs collide, objects compare
/// by identity, strings by content.
#[derive(Clone)]
pub struct MapKey(pub JsValue);

fn canonical_number_bits(value: f64) -> u64 {
    if value.is_nan() {
        f64::NAN.to_bits()
    } else if value == 0.0 {
        0
    } else {
        value.to_bits()
    }
}

impl std::hash::Hash for MapKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match &self.0 {
            JsValue::Undefined => 0u8.hash(state),
            JsValue::Null => 1u8.hash(state),
            JsValue::Bool(value) => {
                2u8.hash(state);
                value.hash(state);
            }
            JsValue::Int(value) => {
                3u8.hash(state);
                canonical_number_bits(*value as f64).hash(state);
            }
            JsValue::Float(value) => {
                3u8.hash(state);
                canonical_number_bits(*value).hash(state);
            }
            JsValue::String(value) => {
                4u8.hash(state);
                value.hash(state);
            }
            JsValue::Symbol(value) => {
                5u8.hash(state);
                value.hash(state);
            }
            JsValue::Object(value) => {
                6u8.hash(state);
                (value.as_ref() as *const GcRefCell<ObjectData> as usize).hash(state);
            }
        }
    }
}

impl PartialEq for MapKey {
    fn eq(&self, other: &Self) -> bool {
        crate::coerce::same_value_zero(&self.0, &other.0)
    }
}

impl Eq for MapKey {}

pub type MapMap = IndexMap<MapKey, JsValue, FxBuildHasher>;
pub type SetSet = IndexSet<MapKey, FxBuildHasher>;

#[allow(private_interfaces)]
pub enum ObjectKind {
    Ordinary,
    Array(ArrayData),
    Function(Rc<JsFunction>),
    BoxedString(crate::string::JsString),
    BoxedNumber(f64),
    BoxedBool(bool),
    Date(f64),
    #[cfg(feature = "intl")]
    IntlDateTimeFormat(Box<IntlDateTimeFormatData>),
    RegExp(Rc<RegExpData>),
    Iterator {
        items: Vec<JsValue>,
        index: usize,
    },
    /// Lazy array iterator, the equivalent of QuickJS'
    /// `JS_CLASS_ARRAY_ITERATOR`: reads elements from the live array on
    /// every `next` (ES2022 23.1.5.1).
    ArrayIterator {
        array: ObjRef,
        index: usize,
        entries: bool,
    },
    /// Lazy string iterator over UTF-16 code units (ES2022 23.1.5.2.1).
    StringIterator {
        text: crate::string::JsString,
        index: usize,
    },
    Map(MapMap),
    Set(SetSet),
    /// Weak collections: entries die with their keys (ES2022 25.3/25.4).
    /// Keys are weak references, so the GC prunes dead entries; values stay
    /// strongly held while the key lives, like QuickJS' weak ref lists.
    WeakMap(WeakObjectMap),
    WeakSet(WeakObjectSet),
    Promise(Box<PromiseData>),
    ArrayBuffer(ArrayBufferData),
    DataView(DataViewData),
    TypedArray(TypedArrayData),
    /// Suspended `async` frame, the equivalent of the QuickJS `JSAsyncFunction`
    /// object that owns the suspended stack (`JSStackFrame.cur_gc_obj`,
    /// quickjs.c:442). Storing it in an object keeps the whole suspended frame
    /// visible to the GC.
    AsyncFrame(Option<Box<crate::vm::Frame>>),
    /// Generator object: the bytecode function plus its suspended frame,
    /// the equivalent of QuickJS `JS_CLASS_GENERATOR` (quickjs.c:20871
    /// `OP_yield` suspension).
    Generator(GeneratorData),
    Error,
}

/// Generator bookkeeping (ES2022 27.5.1): the function, the creation-time
/// receiver and arguments, and the suspended activation.
pub struct GeneratorData {
    pub function: Rc<JsFunction>,
    pub this: JsValue,
    pub arguments: Vec<JsValue>,
    pub state: GeneratorState,
}

/// QuickJS suspends generators by stashing `JSStackFrame.cur_sp/cur_pc`
/// inside the generator object (quickjs.c:20871); the whole `Frame` moves
/// in and out here.
#[allow(private_interfaces)]
pub enum GeneratorState {
    SuspendedStart,
    Executing,
    SuspendedYield(Box<crate::vm::Frame>),
    Completed,
}

impl ObjectKind {
    /// Take the suspended frame out of an async-frame object, like the
    /// async resume path of `JS_CallInternal` (quickjs.c:18093).
    #[inline]
    #[allow(private_interfaces)]
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

impl Finalize for JsFunction {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for JsFunction {
    crate::gc::custom_trace!(this, mark, {
        mark(&this.data);
        if let Some(env) = &this.env {
            mark(env);
        }
        if let Some(bound) = &this.bound {
            mark(bound);
        }
        if let Some(value) = &this.lexical_this {
            mark(value);
        }
        mark(&this.prototype);
    });
}

impl Finalize for BoundFunction {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for BoundFunction {
    crate::gc::custom_trace!(this, mark, {
        mark(&this.target);
        mark(&this.this);
        for argument in &this.arguments {
            mark(argument);
        }
    });
}

impl Finalize for ArrayData {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for ArrayData {
    crate::gc::custom_trace!(this, mark, {
        for value in this.items.iter().flatten() {
            mark(value);
        }
    });
}

impl Finalize for ArrayBufferData {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for ArrayBufferData {
    crate::gc::empty_trace!();
}

impl Finalize for DataViewData {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for DataViewData {
    crate::gc::custom_trace!(this, mark, {
        mark(&this.buffer);
    });
}

impl Finalize for TypedArrayData {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for TypedArrayData {
    crate::gc::custom_trace!(this, mark, {
        mark(&this.buffer);
    });
}

impl Finalize for GeneratorData {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for GeneratorData {
    crate::gc::custom_trace!(this, mark, {
        mark(&*this.function);
        mark(&this.this);
        for argument in &this.arguments {
            mark(argument);
        }
        mark(&this.state);
    });
}

impl Finalize for GeneratorState {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for GeneratorState {
    crate::gc::custom_trace!(this, mark, {
        if let GeneratorState::SuspendedYield(frame) = this {
            mark(&**frame);
        }
    });
}

impl Finalize for PromiseReaction {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for PromiseReaction {
    crate::gc::custom_trace!(this, mark, {
        if let Some(value) = &this.on_fulfilled {
            mark(value);
        }
        if let Some(value) = &this.on_rejected {
            mark(value);
        }
        if let Some(promise) = &this.promise {
            mark(promise);
        }
    });
}

impl Finalize for PromiseData {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for PromiseData {
    crate::gc::custom_trace!(this, mark, {
        mark(&this.value);
        for reaction in &this.reactions {
            mark(reaction);
        }
    });
}

impl Finalize for MapKey {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for MapKey {
    crate::gc::custom_trace!(this, mark, {
        mark(&this.0);
    });
}

impl Finalize for ObjectKind {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for ObjectKind {
    crate::gc::custom_trace!(this, mark, {
        match this {
            ObjectKind::Array(data) => mark(data),
            ObjectKind::Function(function) => mark(&**function),
            ObjectKind::Iterator { items, .. } => {
                for value in items {
                    mark(value);
                }
            }
            ObjectKind::ArrayIterator { array, .. } => mark(array),
            ObjectKind::Map(map) => {
                for (key, value) in map {
                    mark(key);
                    mark(value);
                }
            }
            ObjectKind::Set(set) => {
                for key in set {
                    mark(key);
                }
            }
            ObjectKind::WeakMap(map) => mark(map),
            ObjectKind::WeakSet(set) => mark(set),
            ObjectKind::Promise(data) => mark(&**data),
            ObjectKind::DataView(data) => mark(data),
            ObjectKind::TypedArray(data) => mark(data),
            ObjectKind::AsyncFrame(Some(frame)) => mark(&**frame),
            ObjectKind::Generator(data) => mark(data),
            #[cfg(feature = "intl")]
            ObjectKind::IntlDateTimeFormat(_) => {}
            ObjectKind::Ordinary
            | ObjectKind::BoxedString(_)
            | ObjectKind::BoxedNumber(_)
            | ObjectKind::BoxedBool(_)
            | ObjectKind::Date(_)
            | ObjectKind::RegExp(_)
            | ObjectKind::StringIterator { .. }
            | ObjectKind::ArrayBuffer(_)
            | ObjectKind::AsyncFrame(None)
            | ObjectKind::Error => {}
        }
    });
}
#[cfg(feature = "intl")]
pub struct IntlDateTimeFormatData {
    pub(crate) formatter: icu_datetime::DateTimeFormatter<
        icu_datetime::fieldsets::enums::CompositeDateTimeFieldSet,
    >,
    pub(crate) locale: icu_locale::Locale,
    pub(crate) time_zone: jiff::tz::TimeZone,
    pub(crate) time_zone_name: String,
    pub(crate) date_fields: Option<icu_datetime::fieldsets::builder::DateFields>,
    pub(crate) time_precision: Option<icu_datetime::options::TimePrecision>,
    pub(crate) date_style: Option<String>,
    pub(crate) time_style: Option<String>,
    pub(crate) hour12: Option<bool>,
    pub(crate) calendar: String,
    pub(crate) numbering_system: String,
}
