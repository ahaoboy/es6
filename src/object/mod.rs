use std::cell::{OnceCell, RefCell};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::gc::{Finalize, Gc, GcRefCell};

use rustc_hash::{FxBuildHasher, FxHashMap};

use crate::bytecode::FunctionDef;
use crate::runtime::Realm;
use crate::string::JsString;
use crate::value::{EnvRef, JsResult, JsValue};

pub type ObjRef = Gc<GcRefCell<ObjectData>>;
pub type NativeFn = Rc<dyn Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue>>;
/// Native function with a shared, GC-visible data cell, the equivalent of
/// QuickJS C functions with data (`js_c_function_data`, quickjs.c:6595):
/// the data array is walked by the GC exactly like QuickJS'
/// `mark_children` (quickjs.c:7255).
pub type DataFn = Rc<dyn Fn(&mut Realm, &JsValue, &[JsValue], &[JsValue]) -> JsResult<JsValue>>;
pub type NativeData = Gc<GcRefCell<Vec<JsValue>>>;
pub type WeakObjectMap = crate::gc::WeakMap<GcRefCell<ObjectData>, JsValue>;
pub type WeakObjectSet = crate::gc::WeakMap<GcRefCell<ObjectData>, ()>;

/// Shared object shape, the counterpart of QuickJS `JSShape` (quickjs.c:1102):
/// an immutable key vector plus a key-to-slot index. Objects with the same
/// property layout share one `Rc`, so creating a literal copies a pointer
/// instead of rebuilding a hash table, and inline caches validate with the
/// shape id before reading `props[slot]` directly.
#[derive(Clone)]
pub struct Shape {
    /// Monotonic identity. Never reused, so a cache entry can compare ids.
    pub id: u64,
    /// Slot order, which is also the property enumeration order.
    pub keys: Vec<PropKey>,
    pub index: OnceCell<FxHashMap<PropKey, u32>>,
}

static SHAPE_ID: AtomicU64 = AtomicU64::new(1);

fn next_shape_id() -> u64 {
    SHAPE_ID.fetch_add(1, Ordering::Relaxed)
}

thread_local! {
    static EMPTY_SHAPE: Rc<Shape> = Rc::new(Shape {
        id: 0,
        keys: Vec::new(),
        index: OnceCell::new(),
    });
    /// Shape transition interning, the equivalent of QuickJS' shape
    /// transition table. Objects built by the same code path converge on one
    /// shared shape, which keeps inline caches monomorphic.
    static TRANSITIONS: RefCell<FxHashMap<(u64, PropKey), Rc<Shape>>> =
        RefCell::new(FxHashMap::default());
}

const TRANSITION_LIMIT: usize = 1 << 16;

thread_local! {
    /// Cached atom for the hot prototype key, so construction and function
    /// initialization do not allocate a fresh property name.
    static PROTOTYPE_KEY: PropKey = PropKey::Str(JsString::from("prototype"));
}

pub fn prototype_key() -> PropKey {
    PROTOTYPE_KEY.with(Clone::clone)
}

pub fn empty_shape() -> Rc<Shape> {
    EMPTY_SHAPE.with(Rc::clone)
}

fn add_transition(parent: &Rc<Shape>, key: PropKey) -> Rc<Shape> {
    TRANSITIONS.with(|map| {
        let mut map = map.borrow_mut();
        if let Some(child) = map.get(&(parent.id, key.clone())) {
            return child.clone();
        }
        if map.len() >= TRANSITION_LIMIT {
            map.clear();
        }
        let mut shape = Shape {
            id: next_shape_id(),
            keys: Vec::with_capacity(parent.keys.len() + 1),
            index: OnceCell::new(),
        };
        shape.keys.extend(parent.keys.iter().cloned());
        shape.grow(key.clone());
        let child = Rc::new(shape);
        map.insert((parent.id, key), child.clone());
        child
    })
}

impl PropCache {
    #[inline]
    pub fn poly_own(&self, shape: u64) -> Option<u32> {
        self.own_shapes[..self.own_count as usize]
            .iter()
            .find(|(cached, _)| *cached == shape)
            .map(|(_, slot)| *slot)
    }

    pub fn remember_own(&mut self, shape: u64, slot: u32) {
        let count = self.own_count as usize;
        for entry in &self.own_shapes[..count] {
            if entry.0 == shape {
                return;
            }
        }
        if count < self.own_shapes.len() {
            self.own_shapes[count] = (shape, slot);
            self.own_count += 1;
        } else {
            self.own_shapes.copy_within(1.., 0);
            self.own_shapes[self.own_shapes.len() - 1] = (shape, slot);
        }
    }

    #[inline]
    pub fn poly_proto(&self, shape: u64) -> bool {
        self.proto_shapes[..self.proto_count as usize].contains(&shape)
    }

    pub fn remember_proto(&mut self, shape: u64) {
        let count = self.proto_count as usize;
        if self.proto_shapes[..count].contains(&shape) {
            return;
        }
        if count < self.proto_shapes.len() {
            self.proto_shapes[count] = shape;
            self.proto_count += 1;
        } else {
            self.proto_shapes.copy_within(1.., 0);
            self.proto_shapes[self.proto_shapes.len() - 1] = shape;
        }
    }
}

impl Shape {
    #[inline(always)]
    pub fn slot(&self, key: &PropKey) -> Option<u32> {
        // Small shapes are cheaper to scan than to index; the hash table is
        // built lazily once the shape grows past the transition-sized case.
        if self.keys.len() <= 8 {
            return self
                .keys
                .iter()
                .position(|candidate| candidate == key)
                .map(|index| index as u32);
        }
        self.index
            .get_or_init(|| {
                let mut index = FxHashMap::with_capacity_and_hasher(
                    self.keys.len(),
                    FxBuildHasher,
                );
                for (slot, key) in self.keys.iter().enumerate() {
                    index.insert(key.clone(), slot as u32);
                }
                index
            })
            .get(key)
            .copied()
    }

    fn grow(&mut self, key: PropKey) -> u32 {
        let slot = self.keys.len() as u32;
        self.keys.push(key.clone());
        if let Some(index) = self.index.get_mut() {
            index.insert(key, slot);
        }
        slot
    }

    fn shrink(&mut self, slot: u32) {
        self.keys.remove(slot as usize);
        if let Some(index) = self.index.get_mut() {
            index.clear();
            for (slot, key) in self.keys.iter().enumerate() {
                index.insert(key.clone(), slot as u32);
            }
        }
    }
}

/// Prepared object-literal layout, shared by every evaluation of the site.
#[derive(Clone)]
pub struct LiteralTemplate {
    pub shape: Rc<Shape>,
    pub props: Vec<Property>,
}

impl LiteralTemplate {
    pub fn new(keys: &[PropKey]) -> Self {
        let mut shape = Shape {
            id: 0,
            keys: Vec::with_capacity(keys.len()),
            index: OnceCell::new(),
        };
        for key in keys {
            shape.grow(key.clone());
        }
        shape.id = next_shape_id();
        Self {
            shape: Rc::new(shape),
            props: keys
                .iter()
                .map(|_| Property::data(JsValue::Undefined))
                .collect(),
        }
    }
}

/// Monomorphic property cache for one constant access site. It mirrors
/// QuickJS' `JSPropCacheEntry`: the receiver shape guards the prototype chain,
/// while the holder depth and shape keep the cached slot verifiable.
#[derive(Clone, Default)]
pub struct PropCache {
    pub object_shape: u64,
    pub holder_shape: u64,
    /// Number of prototype-chain edges from the receiver to the holder.
    pub holder_depth: u32,
    /// Raw address of the receiver's prototype at cache time. The receiver
    /// keeps the prototype alive while the cache entry is used, and
    /// `prototype_generation` guards chain changes.
    pub receiver_proto: Option<usize>,
    pub prototype_generation: u64,
    pub index: u32,
    /// Shape before a new own property was inserted, used to define the same
    /// property on the next object of the same shape.
    pub insert_shape: u64,
    /// Interned shape to install on the next insertion at this site and the
    /// slot the new property takes, i.e. the cached shape transition that
    /// QuickJS keeps in JSShape.transitions.
    pub insert_target: Option<Rc<Shape>>,
    pub insert_index: u32,
    pub can_insert: bool,
    pub own: bool,
    pub valid: bool,
    /// Small polymorphic own-property cache. A shape id fully determines the
    /// slot of this site's key, so entries never need invalidation.
    pub own_shapes: [(u64, u32); 4],
    pub own_count: u8,
    /// Receiver shapes that were verified to reach the cached prototype
    /// holder without an own property.
    pub proto_shapes: [u64; 4],
    pub proto_count: u8,
    /// The canonical key of this site, built once from the bytecode
    /// constant: repeated accesses clone the atom instead of re-parsing and
    /// re-canonicalizing the text (QuickJS interns atoms once the same way).
    pub key: Option<PropKey>,
    /// Prepared property layout for an object-literal site.
    pub literal: Option<LiteralTemplate>,
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
/// area): a shared shape, a slot-aligned property vector, prototype and an
/// optional exotic payload.
pub struct ObjectData {
    pub shape: Rc<Shape>,
    pub props: Vec<Property>,
    pub prototype: Option<ObjRef>,
    pub kind: ObjectKind,
    pub extensible: bool,
    pub class_name: &'static str,
    /// Set when this object appears in another object's prototype chain.
    /// Structural changes on prototypes invalidate monomorphic property
    /// caches that may have observed them.
    pub is_prototype: bool,
}

impl ObjectData {
    #[inline(always)]
    pub fn shape_id(&self) -> u64 {
        self.shape.id
    }

    /// Drops cache validity after a structural change.
    pub(crate) fn touch(&mut self) {
        let mut shape = (*self.shape).clone();
        shape.id = next_shape_id();
        self.shape = Rc::new(shape);
        if self.is_prototype {
            PROTOTYPE_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn ordinary(prototype: Option<ObjRef>) -> Self {
        Self::ordinary_with_capacity(prototype, 0)
    }

    /// An ordinary object with room for the data properties a constructor
    /// or empty literal is expected to add. Receiver sizing avoids
    /// repeatedly growing `props` on `this.x = ...` sequences.
    pub fn ordinary_with_capacity(prototype: Option<ObjRef>, capacity: usize) -> Self {
        Self {
            shape: empty_shape(),
            props: Vec::with_capacity(capacity),
            prototype,
            kind: ObjectKind::Ordinary,
            extensible: true,
            class_name: "Object",
            is_prototype: false,
        }
    }

    pub fn array(prototype: Option<ObjRef>, items: Vec<JsValue>) -> Self {
        let length = items.len();
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::Array(ArrayData {
                items: items.into_iter().map(Some).collect(),
                length,
            }),
            extensible: true,
            class_name: "Array",
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn new_array(items: Vec<JsValue>) -> Self {
        Self::array(None, items)
    }

    pub fn new_function(function: Rc<JsFunction>) -> Self {
        Self {
            // `length` and `name` are defined right after construction; size
            // the map once instead of growing it per insert.
            props: Vec::new(),
            prototype: None,
            kind: ObjectKind::Function(function),
            extensible: true,
            class_name: "Function",
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn boxed_string(value: JsString, prototype: Option<ObjRef>) -> Self {
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::BoxedString(value),
            extensible: true,
            class_name: "String",
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn boxed_number(value: f64, prototype: Option<ObjRef>) -> Self {
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::BoxedNumber(value),
            extensible: true,
            class_name: "Number",
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn boxed_bool(value: bool, prototype: Option<ObjRef>) -> Self {
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::BoxedBool(value),
            extensible: true,
            class_name: "Boolean",
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn map(prototype: Option<ObjRef>, entries: Vec<(JsValue, JsValue)>) -> Self {
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::Map(
                entries
                    .into_iter()
                    .map(|(key, value)| (MapKey(key), value))
                    .collect(),
            ),
            extensible: true,
            class_name: "Map",
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn promise(prototype: Option<ObjRef>) -> Self {
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::Promise(Box::new(PromiseData {
                state: PromiseState::Pending,
                value: JsValue::Undefined,
                reactions: Vec::new(),
                handled: false,
            })),
            extensible: true,
            class_name: "Promise",
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn set(prototype: Option<ObjRef>, values: Vec<JsValue>) -> Self {
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::Set(values.into_iter().map(MapKey).collect()),
            extensible: true,
            class_name: "Set",
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn error(prototype: Option<ObjRef>) -> Self {
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::Error,
            extensible: true,
            class_name: "Error",
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn array_buffer(bytes: ByteBuffer, prototype: Option<ObjRef>) -> Self {
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::ArrayBuffer(ArrayBufferData { bytes }),
            extensible: true,
            class_name: "ArrayBuffer",
            shape: empty_shape(),
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
            props: Vec::new(),
            prototype,
            kind: ObjectKind::DataView(DataViewData {
                buffer,
                bytes,
                byte_offset,
                byte_length,
            }),
            extensible: true,
            class_name: "DataView",
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn typed_array(data: TypedArrayData, prototype: Option<ObjRef>) -> Self {
        let class_name = data.kind.name();
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::TypedArray(data),
            extensible: true,
            class_name,
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn date(value: f64, prototype: Option<ObjRef>) -> Self {
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::Date(value),
            extensible: true,
            class_name: "Date",
            shape: empty_shape(),
            is_prototype: false,
        }
    }

    pub fn regexp(data: Rc<RegExpData>, prototype: Option<ObjRef>) -> Self {
        Self {
            props: Vec::new(),
            prototype,
            kind: ObjectKind::RegExp(data),
            extensible: true,
            class_name: "RegExp",
            shape: empty_shape(),
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
        let key = key.into().canonical();
        if let Some(slot) = self.lookup(&key) {
            self.props[slot as usize] = property;
            return;
        }
        if self.is_prototype {
            PROTOTYPE_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
        self.shape = add_transition(&self.shape, key);
        self.props.push(property);
    }

    /// Defines or updates an accessor in place, without touching the shape
    /// when the key already exists.
    pub fn define_accessor(
        &mut self,
        key: PropKey,
        get: Option<JsValue>,
        set: Option<JsValue>,
        enumerable: bool,
    ) {
        let key = key.canonical();
        if let Some(slot) = self.shape.slot(&key) {
            let property = &mut self.props[slot as usize];
            if get.is_some() {
                property.get = get.map(Box::new);
            }
            if set.is_some() {
                property.set = set.map(Box::new);
            }
            if self.is_prototype {
                PROTOTYPE_GENERATION.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        let mut property = Property::accessor(get, set);
        property.enumerable = enumerable;
        self.define(key, property);
    }

    /// Removes a slot-aligned own property.
    pub fn remove(&mut self, key: &PropKey) -> bool {
        let Some(slot) = self.lookup(key) else {
            return false;
        };
        let mut shape = (*self.shape).clone();
        shape.id = next_shape_id();
        shape.shrink(slot);
        self.shape = Rc::new(shape);
        self.props.remove(slot as usize);
        if self.is_prototype {
            PROTOTYPE_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
        true
    }

    /// Slot lookup with the numeric-string canonical fallback.
    #[inline(always)]
    pub(crate) fn lookup(&self, key: &PropKey) -> Option<u32> {
        if let Some(slot) = self.shape.slot(key) {
            return Some(slot);
        }
        if matches!(key, PropKey::Str(_)) {
            self.shape.slot(&key.canonical())
        } else {
            None
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PropKey, &Property)> {
        self.shape.keys.iter().zip(self.props.iter())
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&PropKey, &mut Property)> {
        self.shape.keys.iter().zip(self.props.iter_mut())
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
    #[inline]
    pub(crate) fn property(&self, key: &PropKey) -> Option<&Property> {
        self.lookup(key).map(|slot| &self.props[slot as usize])
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

impl Finalize for Property {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for Property {
    crate::gc::custom_trace!(this, mark, {
        mark(&this.value);
        if let Some(get) = &this.get {
            mark(&**get);
        }
        if let Some(set) = &this.set {
            mark(&**set);
        }
    });
}

impl Finalize for ObjectData {}
#[allow(unsafe_code)]
unsafe impl crate::gc::Trace for ObjectData {
    crate::gc::custom_trace!(this, mark, {
        for property in &this.props {
            mark(property);
        }
        if let Some(prototype) = &this.prototype {
            mark(prototype);
        }
        mark(&this.kind);
    });
}
