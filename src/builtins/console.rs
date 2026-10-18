use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::time::Instant;

use super::*;
use crate::value::JsValue;

// QuickJS: standalone console implementation (quickjs-libc.c, js_console_*).
pub fn install(realm: &mut Realm) {
    let console = realm.object();
    define_method(realm, &console, "log", 0, log);
    define_method(realm, &console, "info", 0, log);
    define_method(realm, &console, "debug", 0, log);
    define_method(realm, &console, "warn", 0, warn);
    define_method(realm, &console, "error", 0, error);
    define_method(realm, &console, "dir", 1, log);
    define_method(realm, &console, "trace", 0, warn);
    define_method(realm, &console, "assert", 1, assert);
    define_method(realm, &console, "clear", 0, clear);
    define_method(realm, &console, "count", 0, count);
    define_method(realm, &console, "countReset", 0, count_reset);
    define_method(realm, &console, "time", 0, time);
    define_method(realm, &console, "timeLog", 1, time_log);
    define_method(realm, &console, "timeEnd", 0, time_end);
    define_builtin_object(realm, "console", &console);
}

thread_local! {
    static COUNTERS: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    static TIMERS: RefCell<HashMap<String, Instant>> = RefCell::new(HashMap::new());
}

fn label(realm: &mut Realm, arguments: &[JsValue]) -> JsResult<String> {
    let value = arg(arguments, 0);
    if value.is_undefined() {
        return Ok("default".to_string());
    }
    crate::vm::to_string(realm, value)
}

fn format(realm: &mut Realm, values: &[JsValue]) -> JsResult<String> {
    let mut parts = Vec::with_capacity(values.len());
    for value in values {
        parts.push(crate::vm::to_string(realm, value.clone())?);
    }
    Ok(parts.join(" "))
}

fn log(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    println!("{}", format(realm, arguments)?);
    Ok(JsValue::Undefined)
}

fn warn(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    eprintln!("{}", format(realm, arguments)?);
    Ok(JsValue::Undefined)
}

fn error(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    eprintln!("{}", format(realm, arguments)?);
    Ok(JsValue::Undefined)
}

fn assert(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    if !crate::vm::to_boolean(&arg(arguments, 0)) {
        eprintln!(
            "Assertion failed: {}",
            format(realm, arguments.get(1..).unwrap_or(&[]))?
        );
    }
    Ok(JsValue::Undefined)
}

fn clear(_realm: &mut Realm, _this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    print!("\x1b[2J\x1b[H");
    let _ = std::io::stdout().flush();
    Ok(JsValue::Undefined)
}

fn count(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let label = label(realm, arguments)?;
    let value = COUNTERS.with(|counters| {
        let mut counters = counters.borrow_mut();
        let value = counters.entry(label.clone()).or_insert(0);
        *value += 1;
        *value
    });
    println!("{label}: {value}");
    Ok(JsValue::Undefined)
}

fn count_reset(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let label = label(realm, arguments)?;
    COUNTERS.with(|counters| {
        counters.borrow_mut().remove(&label);
    });
    Ok(JsValue::Undefined)
}

fn time(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let label = label(realm, arguments)?;
    TIMERS.with(|timers| {
        timers
            .borrow_mut()
            .entry(label)
            .or_insert_with(Instant::now);
    });
    Ok(JsValue::Undefined)
}

fn elapsed(label: &str) -> Option<u128> {
    TIMERS.with(|timers| {
        timers
            .borrow()
            .get(label)
            .map(|start| start.elapsed().as_millis())
    })
}

fn time_log(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let label = label(realm, arguments)?;
    let Some(elapsed) = elapsed(&label) else {
        return Ok(JsValue::Undefined);
    };
    let rest = format(realm, arguments.get(1..).unwrap_or(&[]))?;
    if rest.is_empty() {
        println!("{label}: {elapsed}ms");
    } else {
        println!("{label}: {elapsed}ms {rest}");
    }
    Ok(JsValue::Undefined)
}

fn time_end(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let label = label(realm, arguments)?;
    let elapsed = elapsed(&label);
    TIMERS.with(|timers| {
        timers.borrow_mut().remove(&label);
    });
    if let Some(elapsed) = elapsed {
        println!("{label}: {elapsed}ms");
    }
    Ok(JsValue::Undefined)
}
