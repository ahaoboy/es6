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
use std::rc::{Rc, Weak};

use rustc_hash::FxHashMap;

use crate::bytecode::{ClassDef, Constant, FunctionDef};
use crate::object::{
    GeneratorState, JsFunction, NativeData, ObjRef, ObjectData, ObjectKind, PromiseData,
    PromiseReaction,
};
use crate::value::{Env, JsValue};

/// Initial GC threshold, in entities: comparable to QuickJS' 256 KiB
/// `JS_GC_THRESHOLD` (quickjs.c:2333).
const INITIAL_THRESHOLD: usize = 4096;

/// One GC-managed allocation.
///
/// The registry keeps only a weak reference, like QuickJS' intrusive
/// `gc_obj_list` which does not own its entries: acyclic garbage is freed by
/// its last `Rc` immediately, and only cycles survive to the trial-deletion
/// scan.
#[derive(Clone)]
pub(crate) enum Entity {
    Object(Weak<RefCell<ObjectData>>),
    Env(Weak<Env>),
    Data(Weak<RefCell<Vec<JsValue>>>),
    Def(Weak<FunctionDef>),
}

impl Entity {
    fn key(&self) -> EntityKey {
        match self {
            Entity::Object(object) => EntityKey(0, Weak::as_ptr(object) as usize),
            Entity::Env(env) => EntityKey(1, Weak::as_ptr(env) as usize),
            Entity::Data(data) => EntityKey(2, Weak::as_ptr(data) as usize),
            Entity::Def(def) => EntityKey(3, Weak::as_ptr(def) as usize),
        }
    }

    fn is_live(&self) -> bool {
        self.strong_count() > 0
    }

    fn strong_count(&self) -> usize {
        match self {
            Entity::Object(object) => object.strong_count(),
            Entity::Env(env) => env.strong_count(),
            Entity::Data(data) => data.strong_count(),
            Entity::Def(def) => def.strong_count(),
        }
    }

    /// Drop every outgoing entity reference so refcount cascades can free
    /// unreachable cycles, the equivalent of QuickJS' finalizers under
    /// `JS_GC_PHASE_REMOVE_CYCLES`. Environment parent chains are acyclic by
    /// construction, so clearing the slots is enough to break cycles.
    fn clear(&mut self) {
        match self {
            Entity::Object(object) => {
                if let Some(object) = object.upgrade() {
                    let mut data = object.borrow_mut();
                    data.props.clear();
                    data.prototype = None;
                    data.kind = ObjectKind::Ordinary;
                }
            }
            Entity::Env(env) => {
                if let Some(env) = env.upgrade() {
                    env.slots.borrow_mut().clear();
                }
            }
            Entity::Data(data) => {
                if let Some(data) = data.upgrade() {
                    data.borrow_mut().clear();
                }
            }
            // Function bytecode is a tree: dropping the last external
            // reference frees it with its constants.
            Entity::Def(_) => {}
        }
    }
}

/// Identity of a GC entity: kind discriminant plus allocation address.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct EntityKey(u8, usize);

struct Registry {
    entities: Vec<Entity>,
    live: usize,
    threshold: usize,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = const {
        RefCell::new(Registry {
            entities: Vec::new(),
            live: 0,
            threshold: INITIAL_THRESHOLD,
        })
    };
}

/// Allocate an object and register it, the equivalent of QuickJS
/// `JS_NewObjectFromShape` (quickjs.c:6136) installing every object in
/// `rt->gc_obj_list`.
pub(crate) fn allocate_object(data: ObjectData) -> ObjRef {
    let object = Rc::new(RefCell::new(data));
    register(Entity::Object(Rc::downgrade(&object)));
    object
}

/// Register an environment, like QuickJS' `JS_GC_OBJ_TYPE_VAR_REF` tracking
/// (quickjs.c:450).
pub(crate) fn register_env(env: Rc<Env>) -> Rc<Env> {
    register(Entity::Env(Rc::downgrade(&env)));
    env
}

/// Register a shared native data cell (quickjs `js_c_function_data`,
/// quickjs.c:6595) whose contents `mark_children` walks (quickjs.c:7255).
pub(crate) fn register_data(data: NativeData) -> NativeData {
    register(Entity::Data(Rc::downgrade(&data)));
    data
}

/// Register top-level function bytecode, the equivalent of QuickJS'
/// `JS_GC_OBJ_TYPE_FUNCTION_BYTECODE` (quickjs.c:450).
pub(crate) fn register_def(def: Rc<FunctionDef>) -> Rc<FunctionDef> {
    register(Entity::Def(Rc::downgrade(&def)));
    def
}

fn register(entity: Entity) {
    // Every entity is registered exactly once, at creation; suppressing
    // duplicates with a hash set cost one hash lookup per allocation. The
    // registry is the `rt->gc_obj_list` equivalent, a plain append.
    REGISTRY.with(|registry| registry.borrow_mut().entities.push(entity));
}

/// Allocation-boundary collection used by the interpreter loop. Safe because
/// the trial-deletion scan treats references held by Rust locals (the value
/// stack, frame slots, temporaries) as external roots.
pub(crate) fn maybe_collect() {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.entities.len() < registry.threshold {
            return;
        }
        // Dead weak handles are acyclic garbage that refcounting already
        // freed; dropping the handles is much cheaper than a cycle scan.
        registry.entities.retain(Entity::is_live);
        if registry.entities.len() >= registry.threshold {
            run_gc(&mut registry);
        } else {
            registry.live = registry.entities.len();
            registry.threshold = (registry.live * 2).max(INITIAL_THRESHOLD);
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
    // Entries whose last strong reference is gone cannot be part of a cycle;
    // drop their weak handles before scanning.
    registry.entities.retain(Entity::is_live);
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
        // The registry holds only a weak handle, so every strong count above
        // the internal in-degree is an external root.
        let external = entity.strong_count().saturating_sub(internal);
        if external > 0 {
            marked[index] = true;
            queue.push(index);
        }
    }
    let mut pending: Vec<Entity> = Vec::new();
    while let Some(index) = queue.pop() {
        pending.clear();
        visit_children(&registry.entities[index], &mut |child| {
            pending.push(child.clone())
        });
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
    for (index, entity) in std::mem::take(&mut registry.entities)
        .into_iter()
        .enumerate()
    {
        if marked[index] {
            survivors.push(entity);
        } else {
            garbage.push(entity);
        }
    }
    for mut entity in garbage {
        entity.clear();
    }
    registry.entities = survivors;

    // Prune weak-collection entries whose keys died in this sweep, so their
    // values become unreachable on the next cycle.
    for entity in &registry.entities {
        if let Entity::Object(object) = entity
            && let Some(object) = object.upgrade()
        {
            let mut data = object.borrow_mut();
            match &mut data.kind {
                ObjectKind::WeakMap(entries) => {
                    entries.retain(|(key, _)| key.upgrade().is_some());
                }
                ObjectKind::WeakSet(values) => {
                    values.retain(|key| key.upgrade().is_some());
                }
                _ => {}
            }
        }
    }

    // QuickJS resets the threshold to `malloc_size + malloc_size/2`
    // (quickjs.c:1695); the entity-count equivalent keeps the same growth
    // policy.
    registry.live = registry.entities.len();
    // Grow to twice the live heap before the next cycle scan: weak handles
    // mean acyclic garbage no longer accumulates in the registry, so scans
    // only need to find cycles.
    registry.threshold = (registry.live * 2).max(INITIAL_THRESHOLD);
}

/// `mark_children` (quickjs.c:7255) over the Rust object model. Every field
/// that can hold a strong entity reference must be visited here; the
/// exhaustive match keeps new `ObjectKind` payloads covered.
fn visit_children(entity: &Entity, visit: &mut dyn FnMut(&Entity)) {
    fn value(value: &JsValue, visit: &mut dyn FnMut(&Entity)) {
        if let JsValue::Object(object) = value {
            visit(&Entity::Object(Rc::downgrade(object)));
        }
    }
    fn visit_function(function: &JsFunction, visit: &mut dyn FnMut(&Entity)) {
        if let Some(def) = &function.def {
            visit(&Entity::Def(Rc::downgrade(def)));
        }
        if let Some(env) = &function.env {
            visit(&Entity::Env(Rc::downgrade(env)));
        }
        if let Some(bound) = &function.bound {
            value(&bound.target, visit);
            value(&bound.this, visit);
            for argument in &bound.arguments {
                value(argument, visit);
            }
        }
        if let Some(prototype) = &*function.prototype.borrow() {
            visit(&Entity::Object(Rc::downgrade(prototype)));
        }
        if let Some(this) = &function.lexical_this {
            value(this, visit);
        }
        if let Some(init) = &function.instance_init {
            visit(&Entity::Def(Rc::downgrade(init)));
        }
        // The native data cell shared with `native_data` handlers.
        visit(&Entity::Data(Rc::downgrade(&function.data)));
    }
    fn visit_reaction(reaction: &PromiseReaction, visit: &mut dyn FnMut(&Entity)) {
        if let Some(on_fulfilled) = &reaction.on_fulfilled {
            value(on_fulfilled, visit);
        }
        if let Some(on_rejected) = &reaction.on_rejected {
            value(on_rejected, visit);
        }
        if let Some(promise) = &reaction.promise {
            visit(&Entity::Object(Rc::downgrade(promise)));
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
            visit(&Entity::Def(Rc::downgrade(constructor)));
        }
        for method in &class.methods {
            visit(&Entity::Def(Rc::downgrade(&method.function)));
        }
        if let Some(init) = &class.instance_init {
            visit(&Entity::Def(Rc::downgrade(init)));
        }
        if let Some(init) = &class.static_init {
            visit(&Entity::Def(Rc::downgrade(init)));
        }
    }
    fn visit_frame(frame: &crate::vm::Frame, visit: &mut dyn FnMut(&Entity)) {
        visit(&Entity::Def(Rc::downgrade(&frame.definition)));
        if let Some(env) = &frame.env {
            visit(&Entity::Env(Rc::downgrade(env)));
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
                visit(&Entity::Env(Rc::downgrade(env)));
            }
        }
        if let Some(completion) = &frame.completion {
            visit(&Entity::Object(Rc::downgrade(completion)));
        }
    }
    match entity {
        Entity::Object(object) => {
            let Some(object) = object.upgrade() else {
                return;
            };
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
                visit(&Entity::Object(Rc::downgrade(prototype)));
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
                        value(&key.0, visit);
                        value(value_item, visit);
                    }
                }
                ObjectKind::Set(values) => {
                    for item in values {
                        value(&item.0, visit);
                    }
                }
                // Weak collections: only entries whose key is still alive
                // keep their values reachable, so dead-key entries cannot
                // retain garbage. Pruning happens after the sweep.
                ObjectKind::WeakMap(entries) => {
                    for (key, entry_value) in entries {
                        if key.upgrade().is_some() {
                            value(entry_value, visit);
                        }
                    }
                }
                ObjectKind::WeakSet(_) => {}
                ObjectKind::Promise(inner) => visit_promise(inner, visit),
                ObjectKind::ArrayBuffer(_) => {}
                ObjectKind::DataView(view) => {
                    visit(&Entity::Object(Rc::downgrade(&view.buffer)));
                }
                ObjectKind::TypedArray(typed) => {
                    visit(&Entity::Object(Rc::downgrade(&typed.buffer)));
                }
                ObjectKind::AsyncFrame(frame) => {
                    if let Some(frame) = frame {
                        visit_frame(frame, visit);
                    }
                }
                ObjectKind::ArrayIterator { array, .. } => {
                    visit(&Entity::Object(Rc::downgrade(array)));
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
            let Some(env) = env.upgrade() else {
                return;
            };
            for slot in env.slots.borrow().iter() {
                value(slot, visit);
            }
            if let Some(parent) = &env.parent {
                visit(&Entity::Env(Rc::downgrade(parent)));
            }
        }
        Entity::Data(data) => {
            let Some(data) = data.upgrade() else {
                return;
            };
            for item in data.borrow().iter() {
                value(item, visit);
            }
        }
        // Function bytecode holds only its constant pool: nested defs,
        // classes and plain data (quickjs.c:855 `cpool`).
        Entity::Def(def) => {
            let Some(def) = def.upgrade() else {
                return;
            };
            for constant in &def.constants {
                match constant {
                    Constant::Function(nested) => visit(&Entity::Def(Rc::downgrade(nested))),
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
