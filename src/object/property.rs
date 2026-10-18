//! Property access on `ObjectData`: prototype-chain gets, generic puts,
//! array element writes and enumeration, mirroring
//! `JS_GetPropertyInternal` (quickjs.c:9110) and `JS_SetPropertyInternal2`
//! (quickjs.c:10551).

use super::*;

impl ObjectData {
    pub fn get_property(
        &self,
        realm: &mut Realm,
        key: &PropKey,
        receiver: &JsValue,
    ) -> JsResult<JsValue> {
        if let Some(value) = self.get_array_value(key) {
            return Ok(value);
        }
        if let Some(property) = self.property(key) {
            if property.is_accessor() {
                return match property.get.as_deref() {
                    Some(getter) => {
                        crate::vm::call_value(realm, getter.clone(), receiver.clone(), &[])
                    }
                    None => Ok(JsValue::Undefined),
                };
            }
            return Ok(property.value.clone());
        }
        match &self.prototype {
            Some(proto) => proto.borrow().get_property(realm, key, receiver),
            None => Ok(JsValue::Undefined),
        }
    }

    pub fn put_property(
        &mut self,
        realm: &mut Realm,
        key: PropKey,
        value: JsValue,
        receiver: &JsValue,
    ) -> JsResult<()> {
        if let Some(result) = self.put_array_value(realm, &key, &value)? {
            return Ok(result);
        }
        if let Some(slot) = self.lookup(&key) {
            let property = &self.props[slot as usize];
            if property.is_accessor() {
                let setter = property.set.as_deref().cloned();
                return match setter {
                    Some(setter) => {
                        crate::vm::call_value(realm, setter, receiver.clone(), &[value])?;
                        Ok(())
                    }
                    None => Ok(()),
                };
            }
            if !property.writable {
                return Ok(());
            }
            self.props[slot as usize].value = value;
            return Ok(());
        }
        if let Some(proto) = &self.prototype {
            let mut current = Some(proto.clone());
            while let Some(proto) = current {
                let cursor = proto.borrow();
                if let Some(property) = cursor.property(&key) {
                    if property.is_accessor() {
                        return match property.set.as_deref() {
                            Some(setter) => {
                                crate::vm::call_value(
                                    realm,
                                    setter.clone(),
                                    receiver.clone(),
                                    &[value],
                                )?;
                                Ok(())
                            }
                            None => Ok(()),
                        };
                    }
                    if !property.writable {
                        return Ok(());
                    }
                    break;
                }
                current = cursor.prototype.clone();
            }
        }
        if self.extensible {
            self.define(key, Property::data(value));
        }
        Ok(())
    }

    pub fn put_array_value(
        &mut self,
        realm: &mut Realm,
        key: &PropKey,
        value: &JsValue,
    ) -> JsResult<Option<()>> {
        if let PropKey::Index(index) = key {
            match &self.kind {
                ObjectKind::TypedArray(data) => {
                    if *index < data.length {
                        data.set(realm, *index, value);
                        return Ok(Some(()));
                    }
                    return Ok(None);
                }
                ObjectKind::Array(data) => {
                    let index = *index;
                    if index < data.items.len() {
                        if let ObjectKind::Array(data) = &mut self.kind {
                            data.items[index] = Some(value.clone());
                        }
                        return Ok(Some(()));
                    }
                    if index < crate::coerce::DENSE_LIMIT {
                        if let ObjectKind::Array(data) = &mut self.kind {
                            data.items.resize(index + 1, None);
                            data.items[index] = Some(value.clone());
                            data.length = data.length.max(index + 1);
                        }
                        return Ok(Some(()));
                    }
                    // Beyond the dense limit only the length is recorded;
                    // the value itself lands in the sparse property map.
                    if let ObjectKind::Array(data) = &mut self.kind {
                        data.length = data.length.max(index + 1);
                    }
                    return Ok(None);
                }
                _ => {}
            }
            return Ok(None);
        }
        let PropKey::Str(text) = key else {
            return Ok(None);
        };
        if let ObjectKind::TypedArray(data) = &self.kind
            && let Some(index) = key_to_index(text)
            && index < data.length
        {
            data.set(realm, index, value);
            return Ok(Some(()));
        }
        if let ObjectKind::Array(data) = &mut self.kind {
            if text.as_ref() == "length" {
                let length = crate::vm::to_number(realm, value.clone())?;
                if length.fract() != 0.0
                    || length.is_nan()
                    || length < 0.0
                    || length > 4_294_967_295.0
                {
                    return Err(crate::vm::range_error(realm, "Invalid array length"));
                }
                let length = length as usize;
                if length < data.items.len() {
                    data.items.truncate(length);
                } else if length - data.items.len() < crate::coerce::DENSE_LIMIT {
                    // Grow the dense prefix; huge lengths only record holes,
                    // like `set_array_length` (quickjs.c:10360).
                    data.items.resize(length, None);
                }
                data.length = length;
                return Ok(Some(()));
            }
            if let Some(index) = key_to_index(text) {
                if index < data.items.len() {
                    data.items[index] = Some(value.clone());
                    data.length = data.length.max(index + 1);
                    return Ok(Some(()));
                }
                if index < crate::coerce::DENSE_LIMIT {
                    data.items.resize(index + 1, None);
                    data.items[index] = Some(value.clone());
                    data.length = data.length.max(index + 1);
                    return Ok(Some(()));
                }
                // Beyond the dense limit the element stays in the sparse
                // property map (below), recording only the length.
                data.length = data.length.max(index + 1);
            }
        }
        Ok(None)
    }

    pub fn delete_property(&mut self, key: &PropKey) -> bool {
        if let ObjectKind::Array(data) = &mut self.kind
            && let Some(index) = key.index()
            && index < data.items.len()
        {
            // The slot becomes a hole: not an own property, exactly like the
            // fast-to-slow array conversion QuickJS performs on delete.
            data.items[index] = None;
            return true;
        }
        self.remove(key)
    }

    /// Own keys in spec order: integer indices ascending, then string keys
    /// in insertion order, then symbols — `OrdinaryOwnPropertyKeys`
    /// (ES2022 10.1.12), like QuickJS `__JS_GetOwnPropertyNamesInternal`
    /// (quickjs.c:9515).
    pub fn own_keys(&self) -> Vec<PropKey> {
        self.keys_by(false)
    }

    pub fn enumerable_keys(&self) -> Vec<PropKey> {
        self.keys_by(true)
    }

    fn keys_by(&self, enumerable_only: bool) -> Vec<PropKey> {
        let mut indices: Vec<usize> = Vec::new();
        let mut keys = Vec::new();
        // Virtual keys of exotic objects are never enumerable
        // (ES2022 10.4.2.3 and friends).
        let include_virtual = !enumerable_only;
        match &self.kind {
            ObjectKind::Array(data) => {
                // Only present dense elements are own properties; holes are
                // skipped like a slow QuickJS array.
                for (index, item) in data.items.iter().enumerate() {
                    if item.is_some() {
                        indices.push(index);
                    }
                }
                if include_virtual {
                    keys.push(PropKey::from("length"));
                }
            }
            ObjectKind::TypedArray(data) => {
                indices.extend(0..data.length);
                if include_virtual {
                    keys.push(PropKey::from("length"));
                    keys.push(PropKey::from("byteLength"));
                    keys.push(PropKey::from("byteOffset"));
                }
            }
            ObjectKind::BoxedString(value) => {
                indices.extend(0..utf16_len(value));
                if include_virtual {
                    keys.push(PropKey::from("length"));
                }
            }
            _ => {}
        }
        for (key, property) in self.iter() {
            if enumerable_only && !property.enumerable {
                continue;
            }
            match key {
                PropKey::Index(index) => indices.push(*index),
                _ => keys.push(key.clone()),
            }
        }
        indices.sort_unstable();
        indices.dedup();
        let mut result: Vec<PropKey> = indices.into_iter().map(PropKey::Index).collect();
        result.extend(keys);
        result
    }

    pub fn proto(&self) -> Option<Object> {
        self.prototype
            .as_ref()
            .map(|proto| Object::from_ref(proto.clone()))
    }

    pub fn into_handle(self) -> Object {
        Object::from_ref(Rc::new(RefCell::new(self)))
    }

    pub fn set_proto(&mut self, proto: Option<ObjRef>) {
        self.touch();
        if let Some(proto) = &proto {
            proto.borrow_mut().is_prototype = true;
        }
        PROTOTYPE_GENERATION.fetch_add(1, Ordering::Relaxed);
        self.prototype = proto;
    }
}
