use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::rc::Rc;

use crate::builtins;
use crate::bytecode::FunctionDef;
use crate::compiler;
use crate::object::{Function, JsFunction, ObjRef, Object, ObjectData, ObjectKind};
use crate::parser;
use crate::value::{Env, EnvRef, JsResult, JsValue};

pub type Result<T> = std::result::Result<T, Error>;

/// Native-stack guard for recursive JS calls, like QuickJS'
/// `JS_DEFAULT_STACK_SIZE` bound. Hosts that run the engine on a small thread
/// stack can lower it; the CLI raises the stack and keeps this generous.
pub const DEFAULT_MAX_CALL_DEPTH: usize = 8000;

#[derive(Clone)]
pub struct Error {
    message: String,
    value: JsValue,
    offset: Option<usize>,
    line: Option<usize>,
    column: Option<usize>,
}

impl Error {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            value: JsValue::Undefined,
            offset: None,
            line: None,
            column: None,
        }
    }

    pub fn from_value(value: JsValue, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            value,
            offset: None,
            line: None,
            column: None,
        }
    }

    pub fn with_position(mut self, offset: usize, line: usize, column: usize) -> Self {
        self.offset = Some(offset);
        self.line = (line > 0).then_some(line);
        self.column = (column > 0).then_some(column);
        self
    }

    pub fn syntax(message: impl Into<String>, offset: usize, line: usize, column: usize) -> Self {
        Self {
            message: message.into(),
            value: JsValue::Undefined,
            offset: Some(offset),
            line: (line > 0).then_some(line),
            column: (column > 0).then_some(column),
        }
    }

    pub fn value(&self) -> &JsValue {
        &self.value
    }

    pub fn offset(&self) -> Option<usize> {
        self.offset
    }

    pub fn line(&self) -> Option<usize> {
        self.line
    }

    pub fn column(&self) -> Option<usize> {
        self.column
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

impl From<JsValue> for Error {
    fn from(value: JsValue) -> Self {
        let message = match &value {
            JsValue::String(text) => text.to_string(),
            JsValue::Object(object) => {
                let object = object.borrow();
                if let Some(value) = object.get_own_value(&"message".into()) {
                    match value {
                        JsValue::String(text) => text.to_string(),
                        _ => "JavaScript exception".into(),
                    }
                } else {
                    "JavaScript exception".into()
                }
            }
            _ => "JavaScript exception".into(),
        };
        Self {
            message,
            value,
            offset: None,
            line: None,
            column: None,
        }
    }
}

/// Global realm state. QuickJS separates `JSRuntime` (GC/atoms) from
/// `JSContext` (global object/prototypes); Rust ownership makes one `Realm`
/// sufficient. Initialization mirrors `JS_NewContextRaw` (quickjs.c:2846).
pub struct Realm {
    pub global: ObjRef,
    pub object_proto: ObjRef,
    pub function_proto: ObjRef,
    pub array_proto: ObjRef,
    pub string_proto: ObjRef,
    pub number_proto: ObjRef,
    pub boolean_proto: ObjRef,
    pub error_proto: ObjRef,
    pub type_error_proto: ObjRef,
    pub range_error_proto: ObjRef,
    pub reference_error_proto: ObjRef,
    pub syntax_error_proto: ObjRef,
    pub eval_error_proto: ObjRef,
    pub uri_error_proto: ObjRef,
    pub regexp_proto: ObjRef,
    pub date_proto: ObjRef,
    pub map_proto: ObjRef,
    pub set_proto: ObjRef,
    pub symbol_proto: ObjRef,
    pub promise_proto: ObjRef,
    pub array_buffer_proto: ObjRef,
    pub data_view_proto: ObjRef,
    /// `%GeneratorPrototype%` (ES2022 27.5.2).
    pub generator_proto: ObjRef,
    pub weak_map_proto: ObjRef,
    pub weak_set_proto: ObjRef,
    pub typed_array_protos: std::collections::HashMap<crate::object::TypedArrayKind, ObjRef>,
    pub symbol_counter: u32,
    pub symbols: Vec<crate::string::JsString>,
    pub symbol_registry: std::collections::HashMap<String, u32>,
    pub well_known_symbols: std::collections::HashMap<&'static str, u32>,
    /// FIFO of promise/await jobs. QuickJS keeps the same list in
    /// `JSContext.job_list` and drains it with `JS_ExecutePendingJob`
    /// (quickjs.c:2525).
    pub(crate) jobs: VecDeque<crate::vm::Job>,
    /// Recycled activation records, analogous to reusing `JSStackFrame`
    /// storage on QuickJS' value stack.
    #[allow(clippy::vec_box)]
    pub(crate) frames: Vec<Box<crate::vm::Frame>>,
    #[cfg(feature = "timers")]
    pub(crate) timers: Vec<crate::modules::timers::TimerEntry>,
    #[cfg(feature = "timers")]
    pub(crate) next_timer_id: u64,
    #[cfg(feature = "timers")]
    pub(crate) cleared_timer_ids: std::collections::HashSet<u64>,
    /// Lazily created host modules returned by `require()`.
    pub(crate) module_cache: std::collections::HashMap<String, JsValue>,
    /// Number of bytecode calls currently on the native stack. QuickJS keeps
    /// the equivalent bound in `rt->stack_limit`.
    pub(crate) call_depth: usize,
    pub(crate) max_call_depth: usize,
}

impl Realm {
    fn new() -> Self {
        let object_proto = allocate(ObjectData::ordinary(None));
        let function_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let array_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let string_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let number_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let boolean_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let error_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let type_error_proto = allocate(ObjectData::ordinary(Some(error_proto.clone())));
        let range_error_proto = allocate(ObjectData::ordinary(Some(error_proto.clone())));
        let reference_error_proto = allocate(ObjectData::ordinary(Some(error_proto.clone())));
        let syntax_error_proto = allocate(ObjectData::ordinary(Some(error_proto.clone())));
        let eval_error_proto = allocate(ObjectData::ordinary(Some(error_proto.clone())));
        let uri_error_proto = allocate(ObjectData::ordinary(Some(error_proto.clone())));
        let regexp_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let date_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let map_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let set_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let symbol_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let promise_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let array_buffer_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let data_view_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let generator_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let weak_map_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let weak_set_proto = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let typed_array_protos = [
            crate::object::TypedArrayKind::Uint8,
            crate::object::TypedArrayKind::Uint8Clamped,
            crate::object::TypedArrayKind::Int8,
            crate::object::TypedArrayKind::Uint16,
            crate::object::TypedArrayKind::Int16,
            crate::object::TypedArrayKind::Uint32,
            crate::object::TypedArrayKind::Int32,
            crate::object::TypedArrayKind::Float32,
            crate::object::TypedArrayKind::Float64,
        ]
        .into_iter()
        .map(|kind| {
            (
                kind,
                allocate(ObjectData::ordinary(Some(array_proto.clone()))),
            )
        })
        .collect();
        let global = allocate(ObjectData::ordinary(Some(object_proto.clone())));
        let mut realm = Self {
            global,
            object_proto,
            function_proto,
            array_proto,
            string_proto,
            number_proto,
            boolean_proto,
            error_proto,
            type_error_proto,
            range_error_proto,
            reference_error_proto,
            syntax_error_proto,
            eval_error_proto,
            uri_error_proto,
            regexp_proto,
            date_proto,
            map_proto,
            set_proto,
            symbol_proto,
            promise_proto,
            array_buffer_proto,
            data_view_proto,
            generator_proto,
            weak_map_proto,
            weak_set_proto,
            typed_array_protos,
            symbol_counter: 1,
            symbols: Vec::new(),
            symbol_registry: std::collections::HashMap::new(),
            well_known_symbols: std::collections::HashMap::new(),
            jobs: VecDeque::new(),
            frames: Vec::new(),
            #[cfg(feature = "timers")]
            timers: Vec::new(),
            #[cfg(feature = "timers")]
            next_timer_id: 1,
            #[cfg(feature = "timers")]
            cleared_timer_ids: std::collections::HashSet::new(),
            module_cache: std::collections::HashMap::new(),
            call_depth: 0,
            max_call_depth: DEFAULT_MAX_CALL_DEPTH,
        };
        builtins::install(&mut realm);
        realm
    }

    pub fn object(&self) -> ObjRef {
        allocate(ObjectData::ordinary(Some(self.object_proto.clone())))
    }

    pub fn array(&self, items: Vec<JsValue>) -> ObjRef {
        allocate(ObjectData::array(Some(self.array_proto.clone()), items))
    }

    pub fn boxed_string(&self, value: crate::string::JsString) -> ObjRef {
        allocate(ObjectData::boxed_string(
            value,
            Some(self.string_proto.clone()),
        ))
    }

    pub fn boxed_number(&self, value: f64) -> ObjRef {
        allocate(ObjectData::boxed_number(
            value,
            Some(self.number_proto.clone()),
        ))
    }

    pub fn boxed_bool(&self, value: bool) -> ObjRef {
        allocate(ObjectData::boxed_bool(
            value,
            Some(self.boolean_proto.clone()),
        ))
    }

    pub fn map(&self, entries: Vec<(JsValue, JsValue)>) -> ObjRef {
        allocate(ObjectData::map(Some(self.map_proto.clone()), entries))
    }

    pub fn set(&self, values: Vec<JsValue>) -> ObjRef {
        allocate(ObjectData::set(Some(self.set_proto.clone()), values))
    }

    pub fn promise(&self) -> ObjRef {
        allocate(ObjectData::promise(Some(self.promise_proto.clone())))
    }

    /// Wrap a suspended `async` VM frame in an object, mirroring the
    /// QuickJS `JSAsyncFunction` GC object that owns the suspended stack
    /// (`JSStackFrame.cur_gc_obj`, quickjs.c:442).
    #[allow(private_interfaces)]
    pub fn async_frame(&self, frame: Box<crate::vm::Frame>) -> ObjRef {
        let mut data = ObjectData::ordinary(None);
        data.kind = ObjectKind::AsyncFrame(Some(frame));
        data.class_name = "AsyncFunction";
        allocate(data)
    }

    pub fn array_buffer(&self, length: usize) -> ObjRef {
        allocate(ObjectData::array_buffer(
            Rc::new(RefCell::new(vec![0; length])),
            Some(self.array_buffer_proto.clone()),
        ))
    }

    pub fn array_buffer_from(&self, bytes: Vec<u8>) -> ObjRef {
        allocate(ObjectData::array_buffer(
            Rc::new(RefCell::new(bytes)),
            Some(self.array_buffer_proto.clone()),
        ))
    }

    pub fn data_view(
        &self,
        buffer: ObjRef,
        bytes: crate::object::ByteBuffer,
        offset: usize,
        length: usize,
    ) -> ObjRef {
        allocate(ObjectData::data_view(
            buffer,
            bytes,
            offset,
            length,
            Some(self.data_view_proto.clone()),
        ))
    }

    pub fn typed_array(&self, data: crate::object::TypedArrayData) -> ObjRef {
        let prototype = self
            .typed_array_protos
            .get(&data.kind)
            .cloned()
            .unwrap_or_else(|| self.object_proto.clone());
        allocate(ObjectData::typed_array(data, Some(prototype)))
    }

    pub fn iterator(&self, items: Vec<JsValue>) -> ObjRef {
        let iterator = allocate(ObjectData {
            props: Default::default(),
            prototype: Some(self.object_proto.clone()),
            kind: ObjectKind::Iterator { items, index: 0 },
            extensible: true,
            class_name: "Iterator",
            shape: 0,
            is_prototype: false,
        });
        let next = self.native_function("next", 0, iterator_next);
        iterator
            .borrow_mut()
            .define("next", crate::object::Property::hidden(next));
        iterator
    }

    /// Live array iterator (ES2022 23.1.5.1), like QuickJS
    /// `JS_CLASS_ARRAY_ITERATOR`: the elements are read from the array on
    /// every step.
    pub fn array_iterator(&self, array: ObjRef, entries: bool) -> ObjRef {
        let mut data = ObjectData::ordinary(Some(self.object_proto.clone()));
        data.kind = ObjectKind::ArrayIterator {
            array,
            index: 0,
            entries,
        };
        data.class_name = "Array Iterator";
        let iterator = allocate(data);
        let next = self.native_function("next", 0, iterator_next);
        iterator
            .borrow_mut()
            .define("next", crate::object::Property::hidden(next));
        iterator
    }

    /// Lazy string iterator over UTF-16 code units (ES2022 23.1.5.2.1).
    pub fn string_iterator(&self, text: crate::string::JsString) -> ObjRef {
        let mut data = ObjectData::ordinary(Some(self.object_proto.clone()));
        data.kind = ObjectKind::StringIterator { text, index: 0 };
        data.class_name = "String Iterator";
        let iterator = allocate(data);
        let next = self.native_function("next", 0, iterator_next);
        iterator
            .borrow_mut()
            .define("next", crate::object::Property::hidden(next));
        iterator
    }

    /// Create a generator object (ES2022 27.5.1): the function plus its
    /// creation-time receiver and arguments, suspended at the start, like
    /// QuickJS `JS_CLASS_GENERATOR`.
    pub fn generator(
        &self,
        function: Rc<JsFunction>,
        this: JsValue,
        arguments: Vec<JsValue>,
    ) -> ObjRef {
        let mut data = ObjectData::ordinary(Some(self.generator_proto.clone()));
        data.kind = ObjectKind::Generator(crate::object::GeneratorData {
            function,
            this,
            arguments,
            state: crate::object::GeneratorState::SuspendedStart,
        });
        data.class_name = "Generator";
        allocate(data)
    }

    pub fn native_function(
        &self,
        name: &str,
        length: u32,
        function: impl Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue> + 'static,
    ) -> JsValue {
        let function = Rc::new(JsFunction::native(name, length, function));
        JsValue::Object(self.function_object(function))
    }

    /// Create a function carrying a shared data cell that the GC can walk,
    /// the equivalent of QuickJS C functions with data
    /// (`js_c_function_data`, quickjs.c:6595).
    pub fn native_function_data(
        &self,
        name: &str,
        length: u32,
        data: crate::object::NativeData,
        function: impl Fn(&mut Realm, &JsValue, &[JsValue], &[JsValue]) -> JsResult<JsValue> + 'static,
    ) -> JsValue {
        let data = crate::gc::register_data(data);
        let function = Rc::new(JsFunction {
            name: Rc::from(name),
            length,
            native: None,
            native_data: Some(Rc::new(function)),
            data,
            def: None,
            env: None,
            bound: None,
            lexical_this: None,
            instance_init: None,
            has_super: false,
            prototype: std::cell::RefCell::new(None),
        });
        JsValue::Object(self.function_object(function))
    }

    pub fn function_object(&self, function: Rc<JsFunction>) -> ObjRef {
        let object = allocate(ObjectData::new_function(function.clone()));
        {
            let mut data = object.borrow_mut();
            data.prototype = Some(self.function_proto.clone());
            data.define(
                "length",
                crate::object::Property {
                    value: JsValue::number(function.length as f64),
                    get: None,
                    set: None,
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
            data.define(
                "name",
                crate::object::Property {
                    value: match &function.def {
                        Some(def) => JsValue::String(def.name_atom()),
                        None => JsValue::string(function.name.to_string()),
                    },
                    get: None,
                    set: None,
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        object
    }

    pub fn make_js_function(
        &self,
        def: Rc<FunctionDef>,
        env: EnvRef,
        this: Option<JsValue>,
    ) -> JsValue {
        self.make_js_function_inner(def, env, this, None)
    }

    pub fn make_class_constructor(
        &self,
        def: Rc<FunctionDef>,
        env: EnvRef,
        instance_init: Option<Rc<FunctionDef>>,
        has_super: bool,
    ) -> JsValue {
        // Class wiring immediately reads `.prototype`, so build it eagerly.
        let value =
            self.make_js_function_inner_with_super(def, env, None, instance_init, has_super);
        let function = match &value {
            JsValue::Object(object) => match &object.borrow().kind {
                crate::object::ObjectKind::Function(function) => {
                    Some((object.clone(), function.clone()))
                }
                _ => None,
            },
            _ => None,
        };
        if let Some((object, function)) = function {
            self.function_prototype(&object, &function);
        }
        value
    }

    fn make_js_function_inner(
        &self,
        def: Rc<FunctionDef>,
        env: EnvRef,
        this: Option<JsValue>,
        instance_init: Option<Rc<FunctionDef>>,
    ) -> JsValue {
        self.make_js_function_inner_with_super(def, env, this, instance_init, false)
    }

    fn make_js_function_inner_with_super(
        &self,
        def: Rc<FunctionDef>,
        env: EnvRef,
        this: Option<JsValue>,
        instance_init: Option<Rc<FunctionDef>>,
        has_super: bool,
    ) -> JsValue {
        let name = def.name.clone();
        let length = def.length;
        let is_arrow = def.is_arrow;
        let lexical_this = if is_arrow { this } else { None };
        let function = Rc::new(JsFunction {
            name,
            length,
            native: None,
            native_data: None,
            data: Rc::new(RefCell::new(Vec::new())),
            def: Some(def),
            env: Some(env),
            bound: None,
            lexical_this,
            instance_init,
            has_super,
            prototype: std::cell::RefCell::new(None),
        });
        let object = self.function_object(function);
        // `.prototype` is created on first access (`Realm::function_prototype`),
        // like engines that lazily materialize it.
        JsValue::Object(object)
    }

    /// Materialize a constructable function's `.prototype` object on demand
    /// and answer the value (ES2022 10.2.5).
    pub fn function_prototype(&self, object: &ObjRef, function: &JsFunction) -> JsValue {
        if let Some(prototype) = function.prototype.borrow().clone() {
            return JsValue::Object(prototype);
        }
        let prototype = self.object();
        prototype.borrow_mut().define(
            "constructor",
            crate::object::Property::hidden(JsValue::Object(object.clone())),
        );
        *function.prototype.borrow_mut() = Some(prototype.clone());
        object.borrow_mut().define(
            "prototype",
            crate::object::Property {
                value: JsValue::Object(prototype.clone()),
                get: None,
                set: None,
                writable: true,
                enumerable: false,
                configurable: true,
            },
        );
        JsValue::Object(prototype)
    }

    pub fn error_prototype(&self, name: &str) -> ObjRef {
        match name {
            "TypeError" => self.type_error_proto.clone(),
            "RangeError" => self.range_error_proto.clone(),
            "ReferenceError" => self.reference_error_proto.clone(),
            "SyntaxError" => self.syntax_error_proto.clone(),
            "EvalError" => self.eval_error_proto.clone(),
            "URIError" => self.uri_error_proto.clone(),
            _ => self.error_proto.clone(),
        }
    }

    pub fn error_object(&self, name: &str, message: &str) -> JsValue {
        let object = ObjectData::error(Some(self.error_prototype(name)));
        let object = allocate(object);
        object
            .borrow_mut()
            .define("name", crate::object::Property::data(JsValue::string(name)));
        object.borrow_mut().define(
            "message",
            crate::object::Property::data(JsValue::string(message)),
        );
        JsValue::Object(object)
    }

    pub fn type_error(&self, message: impl Into<String>) -> JsValue {
        self.error_object("TypeError", &message.into())
    }

    pub fn reference_error(&self, message: impl Into<String>) -> JsValue {
        self.error_object("ReferenceError", &message.into())
    }

    pub fn syntax_error(&self, message: impl Into<String>) -> JsValue {
        self.error_object("SyntaxError", &message.into())
    }

    pub fn range_error(&self, message: impl Into<String>) -> JsValue {
        self.error_object("RangeError", &message.into())
    }

    pub fn uri_error(&self, message: impl Into<String>) -> JsValue {
        self.error_object("URIError", &message.into())
    }

    pub fn next_symbol(&mut self, description: &str) -> u32 {
        let value = self.symbol_counter;
        self.symbol_counter += 1;
        self.symbols
            .push(crate::string::JsString::from(description));
        value
    }

    pub fn symbol(&mut self, description: &str) -> JsValue {
        let id = self.next_symbol(description);
        JsValue::Symbol(id)
    }

    pub fn symbol_description(&self, id: u32) -> crate::string::JsString {
        self.symbols
            .get(id as usize)
            .cloned()
            .unwrap_or_else(|| crate::string::JsString::from(""))
    }

    pub fn well_known_symbol(&mut self, name: &'static str) -> u32 {
        if let Some(id) = self.well_known_symbols.get(name) {
            return *id;
        }
        let id = self.next_symbol(&format!("Symbol.{name}"));
        self.well_known_symbols.insert(name, id);
        id
    }

    pub fn global_get(&self, key: &str) -> JsValue {
        Object::from_ref(self.global.clone()).get(key)
    }

    pub fn global_set(&self, key: &str, value: impl Into<JsValue>) {
        Object::from_ref(self.global.clone()).set(key, value);
    }

    pub(crate) fn enqueue_job(&mut self, job: crate::vm::Job) {
        self.jobs.push_back(job);
    }

    pub(crate) fn has_pending_job(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// QuickJS `JS_ExecutePendingJob` (quickjs.c:2525): run one queued job.
    pub(crate) fn execute_pending_job(&mut self) -> Option<JsResult<()>> {
        let job = self.jobs.pop_front()?;
        let result = crate::vm::run_job(self, job);
        // A job boundary is a GC safe point.
        crate::gc::trigger_gc();
        Some(result)
    }

    pub(crate) fn drain_jobs(&mut self) -> JsResult<()> {
        while let Some(result) = self.execute_pending_job() {
            result?;
        }
        Ok(())
    }
}

fn allocate(data: ObjectData) -> ObjRef {
    crate::gc::allocate_object(data)
}

pub(crate) fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let mut line = 1;
    let mut column = 1;
    for ch in source[..offset].chars() {
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn iterator_next(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Err(realm.type_error("next called on non-iterator"));
    };
    let reference = object.to_ref();
    let (value, done) = {
        let mut data = reference.borrow_mut();
        match &mut data.kind {
            ObjectKind::Iterator { items, index } => {
                if *index < items.len() {
                    let value = items[*index].clone();
                    *index += 1;
                    (value, false)
                } else {
                    (JsValue::Undefined, true)
                }
            }
            // Live array iterator (ES2022 23.1.5.1.1): reads the current
            // element every step.
            ObjectKind::ArrayIterator {
                array,
                index,
                entries,
            } => {
                let array = array.clone();
                let current = *index;
                let entries = *entries;
                let length = array.borrow().array_len().unwrap_or(0);
                if current >= length {
                    (JsValue::Undefined, true)
                } else {
                    let value = array
                        .borrow()
                        .get_index_value(current as i32)
                        .unwrap_or(JsValue::Undefined);
                    *index += 1;
                    if entries {
                        let entry = realm.array(vec![JsValue::number(current as f64), value]);
                        (JsValue::Object(entry), false)
                    } else {
                        (value, false)
                    }
                }
            }
            ObjectKind::StringIterator { text, index } => {
                let text = text.clone();
                let current = *index;
                let units = crate::coerce::utf16_length(&text);
                if current < units {
                    let unit = crate::object::utf16_char(&text, current);
                    *index += 1;
                    (JsValue::string(unit.unwrap_or_default()), false)
                } else {
                    (JsValue::Undefined, true)
                }
            }
            _ => return Err(realm.type_error("next called on non-iterator")),
        }
    };
    let result = realm.object();
    {
        let mut data = result.borrow_mut();
        data.define("value", crate::object::Property::data(value));
        data.define("done", crate::object::Property::data(JsValue::Bool(done)));
    }
    Ok(JsValue::Object(result))
}

pub struct Runtime {
    context: Context,
}

/// QuickJS `JS_NewRuntime` (quickjs.c:2430) equivalent.
impl Runtime {
    pub fn new() -> Result<Self> {
        Ok(Self {
            context: Context::new(),
        })
    }

    pub fn context(&self) -> Context {
        self.context.clone()
    }

    pub fn eval(&self, source: &str) -> Result<JsValue> {
        self.context.eval(source)
    }

    pub fn execute_pending_job(&self) -> Result<bool> {
        self.context.execute_pending_job()
    }

    pub fn is_job_pending(&self) -> bool {
        self.context.is_job_pending()
    }

    pub fn run_jobs(&self) -> Result<()> {
        self.context.run_jobs()
    }

    /// Drain promise jobs and execute timers until no timers remain.
    #[cfg(feature = "timers")]
    pub fn run_event_loop(&self) -> Result<()> {
        self.context.run_event_loop()
    }

    /// Run a full cycle collection, like `JS_RunGC` (quickjs.c:7483).
    pub fn run_gc(&self) {
        crate::gc::run_gc_now();
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new().expect("runtime initialization")
    }
}

#[derive(Clone)]
pub struct Context {
    pub(crate) realm: Rc<RefCell<Realm>>,
}

impl Context {
    pub fn new() -> Self {
        Self {
            realm: Rc::new(RefCell::new(Realm::new())),
        }
    }

    /// `rquickjs::Context::full`: create a context for a runtime.
    pub fn full(runtime: &Runtime) -> Result<Self> {
        Ok(runtime.context.clone())
    }

    /// Borrow the context for the lifetime of the callback. This is the
    /// main entry point of the rquickjs-style API.
    pub fn with<F, R>(&self, callback: F) -> R
    where
        F: for<'js> FnOnce(crate::api::Ctx<'js>) -> R,
    {
        callback(crate::api::Ctx::new(self))
    }

    /// QuickJS `JS_EvalInternal` (quickjs.c:38266) and
    /// `JS_EvalFunctionInternal` (quickjs.c:38108): parse, emit bytecode,
    /// then run the stack VM.
    pub fn eval(&self, source: &str) -> Result<JsValue> {
        let program = parser::parse(source).map_err(|error| {
            Error::syntax(error.message, error.offset, error.line, error.column)
        })?;
        let definition = compiler::compile_program(&program).map_err(|error| {
            if error.offset > 0 || error.line > 0 {
                Error::syntax(error.message, error.offset, error.line, error.column)
            } else {
                Error::message(error.to_string())
            }
        })?;
        let mut realm = self.realm.borrow_mut();
        let env = Env::new(definition.env_size as usize, None);
        let global = realm.global.clone();
        let value = crate::vm::execute(&mut realm, definition, env, JsValue::Object(global))
            .map_err(|value| {
                let message = crate::vm::display_error(&realm, value.clone());
                let mut error = Error::from_value(value, message);
                if let Some(offset) = crate::vm::take_error_offset() {
                    let (line, column) = line_column(source, offset);
                    error = error.with_position(offset, line, column);
                }
                error
            })?;
        realm.drain_jobs().map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })?;
        // Safe point for the cycle collector, like `js_trigger_gc`
        // (quickjs.c:1679) firing after evaluation.
        crate::gc::trigger_gc();
        Ok(value)
    }

    pub fn eval_file(&self, path: impl AsRef<std::path::Path>) -> Result<JsValue> {
        let source =
            std::fs::read_to_string(path).map_err(|error| Error::message(error.to_string()))?;
        self.eval(&source)
    }

    /// `JS_NewModuleDef` (quickjs.c:4415): parse and compile a module without
    /// running it.
    pub fn declare_module(&self, name: &str, source: &str) -> Result<crate::module::Module<'_>> {
        crate::module::Module::declare(crate::api::Ctx::new(self), name, source)
    }

    /// Declare and evaluate an ES module, returning its exports namespace.
    pub fn eval_module(&self, name: &str, source: &str) -> Result<Object> {
        let exports = self.declare_module(name, source)?.eval()?;
        Ok(exports.into_raw())
    }

    /// Parse a JSON document with the realm's own parser.
    pub fn parse_json(&self, text: &str) -> Result<JsValue> {
        let mut realm = self.realm.borrow_mut();
        crate::builtins::json::parse_json(&mut realm, &JsValue::Undefined, &[JsValue::string(text)])
            .map_err(|value| {
                let message = crate::vm::display_error(&realm, value.clone());
                Error::from_value(value, message)
            })
    }

    /// Serialize a value with the realm's `JSON.stringify`.
    pub fn to_json(&self, value: &JsValue) -> Result<JsValue> {
        let mut realm = self.realm.borrow_mut();
        crate::builtins::json::stringify(
            &mut realm,
            &JsValue::Undefined,
            std::slice::from_ref(value),
        )
        .map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })
    }

    pub fn global(&self) -> Object {
        Object::from_ref(self.realm.borrow().global.clone())
    }

    /// Register a host module returned by `require()` / static `import`.
    pub fn register_module(&self, name: &str, value: JsValue) {
        self.realm
            .borrow_mut()
            .module_cache
            .insert(name.to_string(), value);
    }

    pub fn object(&self) -> Object {
        Object::from_ref(self.realm.borrow().object())
    }

    pub fn array(&self, items: impl IntoIterator<Item = JsValue>) -> Object {
        Object::from_ref(self.realm.borrow().array(items.into_iter().collect()))
    }

    pub fn function(
        &self,
        name: &str,
        length: u32,
        function: impl Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue> + 'static,
    ) -> Function {
        let realm = self.realm.borrow();
        let value = realm.native_function(name, length, function);
        value.as_function().expect("native function")
    }

    pub fn to_string(&self, value: &JsValue) -> Result<String> {
        let mut realm = self.realm.borrow_mut();
        crate::vm::to_string(&mut realm, value.clone()).map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })
    }

    pub fn to_number(&self, value: &JsValue) -> Result<f64> {
        let mut realm = self.realm.borrow_mut();
        crate::vm::to_numeric(&mut realm, value.clone()).map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })
    }

    pub fn eval_value<T: crate::value::FromJs>(&self, source: &str) -> Result<T> {
        let value = self.eval(source)?;
        T::from_js(value).ok_or_else(|| Error::message("Unexpected JavaScript value type"))
    }

    /// Run a full cycle collection, like `JS_RunGC` (quickjs.c:7483).
    pub fn run_gc(&self) {
        crate::gc::run_gc_now();
    }

    /// Largest JS call depth before `RangeError` (QuickJS `rt->stack_limit`).
    pub fn max_call_depth(&self) -> usize {
        self.realm.borrow().max_call_depth
    }

    pub fn set_max_call_depth(&self, depth: usize) {
        self.realm.borrow_mut().max_call_depth = depth.max(32);
    }

    pub fn execute_pending_job(&self) -> Result<bool> {
        let mut realm = self.realm.borrow_mut();
        match realm.execute_pending_job() {
            None => Ok(false),
            Some(Ok(())) => Ok(true),
            Some(Err(value)) => {
                let message = crate::vm::display_error(&realm, value.clone());
                Err(Error::from_value(value, message))
            }
        }
    }

    pub fn is_job_pending(&self) -> bool {
        self.realm.borrow().has_pending_job()
    }

    pub fn run_jobs(&self) -> Result<()> {
        let mut realm = self.realm.borrow_mut();
        realm.drain_jobs().map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })
    }

    /// Run the microtask queue and timers until the timer queue is empty.
    #[cfg(feature = "timers")]
    pub fn run_event_loop(&self) -> Result<()> {
        loop {
            self.run_jobs()?;
            let delay = {
                let realm = self.realm.borrow();
                crate::modules::timers::next_delay(&realm)
            };
            let Some(delay) = delay else {
                break;
            };
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            let mut realm = self.realm.borrow_mut();
            crate::modules::timers::run_due(&mut realm).map_err(|value| {
                let message = crate::vm::display_error(&realm, value.clone());
                Error::from_value(value, message)
            })?;
        }
        Ok(())
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}
