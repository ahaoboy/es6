//! Garbage collection: a trial-deletion cycle collector.
//!
//! QuickJS runs `JS_RunGC` (quickjs.c:7483) as three phases over its GC object
//! list: `gc_decref` (quickjs.c:7373) subtracts every internal reference,
//! `gc_scan` (quickjs.c:7412) restores the counts of objects reachable from
//! externally-referenced survivors, and `gc_free_cycles` (quickjs.c:7432)
//! frees the unreachable remainder. Triggers come from `js_trigger_gc`
//! (quickjs.c:1679) with the `malloc_size + malloc_size/2` threshold policy.
//!
//! `Rc` reference counts cannot be mutated, so the same trial deletion is
//! computed without touching them: phase one counts every entity-to-entity
//! edge to get each entity's *internal in-degree*, phase two derives the
//! external references as `strong_count - registry - in_degree`, and only
//! entities with no external reference that are also unreachable from those
//! roots are swept. Refcount-driven freeing still handles everything else,
//! exactly like QuickJS' async-free design.
//!
//! The registry plays the role of QuickJS' `rt->gc_obj_list`
//! (`JSGCObjectTypeEnum`, quickjs.c:450): every object, environment, native
//! data cell and top-level function bytecode registers itself at creation,
//! keeping one strong reference alive so sweeping is explicit.

use std::cell::RefCell;
use std::rc::Rc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::bytecode::{ClassDef, Constant, FunctionDef};
use crate::object::{
    GeneratorState, JsFunction, NativeData, ObjRef, ObjectData, ObjectKind, PromiseData,
    PromiseReaction,
};
use crate::value::{Env, JsValue};

/// Initial GC threshold, in entities: comparable to QuickJS' 256 KiB
/// `JS_GC_THRESHOLD` (quickjs.c:2333).
const INITIAL_THRESHOLD: usize = 512;

/// One GC-managed allocation.
#[derive(Clone)]
pub(crate) enum Entity {
    Object(ObjRef),
    Env(Rc<Env>),
    Data(NativeData),
    Def(Rc<FunctionDef>),
}

impl Entity {
    fn key(&self) -> EntityKey {
        match self {
            Entity::Object(object) => EntityKey(0, Rc::as_ptr(object) as usize),
            Entity::Env(env) => EntityKey(1, Rc::as_ptr(env) as usize),
            Entity::Data(data) => EntityKey(2, Rc::as_ptr(data) as usize),
            Entity::Def(def) => EntityKey(3, Rc::as_ptr(def) as usize),
        }
    }

    fn strong_count(&self) -> usize {
        match self {
            Entity::Object(object) => Rc::strong_count(object),
            Entity::Env(env) => Rc::strong_count(env),
            Entity::Data(data) => Rc::strong_count(data),
            Entity::Def(def) => Rc::strong_count(def),
        }
    }

    /// Drop every outgoing entity reference so refcount cascades can free
    /// unreachable cycles, the equivalent of QuickJS' finalizers under
    /// `JS_GC_PHASE_REMOVE_CYCLES`. Environment parent chains are acyclic by
    /// construction, so clearing the slots is enough to break cycles.
    fn clear(&mut self) {
        match self {
            Entity::Object(object) => {
                let mut data = object.borrow_mut();
                data.props.clear();
                data.prototype = None;
                data.kind = ObjectKind::Ordinary;
            }
            Entity::Env(env) => {
                env.slots.borrow_mut().clear();
            }
            Entity::Data(data) => data.borrow_mut().clear(),
            // Function bytecode is a tree: dropping the registry reference
            // lets the refcount cascade free it with its constants.
            Entity::Def(_) => {}
        }
    }
}

/// Identity of a GC entity: kind discriminant plus allocation address.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct EntityKey(u8, usize);

struct Registry {
    entities: Vec<Entity>,
    /// Insert-time keys for duplicate suppression.
    keys: FxHashSet<EntityKey>,
    live: usize,
    threshold: usize,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry {
        entities: Vec::new(),
        keys: FxHashSet::default(),
        live: 0,
        threshold: INITIAL_THRESHOLD,
    });
}

/// Allocate an object and register it, the equivalent of QuickJS
/// `JS_NewObjectFromShape` (quickjs.c:6136) installing every object in
/// `rt->gc_obj_list`.
pub(crate) fn allocate_object(data: ObjectData) -> ObjRef {
    let object = Rc::new(RefCell::new(data));
    register(Entity::Object(object.clone()));
    object
}

/// Register an environment, like QuickJS' `JS_GC_OBJ_TYPE_VAR_REF` tracking
/// (quickjs.c:450).
pub(crate) fn register_env(env: Rc<Env>) -> Rc<Env> {
    register(Entity::Env(env.clone()));
    env
}

/// Register a shared native data cell (quickjs `js_c_function_data`,
/// quickjs.c:6595) whose contents `mark_children` walks (quickjs.c:7255).
pub(crate) fn register_data(data: NativeData) -> NativeData {
    register(Entity::Data(data.clone()));
    data
}

/// Register top-level function bytecode, the equivalent of QuickJS'
/// `JS_GC_OBJ_TYPE_FUNCTION_BYTECODE` (quickjs.c:450).
pub(crate) fn register_def(def: Rc<FunctionDef>) -> Rc<FunctionDef> {
    register(Entity::Def(def.clone()));
    def
}

fn register(entity: Entity) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.keys.insert(entity.key()) {
            registry.entities.push(entity);
        }
    });
}

/// Run a collection if the allocation count passed the threshold, like
/// `js_trigger_gc` (quickjs.c:1679). Only called at safe points where no VM
/// frame is on the Rust call stack.
pub(crate) fn trigger_gc() {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.entities.len() >= registry.threshold {
            run_gc(&mut registry);
        }
    });
}

/// `JS_RunGC` (quickjs.c:7483): `gc_decref` → `gc_scan` → `gc_free_cycles`,
/// computed through explicit internal-in-degree counting since `Rc`
/// reference counts are immutable.
fn run_gc(registry: &mut Registry) {
    let count = registry.entities.len();
    let mut index_of: FxHashMap<EntityKey, usize> =
        FxHashMap::with_capacity_and_hasher(count, Default::default());
    for (index, entity) in registry.entities.iter().enumerate() {
        index_of.insert(entity.key(), index);
    }

    // gc_scan state: index → marked.
    let mut marked = vec![false; count];

    // Phase 1 (gc_decref, quickjs.c:7373): count every entity-to-entity
    // edge, giving each entity's internal in-degree. The subtracted
    // equivalent of the trial deletion: `strong - registry - in_degree`
    // is exactly the external reference count.
    let mut in_degree: FxHashMap<EntityKey, u32> =
        FxHashMap::with_capacity_and_hasher(count, Default::default());
    for entity in &registry.entities {
        visit_children(entity, &mut |child| {
            if index_of.contains_key(&child.key()) {
                *in_degree.entry(child.key()).or_insert(0) += 1;
            }
        });
    }

    // Phase 2 (gc_scan, quickjs.c:7412): mark everything reachable from
    // entities with external references (quickjs' re-increment pass).
    let mut queue: Vec<usize> = Vec::new();
    for (index, entity) in registry.entities.iter().enumerate() {
        let key = entity.key();
        let internal = in_degree.get(&key).copied().unwrap_or(0) as usize;
        let external = entity.strong_count().saturating_sub(1 + internal);
        if external > 0 {
            marked[index] = true;
            queue.push(index);
        }
    }
    let mut pending: Vec<Entity> = Vec::new();
    while let Some(index) = queue.pop() {
        pending.clear();
        visit_children(&registry.entities[index], &mut |child| pending.push(child.clone()));
        for child in pending.drain(..) {
            if let Some(&child_index) = index_of.get(&child.key())
                && !marked[child_index]
            {
                marked[child_index] = true;
                queue.push(child_index);
            }
        }
    }

    // Phase 3 (gc_free_cycles, quickjs.c:7432): clear the unreachable
    // entities' outgoing references so the refcount cascade frees the
    // cycles, then drop the registry's own references.
    let mut garbage: Vec<Entity> = Vec::new();
    let mut survivors: Vec<Entity> = Vec::with_capacity(count);
    let mut survivor_keys: FxHashSet<EntityKey> =
        FxHashSet::with_capacity_and_hasher(count, Default::default());
    for (index, entity) in std::mem::take(&mut registry.entities).into_iter().enumerate() {
        if marked[index] {
            survivor_keys.insert(entity.key());
            survivors.push(entity);
        } else {
            garbage.push(entity);
        }
    }
    for mut entity in garbage {
        entity.clear();
    }
    registry.entities = survivors;
    registry.keys = survivor_keys;

    // QuickJS resets the threshold to `malloc_size + malloc_size/2`
    // (quickjs.c:1695); the entity-count equivalent keeps the same growth
    // policy.
    registry.live = registry.entities.len();
    registry.threshold = (registry.live + registry.live / 2).max(INITIAL_THRESHOLD);
}

/// `mark_children` (quickjs.c:7255) over the Rust object model. Every field
/// that can hold a strong entity reference must be visited here; the
/// exhaustive match keeps new `ObjectKind` payloads covered.
fn visit_children(entity: &Entity, visit: &mut dyn FnMut(&Entity)) {
    fn value(value: &JsValue, visit: &mut dyn FnMut(&Entity)) {
        if let JsValue::Object(object) = value {
            visit(&Entity::Object(object.clone()));
        }
    }
    fn visit_function(function: &JsFunction, visit: &mut dyn FnMut(&Entity)) {
        if let Some(def) = &function.def {
            visit(&Entity::Def(def.clone()));
        }
        if let Some(env) = &function.env {
            visit(&Entity::Env(env.clone()));
        }
        if let Some(bound) = &function.bound {
            value(&bound.target, visit);
            value(&bound.this, visit);
            for argument in &bound.arguments {
                value(argument, visit);
            }
        }
        if let Some(this) = &function.lexical_this {
            value(this, visit);
        }
        if let Some(init) = &function.instance_init {
            visit(&Entity::Def(init.clone()));
        }
        // The native data cell shared with `native_data` handlers.
        visit(&Entity::Data(function.data.clone()));
    }
    fn visit_reaction(reaction: &PromiseReaction, visit: &mut dyn FnMut(&Entity)) {
        if let Some(on_fulfilled) = &reaction.on_fulfilled {
            value(on_fulfilled, visit);
        }
        if let Some(on_rejected) = &reaction.on_rejected {
            value(on_rejected, visit);
        }
        if let Some(promise) = &reaction.promise {
            visit(&Entity::Object(promise.clone()));
        }
    }
    fn visit_promise(promise: &PromiseData, visit: &mut dyn FnMut(&Entity)) {
        value(&promise.value, visit);
        for reaction in &promise.reactions {
            visit_reaction(reaction, visit);
        }
    }
    fn visit_class(class: &Rc<ClassDef>, visit: &mut dyn FnMut(&Entity)) {
        if let Some(constructor) = &class.constructor {
            visit(&Entity::Def(constructor.clone()));
        }
        for method in &class.methods {
            visit(&Entity::Def(method.function.clone()));
        }
        if let Some(init) = &class.instance_init {
            visit(&Entity::Def(init.clone()));
        }
        if let Some(init) = &class.static_init {
            visit(&Entity::Def(init.clone()));
        }
    }
    fn visit_frame(frame: &crate::vm::Frame, visit: &mut dyn FnMut(&Entity)) {
        visit(&Entity::Def(frame.definition.clone()));
        if let Some(env) = &frame.env {
            visit(&Entity::Env(env.clone()));
        }
        value(&frame.this, visit);
        if let Some(callee) = &frame.callee {
            value(callee, visit);
        }
        for item in frame.stack.iter() {
            value(item, visit);
        }
        for item in frame.arguments.iter() {
            value(item, visit);
        }
        for item in frame.locals.iter() {
            value(item, visit);
        }
        for handler in &frame.handlers {
            if let Some(env) = &handler.env {
                visit(&Entity::Env(env.clone()));
            }
        }
        if let Some(completion) = &frame.completion {
            visit(&Entity::Object(completion.clone()));
        }
    }
    match entity {
        Entity::Object(object) => {
            let data = object.borrow();
            for property in data.props.values() {
                value(&property.value, visit);
                if let Some(get) = &property.get {
                    value(get, visit);
                }
                if let Some(set) = &property.set {
                    value(set, visit);
                }
            }
            if let Some(prototype) = &data.prototype {
                visit(&Entity::Object(prototype.clone()));
            }
            match &data.kind {
                ObjectKind::Ordinary => {}
                ObjectKind::Array(array) => {
                    for item in array.items.iter().flatten() {
                        value(item, visit);
                    }
                }
                ObjectKind::Function(inner) => visit_function(inner, visit),
                ObjectKind::BoxedString(_)
                | ObjectKind::BoxedNumber(_)
                | ObjectKind::BoxedBool(_) => {}
                ObjectKind::Date(_) | ObjectKind::RegExp(_) | ObjectKind::Error => {}
                ObjectKind::Iterator { items, .. } => {
                    for item in items {
                        value(item, visit);
                    }
                }
                ObjectKind::Map(entries) => {
                    for (key, value_item) in entries {
                        value(key, visit);
                        value(value_item, visit);
                }
                }
                ObjectKind::Set(values) => {
                    for item in values {
                        value(item, visit);
                    }
                }
                // Weak collections: only entries whose key is still alive
                // keep their values reachable, so dead-key entries cannot
                // retain garbage.
                ObjectKind::WeakMap(entries) => {
                    entries.retain(|(key, _)| key.upgrade().is_some());
                    for (_, entry_value) in entries {
                        value(entry_value, visit);
                    }
                }
                ObjectKind::WeakSet(values) => {
                    values.retain(|key| key.upgrade().is_some());
                }
                ObjectKind::Promise(inner) => visit_promise(inner, visit),
                ObjectKind::ArrayBuffer(_) => {}
                ObjectKind::DataView(view) => {
                    visit(&Entity::Object(view.buffer.clone()));
                }
                ObjectKind::TypedArray(typed) => {
                    visit(&Entity::Object(typed.buffer.clone()));
                }
                ObjectKind::AsyncFrame(frame) => {
                    if let Some(frame) = frame {
                        visit_frame(frame, visit);
                    }
                }
                ObjectKind::ArrayIterator { array, .. } => {
                    visit(&Entity::Object(array.clone()));
                }
                ObjectKind::StringIterator { .. } => {}
                ObjectKind::Generator(generator) => {
                    visit_function(&generator.function, visit);
                    value(&generator.this, visit);
                    for argument in &generator.arguments {
                        value(argument, visit);
                    }
                    if let GeneratorState::SuspendedYield(frame) = &generator.state {
                        visit_frame(frame, visit);
                    }
                }
            }
        }
        Entity::Env(env) => {
            for slot in env.slots.borrow().iter() {
                value(slot, visit);
            }
            if let Some(parent) = &env.parent {
                visit(&Entity::Env(parent.clone()));
            }
        }
        Entity::Data(data) => {
            for item in data.borrow().iter() {
                value(item, visit);
            }
        }
        // Function bytecode holds only its constant pool: nested defs,
        // classes and plain data (quickjs.c:855 `cpool`).
        Entity::Def(def) => {
            for constant in &def.constants {
                match constant {
                    Constant::Function(nested) => visit(&Entity::Def(nested.clone())),
                    Constant::Class(class_def) => visit_class(class_def, visit),
                    _ => {}
                }
            }
        }
    }
}

/// Public entry: run a full collection, like `JS_RunGC` exposure through
/// `JS_ComputeMemoryUsage`-adjacent APIs.
pub fn run_gc_now() {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        run_gc(&mut registry);
    });
}

/// Live entity count, for diagnostics.
pub fn entity_count() -> usize {
    REGISTRY.with(|registry| registry.borrow().entities.len())
}
