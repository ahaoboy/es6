//! `path` module, following LLRT's platform-native path implementation.

use std::path::{Component, MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf};

use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};

use super::{argument, set, set_method, string_argument};

const DELIMITER: &str = if cfg!(windows) { ";" } else { ":" };

pub(crate) fn module(realm: &mut Realm) -> JsValue {
    let path = realm.object();
    set(&path, "sep", JsValue::string(MAIN_SEPARATOR_STR));
    set(&path, "delimiter", JsValue::string(DELIMITER));

    set_method(realm, &path, "dirname", 1, dirname);
    set_method(realm, &path, "basename", 2, basename);
    set_method(realm, &path, "extname", 1, extname);
    set_method(realm, &path, "format", 1, format);
    set_method(realm, &path, "parse", 1, parse);
    set_method(realm, &path, "join", 0, join);
    set_method(realm, &path, "resolve", 0, resolve);
    set_method(realm, &path, "relative", 2, relative);
    set_method(realm, &path, "normalize", 1, normalize);
    set_method(realm, &path, "isAbsolute", 1, is_absolute);
    set_method(
        realm,
        &path,
        "toNamespacedPath",
        1,
        |realm, _, arguments| Ok(JsValue::string(string_argument(realm, arguments, 0)?)),
    );
    set(&path, "posix", JsValue::Object(path.clone()));
    set(&path, "win32", JsValue::Object(path.clone()));
    JsValue::Object(path)
}

fn dirname(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = string_argument(realm, arguments, 0)?;
    let path = Path::new(&value);
    Ok(JsValue::string(
        path.parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".to_string()),
    ))
}

fn basename(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = string_argument(realm, arguments, 0)?;
    let mut name = Path::new(&value)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(suffix) = argument(arguments, 1).as_string()
        && !suffix.is_empty()
        && name.ends_with(suffix.as_ref())
    {
        name.truncate(name.len() - suffix.len());
    }
    Ok(JsValue::string(name))
}

fn extname(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = string_argument(realm, arguments, 0)?;
    let name = Path::new(&value)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = match name.rfind('.') {
        Some(0) | None => String::new(),
        Some(index) => name[index..].to_string(),
    };
    Ok(JsValue::string(extension))
}

fn format(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let JsValue::Object(object) = argument(arguments, 0) else {
        return Err(realm.type_error("path.format expects an object"));
    };
    let mut get = |name: &str| -> JsResult<Option<String>> {
        let key = crate::object::PropKey::from(name);
        if !object.borrow().has_own(&key) {
            return Ok(None);
        }
        let value = crate::vm::get_value_property(realm, &JsValue::Object(object.clone()), &key)?;
        if value.is_nullish() {
            Ok(None)
        } else {
            Ok(Some(crate::vm::to_string(realm, value)?))
        }
    };
    let root = get("root")?.unwrap_or_default();
    let dir = get("dir")?.unwrap_or_default();
    let base = get("base")?.unwrap_or_default();
    let name = get("name")?.unwrap_or_default();
    let ext = get("ext")?.unwrap_or_default();

    let mut result = String::new();
    if !dir.is_empty() {
        result.push_str(&dir);
        if !result.ends_with(MAIN_SEPARATOR) && !result.ends_with('/') {
            result.push(MAIN_SEPARATOR);
        }
        result.push_str(&if base.is_empty() {
            format!("{name}{ext}")
        } else {
            base
        });
    } else if !root.is_empty() {
        result.push_str(&root);
        result.push_str(&if base.is_empty() {
            format!("{name}{ext}")
        } else {
            base
        });
    } else {
        result.push_str(&if base.is_empty() {
            format!("{name}{ext}")
        } else {
            base
        });
    }
    Ok(JsValue::string(result))
}

fn parse(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = string_argument(realm, arguments, 0)?;
    let path = Path::new(&value);
    let base = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = extname(realm, _this, &[JsValue::string(base.clone())])?
        .as_string()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let name = if base.is_empty() {
        String::new()
    } else {
        base[..base.len().saturating_sub(ext.len())].to_string()
    };
    let dir = path
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();
    let root = path
        .components()
        .next()
        .filter(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();

    let result = realm.object();
    set(&result, "root", JsValue::string(root));
    set(&result, "dir", JsValue::string(dir));
    set(&result, "base", JsValue::string(base));
    set(&result, "ext", JsValue::string(ext));
    set(&result, "name", JsValue::string(name));
    Ok(JsValue::Object(result))
}

fn join(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let mut result = String::new();
    for index in 0..arguments.len() {
        let part = string_argument(realm, arguments, index)?;
        if part.is_empty() {
            continue;
        }
        if result.is_empty() {
            result.push_str(&part);
        } else {
            if !result.ends_with(MAIN_SEPARATOR) && !result.ends_with('/') {
                result.push(MAIN_SEPARATOR);
            }
            result.push_str(part.trim_start_matches(['/', '\\']));
        }
    }
    if result.is_empty() {
        return Ok(JsValue::string("."));
    }
    Ok(JsValue::string(normalize_path(&result)))
}

fn resolve(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let mut path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for index in 0..arguments.len() {
        let part = string_argument(realm, arguments, index)?;
        if part.is_empty() {
            continue;
        }
        let candidate = Path::new(&part);
        if candidate.is_absolute() {
            path = candidate.to_path_buf();
        } else {
            path.push(candidate);
        }
    }
    Ok(JsValue::string(
        normalize_path(&path.to_string_lossy()).to_string(),
    ))
}

fn relative(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let from = string_argument(realm, arguments, 0)?;
    let to = string_argument(realm, arguments, 1)?;
    let from = absolute_path(&from);
    let to = absolute_path(&to);
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();
    let mut common = 0;
    while common < from_components.len()
        && common < to_components.len()
        && from_components[common] == to_components[common]
    {
        common += 1;
    }
    let mut parts = Vec::new();
    for _ in common..from_components.len() {
        parts.push("..".to_string());
    }
    for component in &to_components[common..] {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    Ok(JsValue::string(parts.join(MAIN_SEPARATOR_STR)))
}

fn normalize(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = string_argument(realm, arguments, 0)?;
    if value.is_empty() {
        return Ok(JsValue::string("."));
    }
    Ok(JsValue::string(normalize_path(&value)))
}

fn is_absolute(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = string_argument(realm, arguments, 0)?;
    Ok(JsValue::Bool(Path::new(&value).is_absolute()))
}

fn absolute_path(value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn normalize_path(value: &str) -> String {
    let path = Path::new(value);
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    result.push(component.as_os_str());
                }
            }
            _ => result.push(component.as_os_str()),
        }
    }
    let text = result.to_string_lossy().into_owned();
    if text.is_empty() {
        ".".to_string()
    } else {
        text
    }
}
