//! Optional host modules exposed through a Node-style `require()`.
//!
//! Each submodule can be enabled independently through Cargo features:
//! `fs`, `os`, `path`, `console` and `timers`. All of them are enabled by
//! default.

#[cfg(feature = "console")]
pub(crate) mod console;
#[cfg(feature = "fs")]
pub(crate) mod fs;
#[cfg(feature = "os")]
pub(crate) mod os;
#[cfg(feature = "path")]
pub(crate) mod path;
#[cfg(feature = "timers")]
pub(crate) mod timers;

#[cfg(any(feature = "fs", feature = "os", feature = "path"))]
use crate::object::ObjRef;
use crate::object::Property;
use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};

pub(crate) fn install(realm: &mut Realm) {
    #[cfg(feature = "timers")]
    timers::install_globals(realm);

    let require = realm.native_function("require", 1, require_module);
    crate::builtins::define_builtin_object(
        realm,
        "require",
        &require.as_object().expect("native function").to_ref(),
    );
}

fn require_module(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let name = match crate::builtins::arg(arguments, 0) {
        JsValue::String(name) => name.to_string(),
        value => crate::vm::to_string(realm, value)?,
    };
    let name = name.strip_prefix("node:").unwrap_or(&name);

    let cache_key = match name {
        "fs/promises" => "fs/promises",
        "path/posix" | "path/win32" => "path",
        other => other,
    };
    if let Some(module) = realm.module_cache.get(cache_key) {
        return Ok(module.clone());
    }

    let module: Option<JsValue> = match name {
        #[cfg(feature = "fs")]
        "fs" => Some(fs::module(realm)),
        #[cfg(feature = "fs")]
        "fs/promises" => Some(fs::promises_module(realm)),
        #[cfg(feature = "os")]
        "os" => Some(os::module(realm)),
        #[cfg(feature = "path")]
        "path" => Some(path::module(realm)),
        #[cfg(feature = "path")]
        "path/posix" | "path/win32" => Some(path::module(realm)),
        #[cfg(feature = "timers")]
        "timers" => Some(timers::module(realm)),
        #[cfg(feature = "console")]
        "console" => Some(console::module(realm)),
        _ => None,
    };

    match module {
        Some(module) => {
            realm
                .module_cache
                .insert(cache_key.to_string(), module.clone());
            Ok(module)
        }
        None => {
            let error = realm.error_object("Error", &format!("Cannot find module '{name}'"));
            if let JsValue::Object(object) = &error {
                object
                    .borrow_mut()
                    .define("code", Property::data(JsValue::string("MODULE_NOT_FOUND")));
            }
            Err(error)
        }
    }
}

#[cfg(any(feature = "fs", feature = "os", feature = "path"))]
pub(crate) fn set(object: &ObjRef, name: &str, value: impl Into<JsValue>) {
    object
        .borrow_mut()
        .define(name, Property::data(value.into()));
}

#[cfg(any(feature = "fs", feature = "os", feature = "path"))]
pub(crate) fn set_method(
    realm: &mut Realm,
    object: &ObjRef,
    name: &str,
    length: u32,
    function: impl Fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue> + 'static,
) {
    let function = realm.native_function(name, length, function);
    object.borrow_mut().define(name, Property::hidden(function));
}

#[cfg(any(feature = "fs", feature = "os", feature = "path", feature = "timers"))]
pub(crate) fn argument(arguments: &[JsValue], index: usize) -> JsValue {
    arguments.get(index).cloned().unwrap_or(JsValue::Undefined)
}

#[cfg(any(feature = "fs", feature = "path"))]
pub(crate) fn string_argument(
    realm: &mut Realm,
    arguments: &[JsValue],
    index: usize,
) -> JsResult<String> {
    crate::vm::to_string(realm, argument(arguments, index))
}

#[cfg(any(feature = "fs", feature = "os", feature = "timers"))]
pub(crate) fn number_argument(
    realm: &mut Realm,
    arguments: &[JsValue],
    index: usize,
    default: f64,
) -> JsResult<f64> {
    let value = argument(arguments, index);
    if value.is_undefined() {
        return Ok(default);
    }
    crate::vm::to_number(realm, value)
}

#[cfg(feature = "fs")]
pub(crate) fn error_with_code(realm: &Realm, code: &str, message: String) -> JsValue {
    let error = realm.error_object("Error", &message);
    if let JsValue::Object(object) = &error {
        object
            .borrow_mut()
            .define("code", Property::data(JsValue::string(code)));
    }
    error
}

#[cfg(feature = "fs")]
pub(crate) fn io_error(realm: &Realm, error: std::io::Error, path: &str) -> JsValue {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        std::io::ErrorKind::AlreadyExists => "EEXIST",
        std::io::ErrorKind::InvalidInput => "EINVAL",
        _ => "EIO",
    };
    error_with_code(realm, code, format!("{code}: {error}, '{path}'"))
}

#[cfg(feature = "fs")]
pub(crate) fn promise_value(realm: &mut Realm, result: JsResult<JsValue>) -> JsResult<JsValue> {
    let promise = realm.promise();
    match result {
        Ok(value) => crate::promise::resolve(realm, &promise, value)?,
        Err(error) => crate::promise::reject(realm, &promise, error)?,
    }
    Ok(JsValue::Object(promise))
}
