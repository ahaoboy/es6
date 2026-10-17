use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use crate::builtins;
use crate::bytecode::FunctionDef;
use crate::compiler;
use crate::object::{Function, JsFunction, NativeFn, ObjRef, Object, ObjectData, ObjectKind};
use crate::parser;
use crate::value::{Env, EnvRef, JsResult, JsValue};

pub type Result<T> = std::result::Result<T, Error>;

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
/// sufficient. Initialization mirrors `JS_NewContextRaw` (quickjs.c:2892).
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
    pub typed_array_protos: std::collections::HashMap<crate::object::TypedArrayKind, ObjRef>,
    pub symbol_counter: u32,
    pub symbols: Vec<Rc<str>>,
    pub symbol_registry: std::collections::HashMap<String, u32>,
    pub well_known_symbols: std::collections::HashMap<&'static str, u32>,
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
            typed_array_protos,
            symbol_counter: 1,
            symbols: Vec::new(),
            symbol_registry: std::collections::HashMap::new(),
            well_known_symbols: std::collections::HashMap::new(),
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

    pub fn boxed_string(&self, value: Rc<str>) -> ObjRef {
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
        });
        let next = self.native_function("next", 0, iterator_next);
        iterator
            .borrow_mut()
            .define("next", crate::object::Property::hidden(next));
        iterator
    }

    pub fn native_function(&self, name: &str, length: u32, function: NativeFn) -> JsValue {
        let function = Rc::new(JsFunction::native(name, length, function));
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
                    value: JsValue::string(function.name.to_string()),
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
    ) -> JsValue {
        self.make_js_function_inner(def, env, None, instance_init)
    }

    fn make_js_function_inner(
        &self,
        def: Rc<FunctionDef>,
        env: EnvRef,
        this: Option<JsValue>,
        instance_init: Option<Rc<FunctionDef>>,
    ) -> JsValue {
        let name = def.name.clone();
        let length = def.length;
        let is_arrow = def.is_arrow;
        let lexical_this = if is_arrow { this } else { None };
        let function = Rc::new(JsFunction {
            name,
            length,
            native: None,
            def: Some(def),
            env: Some(env),
            bound: None,
            lexical_this,
            instance_init,
        });
        let object = self.function_object(function);
        if !is_arrow {
            let prototype = self.object();
            prototype.borrow_mut().define(
                "constructor",
                crate::object::Property::hidden(JsValue::Object(object.clone())),
            );
            object.borrow_mut().define(
                "prototype",
                crate::object::Property {
                    value: JsValue::Object(prototype),
                    get: None,
                    set: None,
                    writable: true,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        JsValue::Object(object)
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
        self.symbols.push(Rc::from(description));
        value
    }

    pub fn symbol(&mut self, description: &str) -> JsValue {
        let id = self.next_symbol(description);
        JsValue::Symbol(id)
    }

    pub fn symbol_description(&self, id: u32) -> Rc<str> {
        self.symbols
            .get(id as usize)
            .cloned()
            .unwrap_or_else(|| Rc::from(""))
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
}

fn allocate(data: ObjectData) -> ObjRef {
    Rc::new(RefCell::new(data))
}

fn line_column(source: &str, offset: usize) -> (usize, usize) {
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
    pub fn new() -> Self {
        Self {
            context: Context::new(),
        }
    }

    pub fn context(&self) -> Context {
        self.context.clone()
    }

    pub fn eval(&self, source: &str) -> Result<JsValue> {
        self.context.eval(source)
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
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

    /// QuickJS `JS_EvalInternal` (quickjs.c:1470) and `JS_EvalFunctionInternal`
    /// (quickjs.c:38108): parse, emit bytecode, then run the stack VM.
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
        crate::vm::execute(&mut realm, &definition, env, JsValue::Object(global)).map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            let mut error = Error::from_value(value, message);
            if let Some(offset) = crate::vm::take_error_offset() {
                let (line, column) = line_column(source, offset);
                error = error.with_position(offset, line, column);
            }
            error
        })
    }

    pub fn eval_file(&self, path: impl AsRef<std::path::Path>) -> Result<JsValue> {
        let source =
            std::fs::read_to_string(path).map_err(|error| Error::message(error.to_string()))?;
        self.eval(&source)
    }

    pub fn global(&self) -> Object {
        Object::from_ref(self.realm.borrow().global.clone())
    }

    pub fn object(&self) -> Object {
        Object::from_ref(self.realm.borrow().object())
    }

    pub fn array(&self, items: impl IntoIterator<Item = JsValue>) -> Object {
        Object::from_ref(self.realm.borrow().array(items.into_iter().collect()))
    }

    pub fn function(&self, name: &str, length: u32, function: NativeFn) -> Function {
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
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}
