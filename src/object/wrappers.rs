//! Public object handles (`Object`, `Function`, `Array`) and key helpers,
//! the rquickjs-style API surface over `ObjectData`.

use super::*;
use crate::gc::{Gc, GcRefCell};

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
                data.items.resize(index + 1, None);
            }
            data.items[index] = Some(value);
            data.length = data.length.max(index + 1);
            return;
        }
        object.define(key, Property::data(value));
    }

    pub fn set_key(&self, key: PropKey, value: JsValue) {
        let mut object = self.inner.borrow_mut();
        object.define(key, Property::data(value));
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
            ObjectKind::Array(data) => data
                .items
                .get(index)
                .cloned()
                .flatten()
                .unwrap_or(JsValue::Undefined),
            _ => JsValue::Undefined,
        }
    }

    pub fn set_index(&self, index: usize, value: impl Into<JsValue>) {
        let value = value.into();
        let mut object = self.inner.borrow_mut();
        if let ObjectKind::Array(data) = &mut object.kind {
            if index >= data.items.len() {
                data.items.resize(index + 1, None);
            }
            data.items[index] = Some(value);
            data.length = data.length.max(index + 1);
        }
    }

    pub fn push(&self, value: impl Into<JsValue>) -> usize {
        let value = value.into();
        let mut object = self.inner.borrow_mut();
        if let ObjectKind::Array(data) = &mut object.kind {
            data.items.push(Some(value));
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
        Object::from_ref(Gc::new(GcRefCell::new(ObjectData::new_function(
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
        if Gc::ptr_eq(&candidate, object) {
            return true;
        }
        current = candidate.borrow().prototype.clone();
    }
    false
}

pub fn key_from_string(value: crate::string::JsString) -> PropKey {
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
    // ASCII maps one-to-one to UTF-16 code units; the check is vectorized
    // and avoids decoding the whole string for the common case.
    if value.is_ascii() {
        value.len()
    } else {
        value.encode_utf16().count()
    }
}

pub fn utf16_unit(value: &str, index: usize) -> Option<u16> {
    if value.is_ascii() {
        value.as_bytes().get(index).map(|byte| *byte as u16)
    } else {
        value.encode_utf16().nth(index)
    }
}

pub fn utf16_char(value: &str, index: usize) -> Option<char> {
    let unit = utf16_unit(value, index)?;
    char::decode_utf16([unit])
        .next()
        .and_then(std::result::Result::ok)
        .or(Some('\u{FFFD}'))
}

/// Slice by UTF-16 code-unit indices. ASCII strings use a direct byte slice;
/// other strings go through UTF-16 so code-unit semantics stay correct.
pub fn utf16_slice(value: &str, start: usize, end: usize) -> String {
    if value.is_ascii() {
        return value.get(start..end).unwrap_or("").to_owned();
    }
    let units: Vec<u16> = value.encode_utf16().collect();
    let start = start.min(units.len());
    let end = end.min(units.len()).max(start);
    String::from_utf16_lossy(&units[start..end])
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
