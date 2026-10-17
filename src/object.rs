use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use indexmap::IndexMap;
use rustc_hash::FxBuildHasher;

pub type PropertyMap = IndexMap<PropKey, Property, FxBuildHasher>;

use crate::bytecode::FunctionDef;
use crate::runtime::Realm;
use crate::value::{EnvRef, JsResult, JsValue};

pub type ObjRef = Rc<RefCell<ObjectData>>;
pub type NativeFn = fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue>;

#[derive(Clone, Debug)]
pub enum PropKey {
    Str(Rc<str>),
    Index(usize),
    Symbol(u32),
}

impl From<&str> for PropKey {
    fn from(value: &str) -> Self {
        Self::Str(Rc::from(value))
    }
}

impl From<&String> for PropKey {
    fn from(value: &String) -> Self {
        Self::Str(Rc::from(value.as_str()))
    }
}

impl From<String> for PropKey {
    fn from(value: String) -> Self {
        Self::Str(Rc::from(value))
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

    pub fn to_rc_string(&self) -> Option<Rc<str>> {
        match self {
            Self::Str(value) => Some(value.clone()),
            Self::Index(index) => Some(Rc::from(index.to_string())),
            Self::Symbol(_) => None,
        }
    }

    fn canonical(&self) -> PropKey {
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
    pub get: Option<JsValue>,
    pub set: Option<JsValue>,
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
            get,
            set,
            writable: false,
            enumerable: true,
            configurable: true,
        }
    }

    pub fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }
}

#[derive(Clone)]
pub struct ArrayData {
    pub items: Vec<JsValue>,
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

    pub fn set(&self, index: usize, value: &JsValue) {
        if index >= self.length {
            return;
        }
        let number = crate::vm::to_number(value.clone()).unwrap_or(f64::NAN);
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
    pub fn native(name: &str, length: u32, function: NativeFn) -> Self {
        Self {
            name: Rc::from(name),
            length,
            native: Some(function),
            def: None,
            env: None,
            bound: None,
            lexical_this: None,
            instance_init: None,
        }
    }

    pub fn is_native(&self) -> bool {
        self.native.is_some()
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
    Promise(PromiseData),
    ArrayBuffer(ArrayBufferData),
    DataView(DataViewData),
    TypedArray(TypedArrayData),
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromiseState {
    Pending,
    Fulfilled,
    Rejected,
}

#[derive(Clone)]
pub struct PromiseData {
    pub state: PromiseState,
    pub value: JsValue,
}

/// Object representation corresponding to QuickJS `JSObject` (quickjs.c:5900
/// area): shape/properties, prototype and an optional exotic payload. The
/// insertion-ordered map is the safe Rust stand-in for `JSShape`.
pub struct ObjectData {
    pub props: PropertyMap,
    pub prototype: Option<ObjRef>,
    pub kind: ObjectKind,
    pub extensible: bool,
    pub class_name: &'static str,
}

impl ObjectData {
    pub fn ordinary(prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Ordinary,
            extensible: true,
            class_name: "Object",
        }
    }

    pub fn array(prototype: Option<ObjRef>, items: Vec<JsValue>) -> Self {
        let length = items.len();
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Array(ArrayData { items, length }),
            extensible: true,
            class_name: "Array",
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
        }
    }

    pub fn boxed_string(value: Rc<str>, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::BoxedString(value),
            extensible: true,
            class_name: "String",
        }
    }

    pub fn boxed_number(value: f64, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::BoxedNumber(value),
            extensible: true,
            class_name: "Number",
        }
    }

    pub fn boxed_bool(value: bool, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::BoxedBool(value),
            extensible: true,
            class_name: "Boolean",
        }
    }

    pub fn map(prototype: Option<ObjRef>, entries: Vec<(JsValue, JsValue)>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Map(entries),
            extensible: true,
            class_name: "Map",
        }
    }

    pub fn promise(prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Promise(PromiseData {
                state: PromiseState::Pending,
                value: JsValue::Undefined,
            }),
            extensible: true,
            class_name: "Promise",
        }
    }

    pub fn set(prototype: Option<ObjRef>, values: Vec<JsValue>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Set(values),
            extensible: true,
            class_name: "Set",
        }
    }

    pub fn error(prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Error,
            extensible: true,
            class_name: "Error",
        }
    }

    pub fn array_buffer(bytes: ByteBuffer, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::ArrayBuffer(ArrayBufferData { bytes }),
            extensible: true,
            class_name: "ArrayBuffer",
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
        }
    }

    pub fn date(value: f64, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::Date(value),
            extensible: true,
            class_name: "Date",
        }
    }

    pub fn regexp(data: Rc<RegExpData>, prototype: Option<ObjRef>) -> Self {
        Self {
            props: PropertyMap::default(),
            prototype,
            kind: ObjectKind::RegExp(data),
            extensible: true,
            class_name: "RegExp",
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

    pub fn array_items(&self) -> Option<&[JsValue]> {
        match &self.kind {
            ObjectKind::Array(data) => Some(&data.items),
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
        self.props.insert(key.into().canonical(), property);
    }

    pub fn define_value(&mut self, key: impl Into<PropKey>, value: JsValue) {
        self.define(key, Property::data(value));
    }

    pub fn get_own(&self, key: &PropKey) -> Option<&Property> {
        self.props.get(&key.canonical())
    }

    pub fn has_own(&self, key: &PropKey) -> bool {
        if self.has_array_key(key) {
            return true;
        }
        self.props.contains_key(&key.canonical())
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
                key.index().is_some_and(|index| index < data.length)
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
        self.props.get(&key.canonical()).map(|property| {
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
                    .filter(|index| *index < data.length)
                    .map(|index| data.items[index].clone())
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
                    _ => None,
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
        if let Some(property) = self.props.get(&key.canonical()) {
            if property.is_accessor() {
                return ResolvedProperty::Accessor {
                    get: property.get.clone(),
                    set: property.set.clone(),
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

    pub fn get_property(
        &self,
        realm: &mut Realm,
        key: &PropKey,
        receiver: &JsValue,
    ) -> JsResult<JsValue> {
        if let Some(value) = self.get_array_value(key) {
            return Ok(value);
        }
        if let Some(property) = self.props.get(&key.canonical()) {
            if property.is_accessor() {
                return match &property.get {
                    Some(getter) => {
                        crate::vm::call_value(realm, getter.clone(), receiver.clone(), &[])
                    }
                    None => Ok(JsValue::Undefined),
                };
            }
            return Ok(property.value.clone());
        }
        match &self.prototype {
            Some(proto) => proto.borrow().get_property(realm, key, receiver),
            None => Ok(JsValue::Undefined),
        }
    }

    pub fn put_property(
        &mut self,
        realm: &mut Realm,
        key: PropKey,
        value: JsValue,
        receiver: &JsValue,
    ) -> JsResult<()> {
        if let Some(result) = self.put_array_value(realm, &key, &value)? {
            return Ok(result);
        }
        if let Some(property) = self.props.get(&key.canonical()) {
            if property.is_accessor() {
                return match &property.set {
                    Some(setter) => {
                        crate::vm::call_value(realm, setter.clone(), receiver.clone(), &[value])?;
                        Ok(())
                    }
                    None => Ok(()),
                };
            }
            if !property.writable {
                return Ok(());
            }
            if let Some(property) = self.props.get_mut(&key.canonical()) {
                property.value = value;
            }
            return Ok(());
        }
        if let Some(proto) = &self.prototype {
            let mut current = Some(proto.clone());
            while let Some(proto) = current {
                let cursor = proto.borrow();
                if let Some(property) = cursor.props.get(&key.canonical()) {
                    if property.is_accessor() {
                        return match &property.set {
                            Some(setter) => {
                                crate::vm::call_value(
                                    realm,
                                    setter.clone(),
                                    receiver.clone(),
                                    &[value],
                                )?;
                                Ok(())
                            }
                            None => Ok(()),
                        };
                    }
                    if !property.writable {
                        return Ok(());
                    }
                    break;
                }
                current = cursor.prototype.clone();
            }
        }
        if self.extensible {
            self.props.insert(key.canonical(), Property::data(value));
        }
        Ok(())
    }

    pub fn put_array_value(
        &mut self,
        realm: &mut Realm,
        key: &PropKey,
        value: &JsValue,
    ) -> JsResult<Option<()>> {
        if let PropKey::Index(index) = key {
            match &self.kind {
                ObjectKind::TypedArray(data) => {
                    data.set(*index, value);
                    return Ok(Some(()));
                }
                ObjectKind::Array(_data) => {
                    let index = *index;
                    if let ObjectKind::Array(data) = &mut self.kind {
                        if index >= data.items.len() {
                            data.items.resize(index + 1, JsValue::Undefined);
                        }
                        data.items[index] = value.clone();
                        data.length = data.length.max(index + 1);
                    }
                    return Ok(Some(()));
                }
                _ => {}
            }
            return Ok(None);
        }
        let PropKey::Str(text) = key else {
            return Ok(None);
        };
        if let ObjectKind::TypedArray(data) = &self.kind
            && let Some(index) = key_to_index(text)
        {
            data.set(index, value);
            return Ok(Some(()));
        }
        if let ObjectKind::Array(data) = &mut self.kind {
            if text.as_ref() == "length" {
                let length = crate::vm::to_number(value.clone())?;
                let length = if length.is_nan() || length < 0.0 || length > 4_294_967_295.0 {
                    return Err(crate::vm::range_error(realm, "Invalid array length"));
                } else {
                    length as usize
                };
                if length < data.items.len() {
                    data.items.truncate(length);
                } else {
                    data.items.resize(length, JsValue::Undefined);
                }
                data.length = length;
                return Ok(Some(()));
            }
            if let Some(index) = key_to_index(text) {
                if index >= data.items.len() {
                    data.items.resize(index + 1, JsValue::Undefined);
                }
                data.items[index] = value.clone();
                data.length = data.length.max(index + 1);
                return Ok(Some(()));
            }
        }
        Ok(None)
    }

    pub fn delete_property(&mut self, key: &PropKey) -> bool {
        if let ObjectKind::Array(data) = &mut self.kind {
            if let Some(index) = key.index()
                && index < data.items.len()
            {
                data.items[index] = JsValue::Undefined;
                return true;
            }
            if matches!(key, PropKey::Str(text) if text.as_ref() == "length") {
                return false;
            }
        }
        self.props.shift_remove(&key.canonical()).is_some()
    }

    pub fn own_keys(&self) -> Vec<PropKey> {
        let mut keys = Vec::new();
        if let ObjectKind::Array(data) = &self.kind {
            keys.extend((0..data.length).map(PropKey::Index));
        }
        if let ObjectKind::TypedArray(data) = &self.kind {
            keys.extend((0..data.length).map(PropKey::Index));
        }
        keys.extend(self.props.keys().cloned());
        keys
    }

    pub fn enumerable_keys(&self) -> Vec<PropKey> {
        let mut keys = Vec::new();
        if let ObjectKind::Array(data) = &self.kind {
            keys.extend((0..data.length).map(PropKey::Index));
        }
        if let ObjectKind::TypedArray(data) = &self.kind {
            keys.extend((0..data.length).map(PropKey::Index));
        }
        keys.extend(
            self.props
                .iter()
                .filter(|(_, property)| property.enumerable)
                .map(|(key, _)| key.clone()),
        );
        keys
    }

    pub fn proto(&self) -> Option<Object> {
        self.prototype
            .as_ref()
            .map(|proto| Object::from_ref(proto.clone()))
    }

    pub fn into_handle(self) -> Object {
        Object::from_ref(Rc::new(RefCell::new(self)))
    }

    pub fn set_proto(&mut self, proto: Option<ObjRef>) {
        self.prototype = proto;
    }
}

#[derive(Clone)]
pub struct Object {
    inner: ObjRef,
}

impl Object {
    pub fn from_value(value: &JsValue) -> Option<Self> {
        value.as_object()
    }

    pub fn get(&self, key: &str) -> JsValue {
        let key = PropKey::from(key);
        let object = self.inner.borrow();
        match object.get_own_value(&key) {
            Some(value) => value,
            None => {
                let prototype = object.prototype.clone();
                drop(object);
                traverse_property(prototype, &key)
            }
        }
    }

    pub fn get_key(&self, key: &PropKey) -> JsValue {
        let object = self.inner.borrow();
        match object.get_own_value(key) {
            Some(value) => value,
            None => {
                let prototype = object.prototype.clone();
                drop(object);
                traverse_property(prototype, key)
            }
        }
    }

    pub fn set(&self, key: &str, value: impl Into<JsValue>) {
        let key = PropKey::from(key);
        let value = value.into();
        let mut object = self.inner.borrow_mut();
        if let ObjectKind::Array(data) = &mut object.kind
            && let Some(index) = key.index()
        {
            if index >= data.items.len() {
                data.items.resize(index + 1, JsValue::Undefined);
            }
            data.items[index] = value;
            data.length = data.length.max(index + 1);
            return;
        }
        object.props.insert(key.canonical(), Property::data(value));
    }

    pub fn set_key(&self, key: PropKey, value: JsValue) {
        self.inner
            .borrow_mut()
            .props
            .insert(key.canonical(), Property::data(value));
    }

    pub fn define(&self, key: &str, property: Property) {
        self.inner.borrow_mut().define(key, property);
    }

    pub fn define_key(&self, key: PropKey, property: Property) {
        self.inner.borrow_mut().define(key, property);
    }

    pub fn has(&self, key: &str) -> bool {
        self.inner.borrow().has_property(&PropKey::from(key))
    }

    pub fn delete(&self, key: &str) -> bool {
        self.inner.borrow_mut().delete_property(&PropKey::from(key))
    }

    pub fn keys(&self) -> Vec<String> {
        self.inner
            .borrow()
            .own_keys()
            .into_iter()
            .filter_map(|key| key.to_rc_string().map(|value| value.to_string()))
            .collect()
    }

    pub fn enumerable_keys(&self) -> Vec<String> {
        self.inner
            .borrow()
            .enumerable_keys()
            .into_iter()
            .filter_map(|key| key.to_rc_string().map(|value| value.to_string()))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.inner.borrow().array_len().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_array(&self) -> bool {
        self.inner.borrow().is_array()
    }

    pub fn is_function(&self) -> bool {
        self.inner.borrow().is_function()
    }

    pub fn get_index(&self, index: usize) -> JsValue {
        let object = self.inner.borrow();
        match &object.kind {
            ObjectKind::Array(data) => data.items.get(index).cloned().unwrap_or(JsValue::Undefined),
            _ => JsValue::Undefined,
        }
    }

    pub fn set_index(&self, index: usize, value: impl Into<JsValue>) {
        let value = value.into();
        let mut object = self.inner.borrow_mut();
        if let ObjectKind::Array(data) = &mut object.kind {
            if index >= data.items.len() {
                data.items.resize(index + 1, JsValue::Undefined);
            }
            data.items[index] = value;
            data.length = data.length.max(index + 1);
        }
    }

    pub fn push(&self, value: impl Into<JsValue>) -> usize {
        let value = value.into();
        let mut object = self.inner.borrow_mut();
        if let ObjectKind::Array(data) = &mut object.kind {
            data.items.push(value);
            data.length = data.items.len();
            return data.length;
        }
        0
    }

    pub fn to_ref(&self) -> ObjRef {
        self.inner.clone()
    }

    pub fn from_ref(inner: ObjRef) -> Self {
        Self { inner }
    }

    pub fn into_ref(self) -> ObjRef {
        self.inner
    }
}

fn traverse_property(mut prototype: Option<ObjRef>, key: &PropKey) -> JsValue {
    while let Some(proto) = prototype {
        let object = proto.borrow();
        if let Some(value) = object.get_own_value(key) {
            return value;
        }
        prototype = object.prototype.clone();
    }
    JsValue::Undefined
}

#[derive(Clone)]
pub struct Function {
    inner: Rc<JsFunction>,
}

impl Function {
    pub fn from_ref(inner: Rc<JsFunction>) -> Self {
        Self { inner }
    }

    pub fn name(&self) -> Rc<str> {
        self.inner.name.clone()
    }

    pub fn to_ref(&self) -> Rc<JsFunction> {
        self.inner.clone()
    }

    pub fn as_object(&self) -> Object {
        Object::from_ref(Rc::new(RefCell::new(ObjectData::new_function(
            self.inner.clone(),
        ))))
    }
}

#[derive(Clone)]
pub struct Array {
    object: Object,
}

impl Array {
    pub fn from_object(object: Object) -> Option<Self> {
        object.is_array().then_some(Self { object })
    }

    pub fn len(&self) -> usize {
        self.object.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> JsValue {
        self.object.get_index(index)
    }

    pub fn set(&self, index: usize, value: impl Into<JsValue>) {
        self.object.set_index(index, value);
    }

    pub fn push(&self, value: impl Into<JsValue>) -> usize {
        self.object.push(value)
    }

    pub fn iter(&self) -> impl Iterator<Item = JsValue> {
        let items = self
            .object
            .inner
            .borrow()
            .array_items()
            .map(|items| items.to_vec())
            .unwrap_or_default();
        items.into_iter()
    }

    pub fn into_object(self) -> Object {
        self.object
    }
}

impl IntoIterator for Array {
    type Item = JsValue;
    type IntoIter = std::vec::IntoIter<JsValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter().collect::<Vec<_>>().into_iter()
    }
}

pub fn would_create_cycle(object: &ObjRef, prototype: &ObjRef) -> bool {
    let mut current = Some(prototype.clone());
    while let Some(candidate) = current {
        if Rc::ptr_eq(&candidate, object) {
            return true;
        }
        current = candidate.borrow().prototype.clone();
    }
    false
}

pub fn key_from_string(value: Rc<str>) -> PropKey {
    key_to_index(&value)
        .map(PropKey::Index)
        .unwrap_or(PropKey::Str(value))
}

pub fn key_to_index(key: &str) -> Option<usize> {
    if key.is_empty() || key.len() > 10 || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if key.len() > 1 && key.starts_with('0') {
        return None;
    }
    key.parse().ok()
}

pub fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

pub fn utf16_char(value: &str, index: usize) -> Option<char> {
    value.encode_utf16().nth(index).map(|unit| {
        char::decode_utf16([unit])
            .next()
            .and_then(std::result::Result::ok)
            .unwrap_or('\u{FFFD}')
    })
}

impl PartialEq for PropKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Index(a), Self::Index(b)) => a == b,
            (Self::Symbol(a), Self::Symbol(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for PropKey {}

impl Hash for PropKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Str(value) => value.hash(state),
            Self::Index(value) => value.hash(state),
            Self::Symbol(value) => value.hash(state),
        }
    }
}
