//! Synchronous `fs` subset and `fs.promises`, modelled on LLRT's module.
//!
//! The engine has no thread pool, so promise variants perform the operation
//! immediately and return an already-settled promise.

use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::object::{ObjectKind, TypedArrayData, TypedArrayKind};
use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};

use super::{
    argument, error_with_code, io_error, number_argument, promise_value, set_method,
    string_argument,
};

const F_OK: i32 = 0;
const R_OK: i32 = 4;
const W_OK: i32 = 2;
const X_OK: i32 = 1;

pub(crate) fn module(realm: &mut Realm) -> JsValue {
    let module = realm.object();
    let promises = promises_object(realm);
    super::set(&module, "promises", JsValue::Object(promises));

    set_method(realm, &module, "readFileSync", 2, read_file_sync);
    set_method(realm, &module, "writeFileSync", 3, write_file_sync);
    set_method(realm, &module, "appendFileSync", 3, append_file_sync);
    set_method(realm, &module, "existsSync", 1, exists_sync);
    set_method(realm, &module, "readdirSync", 2, readdir_sync);
    set_method(realm, &module, "mkdirSync", 2, mkdir_sync);
    set_method(realm, &module, "mkdtempSync", 2, mkdtemp_sync);
    set_method(realm, &module, "rmSync", 2, rm_sync);
    set_method(realm, &module, "rmdirSync", 2, rmdir_sync);
    set_method(realm, &module, "unlinkSync", 1, unlink_sync);
    set_method(realm, &module, "renameSync", 2, rename_sync);
    set_method(realm, &module, "copyFileSync", 3, copy_file_sync);
    set_method(realm, &module, "statSync", 2, stat_sync);
    set_method(realm, &module, "lstatSync", 2, lstat_sync);
    set_method(realm, &module, "realpathSync", 2, realpath_sync);
    set_method(realm, &module, "accessSync", 2, access_sync);
    set_method(realm, &module, "chmodSync", 2, chmod_sync);
    set_method(realm, &module, "symlinkSync", 3, symlink_sync);

    let constants = realm.object();
    super::set(&constants, "F_OK", JsValue::Int(F_OK));
    super::set(&constants, "R_OK", JsValue::Int(R_OK));
    super::set(&constants, "W_OK", JsValue::Int(W_OK));
    super::set(&constants, "X_OK", JsValue::Int(X_OK));
    super::set(&module, "constants", JsValue::Object(constants));
    JsValue::Object(module)
}

pub(crate) fn promises_module(realm: &mut Realm) -> JsValue {
    JsValue::Object(promises_object(realm))
}

fn promises_object(realm: &mut Realm) -> crate::object::ObjRef {
    let promises = realm.object();
    set_method(realm, &promises, "readFile", 2, promise_read_file);
    set_method(realm, &promises, "writeFile", 3, promise_write_file);
    set_method(realm, &promises, "appendFile", 3, promise_append_file);
    set_method(realm, &promises, "readdir", 2, promise_readdir);
    set_method(realm, &promises, "mkdir", 2, promise_mkdir);
    set_method(realm, &promises, "mkdtemp", 2, promise_mkdtemp);
    set_method(realm, &promises, "rm", 2, promise_rm);
    set_method(realm, &promises, "rmdir", 2, promise_rmdir);
    set_method(realm, &promises, "unlink", 1, promise_unlink);
    set_method(realm, &promises, "rename", 2, promise_rename);
    set_method(realm, &promises, "copyFile", 3, promise_copy_file);
    set_method(realm, &promises, "stat", 2, promise_stat);
    set_method(realm, &promises, "lstat", 2, promise_lstat);
    set_method(realm, &promises, "realpath", 2, promise_realpath);
    set_method(realm, &promises, "access", 2, promise_access);
    set_method(realm, &promises, "chmod", 2, promise_chmod);
    set_method(realm, &promises, "symlink", 3, promise_symlink);
    promises
}

fn path_argument(realm: &mut Realm, arguments: &[JsValue]) -> JsResult<String> {
    string_argument(realm, arguments, 0)
}

fn option(realm: &mut Realm, value: JsValue, name: &str) -> JsResult<Option<JsValue>> {
    let JsValue::Object(object) = value else {
        return Ok(None);
    };
    let key = crate::object::PropKey::from(name);
    if !object.borrow().has_own(&key) {
        return Ok(None);
    }
    let object = JsValue::Object(object);
    Ok(Some(crate::vm::get_value_property(realm, &object, &key)?))
}

fn encoding(realm: &mut Realm, arguments: &[JsValue], index: usize) -> JsResult<Option<String>> {
    match argument(arguments, index) {
        JsValue::String(encoding) => Ok(Some(encoding.to_string())),
        value @ JsValue::Object(_) => {
            if let Some(encoding) = option(realm, value, "encoding")?
                && !encoding.is_nullish()
            {
                return Ok(Some(crate::vm::to_string(realm, encoding)?));
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn flag(realm: &mut Realm, arguments: &[JsValue], index: usize, name: &str) -> JsResult<bool> {
    match option(realm, argument(arguments, index), name)? {
        Some(value) => Ok(crate::vm::to_boolean(&value)),
        None => Ok(false),
    }
}

fn text_from_bytes(bytes: Vec<u8>) -> JsValue {
    JsValue::string(String::from_utf8_lossy(&bytes).into_owned())
}

fn buffer_value(realm: &Realm, bytes: Vec<u8>) -> JsValue {
    let length = bytes.len();
    let buffer = realm.array_buffer_from(bytes);
    let bytes = match &buffer.borrow().kind {
        ObjectKind::ArrayBuffer(data) => data.bytes.clone(),
        _ => unreachable!(),
    };
    let array = realm.typed_array(TypedArrayData {
        kind: TypedArrayKind::Uint8,
        buffer,
        bytes,
        byte_offset: 0,
        length,
    });
    JsValue::Object(array)
}

fn bytes_from_value(realm: &mut Realm, value: JsValue) -> JsResult<Vec<u8>> {
    match value {
        JsValue::String(text) => Ok(text.as_bytes().to_vec()),
        JsValue::Object(object) => {
            let data = object.borrow();
            match &data.kind {
                ObjectKind::TypedArray(array) => {
                    let bytes = array.bytes.borrow();
                    Ok(bytes[array.byte_offset..array.byte_offset + array.byte_length()].to_vec())
                }
                ObjectKind::ArrayBuffer(buffer) => Ok(buffer.bytes.borrow().clone()),
                _ => {
                    drop(data);
                    Ok(crate::vm::to_string(realm, JsValue::Object(object))?.into_bytes())
                }
            }
        }
        value => Ok(crate::vm::to_string(realm, value)?.into_bytes()),
    }
}

fn read_file_bytes(realm: &Realm, path: &str) -> JsResult<Vec<u8>> {
    std::fs::read(path).map_err(|error| io_error(realm, error, path))
}

fn read_file_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    let bytes = read_file_bytes(realm, &path)?;
    if encoding(realm, arguments, 1)?.is_some() {
        Ok(text_from_bytes(bytes))
    } else {
        Ok(buffer_value(realm, bytes))
    }
}

fn write_file_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    let bytes = bytes_from_value(realm, argument(arguments, 1))?;
    std::fs::write(&path, bytes).map_err(|error| io_error(realm, error, &path))?;
    Ok(JsValue::Undefined)
}

fn append_file_sync(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    use std::io::Write;
    let path = path_argument(realm, arguments)?;
    let bytes = bytes_from_value(realm, argument(arguments, 1))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| io_error(realm, error, &path))?;
    file.write_all(&bytes)
        .map_err(|error| io_error(realm, error, &path))?;
    Ok(JsValue::Undefined)
}

fn exists_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let Ok(path) = path_argument(realm, arguments) else {
        return Ok(JsValue::Bool(false));
    };
    Ok(JsValue::Bool(Path::new(&path).exists()))
}

fn readdir_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    let with_types = flag(realm, arguments, 1, "withFileTypes")?;
    let entries = std::fs::read_dir(&path).map_err(|error| io_error(realm, error, &path))?;
    let mut values = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_error(realm, error, &path))?;
        if with_types {
            values.push(dirent_value(realm, &entry));
        } else {
            values.push(JsValue::string(
                entry.file_name().to_string_lossy().into_owned(),
            ));
        }
    }
    Ok(JsValue::Object(realm.array(values)))
}

fn dirent_value(realm: &mut Realm, entry: &std::fs::DirEntry) -> JsValue {
    let file_type = entry.file_type().ok();
    let object = realm.object();
    super::set(
        &object,
        "name",
        JsValue::string(entry.file_name().to_string_lossy().into_owned()),
    );
    let is_file = file_type.is_some_and(|file_type| file_type.is_file());
    let is_dir = file_type.is_some_and(|file_type| file_type.is_dir());
    let is_symlink = file_type.is_some_and(|file_type| file_type.is_symlink());
    let method = |value: bool| {
        move |_realm: &mut Realm, _this: &JsValue, _arguments: &[JsValue]| Ok(JsValue::Bool(value))
    };
    set_method(realm, &object, "isFile", 0, method(is_file));
    set_method(realm, &object, "isDirectory", 0, method(is_dir));
    set_method(realm, &object, "isSymbolicLink", 0, method(is_symlink));
    JsValue::Object(object)
}

fn mkdir_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    let recursive = flag(realm, arguments, 1, "recursive")?;
    let result = if recursive {
        std::fs::create_dir_all(&path)
    } else {
        std::fs::create_dir(&path)
    };
    result.map_err(|error| io_error(realm, error, &path))?;
    Ok(JsValue::Undefined)
}

fn mkdtemp_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let prefix = path_argument(realm, arguments)?;
    for attempt in 0..1000u32 {
        let path = format!("{prefix}{}{}", std::process::id(), attempt);
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(JsValue::string(path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(realm, error, &path)),
        }
    }
    Err(error_with_code(
        realm,
        "EEXIST",
        "mkdtemp: could not create a unique directory".to_string(),
    ))
}

fn rm_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    let recursive = flag(realm, arguments, 1, "recursive")?;
    let force = flag(realm, arguments, 1, "force")?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if force && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(JsValue::Undefined);
        }
        Err(error) => return Err(io_error(realm, error, &path)),
    };
    let result = if metadata.is_dir() {
        if recursive {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_dir(&path)
        }
    } else {
        std::fs::remove_file(&path)
    };
    match result {
        Ok(()) => Ok(JsValue::Undefined),
        Err(error) if force && error.kind() == std::io::ErrorKind::NotFound => {
            Ok(JsValue::Undefined)
        }
        Err(error) => Err(io_error(realm, error, &path)),
    }
}

fn rmdir_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    std::fs::remove_dir(&path).map_err(|error| io_error(realm, error, &path))?;
    Ok(JsValue::Undefined)
}

fn unlink_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    std::fs::remove_file(&path).map_err(|error| io_error(realm, error, &path))?;
    Ok(JsValue::Undefined)
}

fn rename_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let from = string_argument(realm, arguments, 0)?;
    let to = string_argument(realm, arguments, 1)?;
    std::fs::rename(&from, &to).map_err(|error| io_error(realm, error, &from))?;
    Ok(JsValue::Undefined)
}

fn copy_file_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let from = string_argument(realm, arguments, 0)?;
    let to = string_argument(realm, arguments, 1)?;
    std::fs::copy(&from, &to).map_err(|error| io_error(realm, error, &from))?;
    Ok(JsValue::Undefined)
}

fn stat_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    let metadata = std::fs::metadata(&path).map_err(|error| io_error(realm, error, &path))?;
    Ok(JsValue::Object(stats_object(realm, &metadata)))
}

fn lstat_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|error| io_error(realm, error, &path))?;
    Ok(JsValue::Object(stats_object(realm, &metadata)))
}

fn stats_object(realm: &mut Realm, metadata: &std::fs::Metadata) -> crate::object::ObjRef {
    let object = realm.object();
    let millis = |time: std::io::Result<std::time::SystemTime>| {
        time.ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0.0, |duration| duration.as_secs_f64() * 1000.0)
    };
    super::set(&object, "size", JsValue::number(metadata.len() as f64));
    super::set(
        &object,
        "mtimeMs",
        JsValue::number(millis(metadata.modified())),
    );
    super::set(
        &object,
        "atimeMs",
        JsValue::number(millis(metadata.accessed())),
    );
    super::set(
        &object,
        "ctimeMs",
        JsValue::number(millis(metadata.created())),
    );
    super::set(
        &object,
        "birthtimeMs",
        JsValue::number(millis(metadata.created())),
    );
    super::set(&object, "mode", JsValue::number(0.0));
    super::set(&object, "uid", JsValue::Int(0));
    super::set(&object, "gid", JsValue::Int(0));
    let is_file = metadata.is_file();
    let is_dir = metadata.is_dir();
    let is_symlink = metadata.file_type().is_symlink();
    let method = |value: bool| {
        move |_realm: &mut Realm, _this: &JsValue, _arguments: &[JsValue]| Ok(JsValue::Bool(value))
    };
    set_method(realm, &object, "isFile", 0, method(is_file));
    set_method(realm, &object, "isDirectory", 0, method(is_dir));
    set_method(realm, &object, "isSymbolicLink", 0, method(is_symlink));
    object
}

fn realpath_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    let path = std::fs::canonicalize(&path).map_err(|error| io_error(realm, error, &path))?;
    Ok(JsValue::string(path.to_string_lossy().into_owned()))
}

fn access_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    std::fs::metadata(&path).map_err(|error| io_error(realm, error, &path))?;
    Ok(JsValue::Undefined)
}

fn chmod_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let path = path_argument(realm, arguments)?;
    let mode = number_argument(realm, arguments, 1, 0.0)? as u32;
    let metadata = std::fs::metadata(&path).map_err(|error| io_error(realm, error, &path))?;
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(mode);
    }
    #[cfg(not(unix))]
    {
        permissions.set_readonly(mode & 0o200 == 0);
    }
    std::fs::set_permissions(&path, permissions).map_err(|error| io_error(realm, error, &path))?;
    Ok(JsValue::Undefined)
}

fn symlink_sync(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let target = string_argument(realm, arguments, 0)?;
    let path = string_argument(realm, arguments, 1)?;
    #[cfg(windows)]
    {
        let result = if Path::new(&target).is_dir() {
            std::os::windows::fs::symlink_dir(&target, &path)
        } else {
            std::os::windows::fs::symlink_file(&target, &path)
        };
        result.map_err(|error| io_error(realm, error, &path))?;
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(&target, &path)
            .map_err(|error| io_error(realm, error, &path))?;
    }
    Ok(JsValue::Undefined)
}

macro_rules! promise_wrapper {
    ($name:ident, $target:ident, $length:expr) => {
        fn $name(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
            let this = JsValue::Undefined;
            let result = $target(realm, &this, arguments);
            promise_value(realm, result)
        }
    };
}

promise_wrapper!(promise_read_file, read_file_sync, 2);
promise_wrapper!(promise_write_file, write_file_sync, 3);
promise_wrapper!(promise_append_file, append_file_sync, 3);
promise_wrapper!(promise_readdir, readdir_sync, 2);
promise_wrapper!(promise_mkdir, mkdir_sync, 2);
promise_wrapper!(promise_mkdtemp, mkdtemp_sync, 2);
promise_wrapper!(promise_rm, rm_sync, 2);
promise_wrapper!(promise_rmdir, rmdir_sync, 2);
promise_wrapper!(promise_unlink, unlink_sync, 1);
promise_wrapper!(promise_rename, rename_sync, 2);
promise_wrapper!(promise_copy_file, copy_file_sync, 3);
promise_wrapper!(promise_stat, stat_sync, 2);
promise_wrapper!(promise_lstat, lstat_sync, 2);
promise_wrapper!(promise_realpath, realpath_sync, 2);
promise_wrapper!(promise_access, access_sync, 2);
promise_wrapper!(promise_chmod, chmod_sync, 2);
promise_wrapper!(promise_symlink, symlink_sync, 3);
