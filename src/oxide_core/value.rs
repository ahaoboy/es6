//! 8-byte NaN-boxed JavaScript value.
//!
//! Bit layout follows quickjs-oxide's phase-1 core: every word below
//! `0xFFF9_0000_0000_0000` is a double; the high 16 bits select a boxed kind
//! and the low 32 bits carry an arena index or an inline payload.
//!
//! This module is deliberately independent of the current `JsValue` while
//! the new heap is brought up.

use std::fmt;

const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;

const TAG_INT32: u64 = 0xFFF9_0000_0000_0000;
const TAG_SPECIAL: u64 = 0xFFFA_0000_0000_0000;
const TAG_OBJECT: u64 = 0xFFFB_0000_0000_0000;
const TAG_STRING: u64 = 0xFFFC_0000_0000_0000;
const TAG_SYMBOL: u64 = 0xFFFD_0000_0000_0000;
const TAG_BIGINT: u64 = 0xFFFE_0000_0000_0000;

const SPECIAL_UNDEFINED: u32 = 1;
const SPECIAL_NULL: u32 = 2;
const SPECIAL_EMPTY: u32 = 3;
const SPECIAL_BOOL_BIT: u32 = 1 << 2;

const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;

/// A 64-bit JavaScript value. Moving or copying it never touches a refcount.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Value(pub(crate) u64);

const _: () = assert!(std::mem::size_of::<Value>() == 8);

/// The observable kind of a [`Value`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Int32,
    Float64,
    Bool,
    Undefined,
    Null,
    Empty,
    Object,
    String,
    Symbol,
    BigInt,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ObjectId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StringId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SymbolId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BigIntId(pub u32);

impl ObjectId {
    pub const NONE: Self = Self(u32::MAX);
}

impl Value {
    pub const UNDEFINED: Self = Self(TAG_SPECIAL | SPECIAL_UNDEFINED as u64);
    pub const NULL: Self = Self(TAG_SPECIAL | SPECIAL_NULL as u64);
    /// Internal hole marker; never observable from JavaScript.
    pub const EMPTY: Self = Self(TAG_SPECIAL | SPECIAL_EMPTY as u64);
    pub const TRUE: Self = Self(TAG_SPECIAL | SPECIAL_BOOL_BIT as u64);
    pub const FALSE: Self = Self(TAG_SPECIAL);

    #[inline(always)]
    pub const fn int32(value: i32) -> Self {
        Self(TAG_INT32 | value as u32 as u64)
    }

    #[inline]
    pub fn number(value: f64) -> Self {
        let bits = value.to_bits();
        if bits >= TAG_INT32 {
            // A NaN must never alias one of the boxed tags.
            Self(if value.is_nan() { CANONICAL_NAN } else { bits })
        } else {
            Self(bits)
        }
    }

    #[inline(always)]
    pub const fn boolean(value: bool) -> Self {
        if value { Self::TRUE } else { Self::FALSE }
    }

    #[inline(always)]
    pub const fn object(id: ObjectId) -> Self {
        Self(TAG_OBJECT | id.0 as u64)
    }

    #[inline(always)]
    pub const fn string(id: StringId) -> Self {
        Self(TAG_STRING | id.0 as u64)
    }

    #[inline(always)]
    pub const fn symbol(id: SymbolId) -> Self {
        Self(TAG_SYMBOL | id.0 as u64)
    }

    #[inline(always)]
    pub const fn bigint(id: BigIntId) -> Self {
        Self(TAG_BIGINT | id.0 as u64)
    }

    #[inline(always)]
    pub const fn raw(self) -> u64 {
        self.0
    }

    #[inline(always)]
    pub const fn is_number(self) -> bool {
        self.0 < TAG_SPECIAL
    }

    #[inline(always)]
    pub const fn is_int32(self) -> bool {
        self.0 & TAG_MASK == TAG_INT32
    }

    #[inline(always)]
    pub const fn is_float64(self) -> bool {
        self.0 < TAG_INT32
    }

    #[inline(always)]
    pub const fn as_int32(self) -> Option<i32> {
        if self.is_int32() {
            Some(self.0 as u32 as i32)
        } else {
            None
        }
    }

    /// Numeric view: doubles and Int32 values, as `ToNumber` would see them.
    #[inline(always)]
    pub fn as_f64(self) -> Option<f64> {
        if self.is_int32() {
            Some(self.0 as u32 as i32 as f64)
        } else if self.is_float64() {
            Some(f64::from_bits(self.0))
        } else {
            None
        }
    }

    #[inline]
    pub fn kind(self) -> ValueKind {
        if self.0 < TAG_SPECIAL {
            return if self.is_int32() {
                ValueKind::Int32
            } else {
                ValueKind::Float64
            };
        }
        match self.0 & TAG_MASK {
            TAG_SPECIAL => match self.0 as u32 & !SPECIAL_BOOL_BIT {
                SPECIAL_UNDEFINED => ValueKind::Undefined,
                SPECIAL_NULL => ValueKind::Null,
                SPECIAL_EMPTY => ValueKind::Empty,
                _ if self.0 as u32 & SPECIAL_BOOL_BIT != 0 => ValueKind::Bool,
                _ => ValueKind::Bool,
            },
            TAG_OBJECT => ValueKind::Object,
            TAG_STRING => ValueKind::String,
            TAG_SYMBOL => ValueKind::Symbol,
            TAG_BIGINT => ValueKind::BigInt,
            _ => unreachable!("reserved NaN box"),
        }
    }

    #[inline(always)]
    pub const fn as_object(self) -> Option<ObjectId> {
        if self.0 & TAG_MASK == TAG_OBJECT {
            Some(ObjectId(self.0 as u32))
        } else {
            None
        }
    }

    #[inline(always)]
    pub const fn as_string(self) -> Option<StringId> {
        if self.0 & TAG_MASK == TAG_STRING {
            Some(StringId(self.0 as u32))
        } else {
            None
        }
    }

    #[inline(always)]
    pub const fn as_symbol(self) -> Option<SymbolId> {
        if self.0 & TAG_MASK == TAG_SYMBOL {
            Some(SymbolId(self.0 as u32))
        } else {
            None
        }
    }

    #[inline(always)]
    pub const fn as_bigint(self) -> Option<BigIntId> {
        if self.0 & TAG_MASK == TAG_BIGINT {
            Some(BigIntId(self.0 as u32))
        } else {
            None
        }
    }

    #[inline(always)]
    pub const fn as_bool(self) -> Option<bool> {
        if self.0 & TAG_MASK == TAG_SPECIAL && self.0 as u32 & SPECIAL_BOOL_BIT != 0 {
            Some(true)
        } else if self.0 == Self::FALSE.0 {
            Some(false)
        } else {
            None
        }
    }

    #[inline(always)]
    pub const fn is_undefined(self) -> bool {
        self.0 == Self::UNDEFINED.0
    }

    #[inline(always)]
    pub const fn is_null(self) -> bool {
        self.0 == Self::NULL.0
    }

    #[inline(always)]
    pub const fn is_nullish(self) -> bool {
        self.0 == Self::UNDEFINED.0 || self.0 == Self::NULL.0
    }

    #[inline(always)]
    pub const fn is_empty(self) -> bool {
        self.0 == Self::EMPTY.0
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind() {
            ValueKind::Int32 => write!(f, "{}", self.as_int32().unwrap()),
            ValueKind::Float64 => write!(f, "{}", self.as_f64().unwrap()),
            ValueKind::Bool => write!(f, "{}", self.as_bool().unwrap()),
            ValueKind::Undefined => f.write_str("undefined"),
            ValueKind::Null => f.write_str("null"),
            ValueKind::Empty => f.write_str("<empty>"),
            ValueKind::Object => write!(f, "Object({})", self.as_object().unwrap().0),
            ValueKind::String => write!(f, "String({})", self.as_string().unwrap().0),
            ValueKind::Symbol => write!(f, "Symbol({})", self.as_symbol().unwrap().0),
            ValueKind::BigInt => write!(f, "BigInt({})", self.as_bigint().unwrap().0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ObjectId, StringId, Value, ValueKind};

    #[test]
    fn size_and_copy() {
        assert_eq!(std::mem::size_of::<Value>(), 8);
        let value = Value::int32(42);
        let copy = value;
        assert_eq!(copy, value);
    }

    #[test]
    fn numbers_keep_signed_zero_and_canonicalize_boxed_nan() {
        assert!(Value::number(0.0).as_f64().unwrap().is_sign_positive());
        assert!(Value::number(-0.0).as_f64().unwrap().is_sign_negative());
        assert_eq!(Value::number(f64::NAN).raw(), 0x7FF8_0000_0000_0000);
        assert_eq!(Value::number(1.5).kind(), ValueKind::Float64);
    }

    #[test]
    fn tagged_payloads() {
        let object = Value::object(ObjectId(7));
        assert_eq!(object.as_object(), Some(ObjectId(7)));
        assert_eq!(object.kind(), ValueKind::Object);
        let string = Value::string(StringId(9));
        assert_eq!(string.as_string(), Some(StringId(9)));
        assert_eq!(string.kind(), ValueKind::String);
        assert_eq!(Value::TRUE.as_bool(), Some(true));
        assert_eq!(Value::FALSE.as_bool(), Some(false));
        assert!(Value::UNDEFINED.is_nullish());
        assert!(Value::EMPTY.is_empty());
    }

    #[test]
    fn int32_roundtrip() {
        for value in [i32::MIN, -1, 0, 1, i32::MAX] {
            let boxed = Value::int32(value);
            assert_eq!(boxed.as_int32(), Some(value));
            assert_eq!(boxed.as_f64(), Some(value as f64));
        }
    }
}
