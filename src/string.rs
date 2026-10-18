//! JavaScript string values.
//!
//! QuickJS stores strings as heap objects (`JSString`) and keeps the `JSValue`
//! payload a single pointer. `Rc<str>` would be a fat pointer and make every
//! `JSValue` 24 bytes; a thin `Rc<CompactString>` keeps `JsValue` at 16 bytes
//! while still inlining short strings (the common case for atoms, property
//! names and generated keys) in the allocation.
//!
//! Equality checks the pointer first: property names created from the same
//! bytecode constant share one allocation, so the common map/IC comparison
//! does not touch the bytes.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::rc::Rc;

use compact_str::CompactString;

/// GC-independent reference-counted string, the `JSString *` payload of a
/// `JSValue`.
#[derive(Clone)]
pub struct JsString(Rc<CompactString>);

impl JsString {
    #[inline]
    pub fn new(value: impl Into<CompactString>) -> Self {
        Self(Rc::new(value.into()))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    #[inline]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    #[inline]
    pub fn into_string(self) -> String {
        self.0.as_str().to_owned()
    }
}

impl Deref for JsString {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        self.0.as_str()
    }
}

impl AsRef<str> for JsString {
    #[inline]
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl PartialEq for JsString {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other) || self.0 == other.0
    }
}

impl Eq for JsString {}

impl Hash for JsString {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_str().hash(state);
    }
}

impl fmt::Debug for JsString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0.as_str(), formatter)
    }
}

impl fmt::Display for JsString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0.as_str())
    }
}

impl Default for JsString {
    fn default() -> Self {
        Self::new("")
    }
}

impl From<&str> for JsString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<char> for JsString {
    fn from(value: char) -> Self {
        let mut buffer = [0u8; 4];
        let text = value.encode_utf8(&mut buffer);
        Self::new(&*text)
    }
}

impl From<String> for JsString {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&String> for JsString {
    fn from(value: &String) -> Self {
        Self::new(value.as_str())
    }
}

impl From<Rc<str>> for JsString {
    fn from(value: Rc<str>) -> Self {
        Self::new(value.as_ref())
    }
}

impl From<JsString> for String {
    fn from(value: JsString) -> Self {
        value.into_string()
    }
}

impl PartialEq<str> for JsString {
    fn eq(&self, other: &str) -> bool {
        self.0.as_str() == other
    }
}

impl PartialEq<&str> for JsString {
    fn eq(&self, other: &&str) -> bool {
        self.0.as_str() == *other
    }
}
