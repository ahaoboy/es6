//! Global timers and the timer half of the event loop.
//!
//! This follows the LLRT timers module: `setTimeout`, `setInterval`,
//! `setImmediate`, `clearTimeout`, `clearInterval` and `queueMicrotask`.

use std::time::{Duration, Instant};

use crate::object::{Object, Property};
use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};

use super::{argument, number_argument};

pub(crate) struct TimerEntry {
    pub(crate) id: u64,
    pub(crate) due: Instant,
    pub(crate) interval: Option<Duration>,
    pub(crate) callback: JsValue,
    pub(crate) arguments: Vec<JsValue>,
}

fn schedule(realm: &mut Realm, arguments: &[JsValue], repeating: bool) -> JsResult<JsValue> {
    let callback = argument(arguments, 0);
    if !callback.is_function() {
        return Err(realm.type_error("callback must be a function"));
    }
    let delay = number_argument(realm, arguments, 1, 0.0)?.max(0.0);
    let id = realm.next_timer_id;
    realm.next_timer_id = realm.next_timer_id.wrapping_add(1);
    let interval = repeating.then(|| Duration::from_secs_f64(delay / 1000.0));
    realm.timers.push(TimerEntry {
        id,
        due: Instant::now() + Duration::from_secs_f64(delay / 1000.0),
        interval,
        callback,
        arguments: arguments.iter().skip(2).cloned().collect(),
    });
    Ok(JsValue::number(id as f64))
}

fn set_timeout(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    schedule(realm, arguments, false)
}

fn set_interval(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    schedule(realm, arguments, true)
}

fn set_immediate(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let callback = argument(arguments, 0);
    if !callback.is_function() {
        return Err(realm.type_error("callback must be a function"));
    }
    let id = realm.next_timer_id;
    realm.next_timer_id = realm.next_timer_id.wrapping_add(1);
    realm.timers.push(TimerEntry {
        id,
        due: Instant::now(),
        interval: None,
        callback,
        arguments: arguments.iter().skip(1).cloned().collect(),
    });
    Ok(JsValue::number(id as f64))
}

fn clear_timer(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let id = number_argument(realm, arguments, 0, f64::NAN)?;
    if id.is_finite() {
        let id = id.max(0.0) as u64;
        let mut removed = false;
        realm.timers.retain(|timer| {
            if timer.id == id {
                removed = true;
                false
            } else {
                true
            }
        });
        if !removed {
            // The timer is currently executing; do not reschedule it.
            realm.cleared_timer_ids.insert(id);
        }
    }
    Ok(JsValue::Undefined)
}

fn queue_microtask(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let callback = argument(arguments, 0);
    if !callback.is_function() {
        return Err(realm.type_error("callback must be a function"));
    }
    realm.enqueue_job(crate::vm::Job::Callback {
        function: callback,
        arguments: Vec::new(),
    });
    Ok(JsValue::Undefined)
}

pub(crate) fn install_globals(realm: &mut Realm) {
    let globals = realm.global.clone();
    let entries: [(&str, u32, crate::object::NativeFn); 6] = [
        ("setTimeout", 2, std::rc::Rc::new(set_timeout)),
        ("setInterval", 2, std::rc::Rc::new(set_interval)),
        ("setImmediate", 1, std::rc::Rc::new(set_immediate)),
        ("clearTimeout", 1, std::rc::Rc::new(clear_timer)),
        ("clearInterval", 1, std::rc::Rc::new(clear_timer)),
        ("queueMicrotask", 1, std::rc::Rc::new(queue_microtask)),
    ];
    for (name, length, function) in entries {
        let value = realm.native_function(name, length, move |realm, this, args| {
            function(realm, this, args)
        });
        globals.borrow_mut().define(name, Property::data(value));
    }
}

pub(crate) fn module(realm: &mut Realm) -> JsValue {
    let timers = Object::from_ref(realm.object());
    let globals = Object::from_ref(realm.global.clone());
    for name in [
        "setTimeout",
        "clearTimeout",
        "setInterval",
        "clearInterval",
        "setImmediate",
        "queueMicrotask",
    ] {
        let function = globals.get(name);
        timers.set(name, function);
    }
    JsValue::Object(timers.into_ref())
}

pub(crate) fn next_delay(realm: &Realm) -> Option<Duration> {
    let now = Instant::now();
    realm
        .timers
        .iter()
        .map(|timer| timer.due.saturating_duration_since(now))
        .min()
}

/// Run the earliest due timer. Returns false when no timer is due.
pub(crate) fn run_due(realm: &mut Realm) -> JsResult<bool> {
    let now = Instant::now();
    let Some(index) = realm
        .timers
        .iter()
        .enumerate()
        .filter(|(_, timer)| timer.due <= now)
        .min_by_key(|(_, timer)| (timer.due, timer.id))
        .map(|(index, _)| index)
    else {
        return Ok(false);
    };
    let mut timer = realm.timers.remove(index);
    crate::vm::call_value(
        realm,
        timer.callback.clone(),
        JsValue::Undefined,
        &timer.arguments,
    )?;
    let cancelled = realm.cleared_timer_ids.remove(&timer.id);
    if let Some(interval) = timer.interval
        && !cancelled
    {
        timer.due = Instant::now() + interval;
        realm.timers.push(timer);
    }
    Ok(true)
}
