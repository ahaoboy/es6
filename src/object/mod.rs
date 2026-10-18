use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use indexmap::IndexMap;
use rustc_hash::FxBuildHasher;

pub type PropertyMap = IndexMap<PropKey, Property, FxBuildHasher>;

use crate::bytecode::FunctionDef;
use crate::runtime::Realm;
use crate::string::JsString;
use crate::value::{EnvRef, JsResult, JsValue};

pub type ObjRef = Rc<RefCell<ObjectData>>;
pub type NativeFn = Rc<dyn Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue>>;
/// Native function with a shared, GC-visible data cell, the equivalent of
/// QuickJS C functions with data (`js_c_function_data`, quickjs.c:6595):
/// the data array is walked by the GC exactly like QuickJS'
/// `mark_children` (quickjs.c:7255).
pub type DataFn = Rc<dyn Fn(&mut Realm, &JsValue, &[JsValue], &[JsValue]) -> JsResult<JsValue>>;
pub type NativeData = Rc<RefCell<Vec<JsValue>>>;

/// Monomorphic property cache for one constant access site. It mirrors
/// QuickJS' `JSPropCacheEntry`: the receiver shape guards the prototype chain,
/// while a weak holder reference keeps the cached slot alive and verifiable.
#[derive(Clone, Default)]
pub struct PropCache {
    pub object_shape: u64,
    pub holder: Option<Weak<RefCell<ObjectData>>>,
    pub holder_shape: u64,
    pub receiver_proto: Option<Weak<RefCell<ObjectData>>>,
    pub prototype_generation: u64,
    pub index: u32,
    /// Shape before a new own property was inserted, used to define the same
    /// property on the next object of the same shape.
    pub insert_shape: u64,
    pub can_insert: bool,
    pub own: bool,
    pub valid: bool,
}

static PROTOTYPE_GENERATION: AtomicU64 = AtomicU64::new(1);

pub fn prototype_generation() -> u64 {
    PROTOTYPE_GENERATION.load(Ordering::Relaxed)
}

pub fn bump_prototype_generation() {
    PROTOTYPE_GENERATION.fetch_add(1, Ordering::Relaxed);
}

#[derive(Clone, Debug)]
pub enum PropKey {
    Str(JsString),
    Index(usize),
    Symbol(u32),
}

impl From<&str> for PropKey {
    fn from(value: &str) -> Self {
        Self::Str(JsString::from(value))
    }
}

impl From<&String> for PropKey {
    fn from(value: &String) -> Self {
        Self::Str(JsString::from(value.as_str()))
    }
}

impl From<String> for PropKey {
    fn from(value: String) -> Self {
        Self::Str(JsString::from(value))
    }
}

impl From<JsString> for PropKey {
    fn from(value: JsString) -> Self {
        Self::Str(value)
    }
}

impl PropKey {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) => Some(value),
            Self::Index(_) | Self::Symbol(_) => None,
        }
    }

    pub fn index(&self) -> Option<usize> {
        match self {
            Self::Index(index) => Some(*index),
            Self::Str(value) => key_to_index(value),
            Self::Symbol(_) => None,
        }
    }

    pub fn to_rc_string(&self) -> Option<JsString> {
        match self {
            Self::Str(value) => Some(value.clone()),
            Self::Index(index) => Some(JsString::from(index.to_string())),
            Self::Symbol(_) => None,
        }
    }

    pub(crate) fn canonical(&self) -> PropKey {
        match self {
            Self::Index(index) => Self::Index(*index),
            Self::Str(value) => key_to_index(value)
                .map(Self::Index)
                .unwrap_or_else(|| Self::Str(value.clone())),
            Self::Symbol(value) => Self::Symbol(*value),
        }
    }
}

/// ECMAScript property descriptor. QuickJS stores the same fields packed in
/// `JSProperty` plus `JSShape` transitions; `IndexMap` keeps insertion order
/// without unsafe shape code.
#[derive(Clone)]
pub struct Property {
    pub value: JsValue,
    /// Accessors are rare, so boxing them keeps the common data property
    /// compact (QuickJS stores them as separate `JSProperty` variants).
    pub get: Option<Box<JsValue>>,
    pub set: Option<Box<JsValue>>,
    pub writable: bool,
    pub enumerable: bool,
    pub configurable: bool,
}

impl Property {
    pub fn data(value: JsValue) -> Self {
        Self {
            value,
            get: None,
            set: None,
            writable: true,
            enumerable: true,
            configurable: true,
        }
    }

    pub fn hidden(value: JsValue) -> Self {
        Self {
            value,
            get: None,
            set: None,
            writable: true,
            enumerable: false,
            configurable: true,
        }
    }

    pub fn read_only(value: JsValue) -> Self {
        Self {
            value,
            get: None,
            set: None,
            writable: false,
            enumerable: false,
            configurable: false,
        }
    }

    pub fn accessor(get: Option<JsValue>, set: Option<JsValue>) -> Self {
        Self {
            value: JsValue::Undefined,
            get: get.map(Box::new),
            set: set.map(Box::new),
            writable: false,
            enumerable: true,
            configurable: true,
        }
    }

    pub fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }
}

/// Object representation corresponding to QuickJS `JSObject` (quickjs.c:1125
/// area): shape/properties, prototype and an optional exotic payload. The
/// insertion-ordered map is the safe Rust stand-in for `JSShape`.
pub struct ObjectData {
    pub props: PropertyMap,
    pub prototype: Option<ObjRef>,
    pub kind: ObjectKind,
    pub extensible: bool,
    pub class_name: &'static str,
    /// Monotonic property-shape version. Structural changes (insert/delete,
    /// prototype change) invalidate inline caches, mirroring QuickJS shapes.
    pub shape: u64,
    /// Set when this object appears in another object's prototype chain.
    /// Structural changes on prototypes invalidate monomorphic property
    /// caches that may have observed them.
    pub is_prototype: bool,
}

impl ObjectData {
    pub fn ordinary(prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::with_capacity_and_hasher(4, FxBuildHasher),
            prototype,
            kind: ObjectKind::Ordinary,
            extensible: true,
            class_name: "Object",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn array(prototype: Option<ObjRef>, items: Vec<JsValue>) -> Self {
        let length = items.len();
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Array(ArrayData {
                items: items.into_iter().map(Some).collect(),
                length,
            }),
            extensible: true,
            class_name: "Array",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn new_array(items: Vec<JsValue>) -> Self {
        Self::array(None, items)
    }

    pub fn new_function(function: Rc<JsFunction>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype: None,
            kind: ObjectKind::Function(function),
            extensible: true,
            class_name: "Function",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn boxed_string(value: JsString, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::BoxedString(value),
            extensible: true,
            class_name: "String",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn boxed_number(value: f64, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::BoxedNumber(value),
            extensible: true,
            class_name: "Number",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn boxed_bool(value: bool, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::BoxedBool(value),
            extensible: true,
            class_name: "Boolean",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn map(prototype: Option<ObjRef>, entries: Vec<(JsValue, JsValue)>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Map(entries),
            extensible: true,
            class_name: "Map",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn promise(prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Promise(Box::new(PromiseData {
                state: PromiseState::Pending,
                value: JsValue::Undefined,
                reactions: Vec::new(),
                handled: false,
            })),
            extensible: true,
            class_name: "Promise",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn set(prototype: Option<ObjRef>, values: Vec<JsValue>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Set(values),
            extensible: true,
            class_name: "Set",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn error(prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Error,
            extensible: true,
            class_name: "Error",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn array_buffer(bytes: ByteBuffer, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::ArrayBuffer(ArrayBufferData { bytes }),
            extensible: true,
            class_name: "ArrayBuffer",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn data_view(
        buffer: ObjRef,
        bytes: ByteBuffer,
        byte_offset: usize,
        byte_length: usize,
        prototype: Option<ObjRef>,
    ) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::DataView(DataViewData {
                buffer,
                bytes,
                byte_offset,
                byte_length,
            }),
            extensible: true,
            class_name: "DataView",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn typed_array(data: TypedArrayData, prototype: Option<ObjRef>) -> Self {
        let class_name = data.kind.name();
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::TypedArray(data),
            extensible: true,
            class_name,
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn date(value: f64, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Date(value),
            extensible: true,
            class_name: "Date",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn regexp(data: Rc<RegExpData>, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::RegExp(data),
            extensible: true,
            class_name: "RegExp",
            shape: 0,
            is_prototype: false,
        }
    }

    pub fn is_function(&self) -> bool {
        matches!(self.kind, ObjectKind::Function(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self.kind, ObjectKind::Array(_))
    }

    pub fn is_error(&self) -> bool {
        matches!(self.kind, ObjectKind::Error)
    }

    pub fn function(&self) -> Option<Rc<JsFunction>> {
        match &self.kind {
            ObjectKind::Function(function) => Some(function.clone()),
            _ => None,
        }
    }

    /// Dense elements with holes flattened to `undefined`; array methods that
    /// need hole visibility go through `array_holes` instead.
    pub fn array_items(&self) -> Option<Vec<JsValue>> {
        match &self.kind {
            ObjectKind::Array(data) => Some(
                data.items
                    .iter()
                    .map(|item| item.clone().unwrap_or(JsValue::Undefined))
                    .collect(),
            ),
            _ => None,
        }
    }

    pub fn array_len(&self) -> Option<usize> {
        match &self.kind {
            ObjectKind::Array(data) => Some(data.length),
            _ => None,
        }
    }

    pub fn define(&mut self, key: impl Into<PropKey>, property: Property) {
        self.shape = self.shape.wrapping_add(1);
        if self.is_prototype {
            PROTOTYPE_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
        self.props.insert(key.into().canonical(), property);
    }

    pub fn define_value(&mut self, key: impl Into<PropKey>, value: JsValue) {
        self.define(key, Property::data(value));
    }

    pub fn get_own(&self, key: &PropKey) -> Option<&Property> {
        self.property(key)
    }

    /// Fast indexed array/typed-array read used by the VM before it builds a
    /// `PropKey`, like `js_get_fast_array_element` (quickjs.c:9969).
    #[inline(always)]
    pub fn get_index_value(&self, index: i32) -> Option<JsValue> {
        if index < 0 {
            return None;
        }
        let index = index as usize;
        match &self.kind {
            ObjectKind::Array(data) => data
                .items
                .get(index)
                .and_then(|item| item.as_ref().cloned()),
            ObjectKind::TypedArray(data) if index < data.length => Some(data.get(index)),
            _ => None,
        }
    }

    /// Fast indexed array/typed-array write. Returns false when the receiver
    /// is not an indexed object and the generic property path must run, like
    /// `JS_SetPropertyValue` (quickjs.c:10830). Beyond `DENSE_LIMIT` the write
    /// goes through the sparse property map.
    #[inline(always)]
    pub fn set_index_value(&mut self, realm: &mut Realm, index: i32, value: &JsValue) -> bool {
        if index < 0 {
            return false;
        }
        let index = index as usize;
        match &mut self.kind {
            ObjectKind::Array(data) if index < data.items.len() => {
                data.items[index] = Some(value.clone());
                true
            }
            ObjectKind::Array(data) if index < crate::coerce::DENSE_LIMIT => {
                data.items.resize(index + 1, None);
                data.items[index] = Some(value.clone());
                data.length = data.length.max(index + 1);
                true
            }
            ObjectKind::TypedArray(data) if index < data.length => {
                data.set(realm, index, value);
                true
            }
            _ => false,
        }
    }

    pub fn has_own(&self, key: &PropKey) -> bool {
        if self.has_array_key(key) {
            return true;
        }
        self.property(key).is_some()
    }

    /// Fast property lookup for already-canonical VM keys. Only numeric
    /// string keys need a canonical fallback, mirroring QuickJS' atom keys.
    fn property(&self, key: &PropKey) -> Option<&Property> {
        self.props.get(key).or_else(|| {
            if matches!(key, PropKey::Str(_)) {
                let canonical = key.canonical();
                self.props.get(&canonical)
            } else {
                None
            }
        })
    }

    pub fn has_property(&self, key: &PropKey) -> bool {
        if self.has_own(key) {
            return true;
        }
        match &self.prototype {
            Some(proto) => proto.borrow().has_property(key),
            None => false,
        }
    }

    fn has_array_key(&self, key: &PropKey) -> bool {
        match &self.kind {
            ObjectKind::Array(data) => {
                if matches!(key, PropKey::Str(text) if text.as_ref() == "length") {
                    return true;
                }
                // Holes are not own properties, like a slow QuickJS array.
                key.index()
                    .is_some_and(|index| data.items.get(index).is_some_and(Option::is_some))
            }
            ObjectKind::BoxedString(value) => {
                if matches!(key, PropKey::Str(text) if text.as_ref() == "length") {
                    return true;
                }
                key.index().is_some_and(|index| index < utf16_len(value))
            }
            ObjectKind::TypedArray(value) => {
                matches!(key, PropKey::Str(text) if matches!(
                    text.as_ref(),
                    "length" | "byteLength" | "byteOffset" | "buffer"
                )) || key.index().is_some_and(|index| index < value.length)
            }
            _ => false,
        }
    }

    pub fn get_own_value(&self, key: &PropKey) -> Option<JsValue> {
        if let Some(value) = self.get_array_value(key) {
            return Some(value);
        }
        self.property(key).map(|property| {
            if property.is_accessor() {
                JsValue::Undefined
            } else {
                property.value.clone()
            }
        })
    }

    fn get_array_value(&self, key: &PropKey) -> Option<JsValue> {
        match &self.kind {
            ObjectKind::Array(data) => {
                if matches!(key, PropKey::Str(text) if text.as_ref() == "length") {
                    return Some(JsValue::number(data.length as f64));
                }
                key.index()
                    .and_then(|index| data.items.get(index))
                    .and_then(|item| item.clone())
            }
            ObjectKind::BoxedString(value) => {
                if matches!(key, PropKey::Str(text) if text.as_ref() == "length") {
                    return Some(JsValue::number(utf16_len(value) as f64));
                }
                key.index()
                    .filter(|index| *index < utf16_len(value))
                    .and_then(|index| utf16_char(value, index))
                    .map(JsValue::string)
            }
            ObjectKind::TypedArray(value) => match key {
                PropKey::Str(text) => match text.as_ref() {
                    "length" => Some(JsValue::number(value.length as f64)),
                    "byteLength" => Some(JsValue::number(value.byte_length() as f64)),
                    "byteOffset" => Some(JsValue::number(value.byte_offset as f64)),
                    "buffer" => Some(JsValue::Object(value.buffer.clone())),
                    // Numeric string keys reach here from generic property paths
                    // (`Array.prototype.join` etc.), like QuickJS' tagged-int
                    // atom conversion in `JS_NewAtomStr` (quickjs.c:3697).
                    _ => key
                        .index()
                        .filter(|index| *index < value.length)
                        .map(|index| value.get(index)),
                },
                PropKey::Index(index) => Some(value.get(*index)),
                PropKey::Symbol(_) => None,
            },
            _ => None,
        }
    }

    /// Resolves a property without invoking accessors, so callers can drop the
    /// `RefCell` borrow before re-entering the VM. QuickJS performs the same
    /// split between lookup and `JS_GetPropertyInternal` (quickjs.c:9110).
    pub fn resolve_property(&self, key: &PropKey) -> ResolvedProperty {
        if let Some(value) = self.get_array_value(key) {
            let writable = match &self.kind {
                ObjectKind::Array(_) => true,
                ObjectKind::BoxedString(_) => false,
                ObjectKind::TypedArray(_) => key.index().is_some(),
                _ => true,
            };
            return ResolvedProperty::Data {
                value,
                writable,
                own: true,
            };
        }
        if let Some(property) = self.property(key) {
            if property.is_accessor() {
                return ResolvedProperty::Accessor {
                    get: property.get.as_deref().cloned(),
                    set: property.set.as_deref().cloned(),
                    own: true,
                };
            }
            return ResolvedProperty::Data {
                value: property.value.clone(),
                writable: property.writable,
                own: true,
            };
        }
        match &self.prototype {
            Some(prototype) => {
                let resolved = prototype.borrow().resolve_property(key);
                match resolved {
                    ResolvedProperty::Data {
                        value,
                        writable,
                        own: _,
                    } => ResolvedProperty::Data {
                        value,
                        writable,
                        own: false,
                    },
                    ResolvedProperty::Accessor { get, set, own: _ } => ResolvedProperty::Accessor {
                        get,
                        set,
                        own: false,
                    },
                    ResolvedProperty::Missing => ResolvedProperty::Missing,
                }
            }
            None => ResolvedProperty::Missing,
        }
    }
}

mod data;
mod property;
mod wrappers;

pub use data::*;
pub use wrappers::*;
