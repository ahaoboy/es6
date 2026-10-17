use super::*;
use crate::object::Property;
use crate::value::JsValue;

// QuickJS-ng exposes MessageChannel in its web-compatible host layer. The
// object model mirrors `JS_NewObjectClass` plus native methods; delivery here
// is synchronous because the standalone CLI has no event loop.
pub fn install(realm: &mut Realm) {
    let channel = realm.native_function("MessageChannel", 0, message_channel);
    let port = realm.native_function("MessagePort", 0, message_port);
    realm.global_set("MessageChannel", channel);
    realm.global_set("MessagePort", port);
}

fn message_channel(
    realm: &mut Realm,
    _this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    let first = make_port(realm)?;
    let second = make_port(realm)?;
    first.set("__peer__", JsValue::Object(second.to_ref()));
    second.set("__peer__", JsValue::Object(first.to_ref()));
    let channel = realm.object();
    channel
        .borrow_mut()
        .define("port1", Property::data(JsValue::Object(first.into_ref())));
    channel
        .borrow_mut()
        .define("port2", Property::data(JsValue::Object(second.into_ref())));
    Ok(JsValue::Object(channel))
}

fn message_port(realm: &mut Realm, _this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Object(make_port(realm)?.into_ref()))
}

fn make_port(realm: &mut Realm) -> JsResult<Object> {
    let port = Object::from_ref(realm.object());
    port.define(
        "onmessage",
        Property {
            value: JsValue::Null,
            get: None,
            set: None,
            writable: true,
            enumerable: true,
            configurable: true,
        },
    );
    let post = realm.native_function("postMessage", 1, post_message);
    let close = realm.native_function("close", 0, close_port);
    let start = realm.native_function("start", 0, start_port);
    port.define("postMessage", Property::hidden(post));
    port.define("close", Property::hidden(close));
    port.define("start", Property::hidden(start));
    Ok(port)
}

fn post_message(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let Some(port) = this.as_object() else {
        return Err(realm.type_error("MessagePort method called on incompatible receiver"));
    };
    if port.get("__closed__").is_truthy() {
        return Ok(JsValue::Undefined);
    }
    let peer = port.get("__peer__").as_object();
    if let Some(peer) = peer {
        if peer.get("__closed__").is_truthy() {
            return Ok(JsValue::Undefined);
        }
        let handler = peer.get("onmessage");
        if handler.is_function() {
            let event = Object::from_ref(realm.object());
            event.set("data", arg(arguments, 0));
            super::call(
                realm,
                handler,
                JsValue::Object(peer.to_ref()),
                &[JsValue::Object(event.into_ref())],
            )?;
        }
    }
    Ok(JsValue::Undefined)
}

fn close_port(_realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    if let Some(port) = this.as_object() {
        port.set("__closed__", JsValue::Bool(true));
        port.set("__peer__", JsValue::Undefined);
    }
    Ok(JsValue::Undefined)
}

fn start_port(_realm: &mut Realm, _this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Undefined)
}

trait Truthy {
    fn is_truthy(&self) -> bool;
}

impl Truthy for JsValue {
    fn is_truthy(&self) -> bool {
        crate::vm::to_boolean(self)
    }
}
