//! `require("console")`: the same console object installed globally.

use crate::runtime::Realm;
use crate::value::JsValue;

pub(crate) fn module(realm: &mut Realm) -> JsValue {
    realm.global_get("console")
}
