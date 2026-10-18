use super::*;
use crate::value::JsValue;
use std::cmp::Ordering;

// QuickJS: JS_AddIntrinsicArray / js_array_* (quickjs.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.array_proto.clone();
    install_constructor(realm, "Array", 1, constructor, &prototype);
    define_method(realm, &prototype, "push", 1, push);
    define_method(realm, &prototype, "pop", 0, pop);
    define_method(realm, &prototype, "shift", 0, shift);
    define_method(realm, &prototype, "unshift", 1, unshift);
    define_method(realm, &prototype, "slice", 2, slice);
    define_method(realm, &prototype, "splice", 2, splice);
    define_method(realm, &prototype, "join", 1, join);
    define_method(realm, &prototype, "toString", 0, to_string);
    define_method(realm, &prototype, "concat", 1, concat);
    define_method(realm, &prototype, "indexOf", 1, index_of);
    define_method(realm, &prototype, "lastIndexOf", 1, last_index_of);
    define_method(realm, &prototype, "includes", 1, includes);
    define_method(realm, &prototype, "forEach", 1, for_each);
    define_method(realm, &prototype, "map", 1, map);
    define_method(realm, &prototype, "filter", 1, filter);
    define_method(realm, &prototype, "reduce", 1, reduce);
    define_method(realm, &prototype, "reduceRight", 1, reduce_right);
    define_method(realm, &prototype, "some", 1, some);
    define_method(realm, &prototype, "every", 1, every);
    define_method(realm, &prototype, "find", 1, find);
    define_method(realm, &prototype, "findIndex", 1, find_index);
    define_method(realm, &prototype, "reverse", 0, reverse);
    define_method(realm, &prototype, "sort", 1, sort);
    define_method(realm, &prototype, "fill", 1, fill);
    define_method(realm, &prototype, "copyWithin", 2, copy_within);
    define_method(realm, &prototype, "flat", 0, flat);
    define_method(realm, &prototype, "flatMap", 1, flat_map);
    define_method(realm, &prototype, "at", 1, at);
    define_method(realm, &prototype, "keys", 0, keys);
    define_method(realm, &prototype, "values", 0, values);
    define_method(realm, &prototype, "entries", 0, entries);
    let iterator = realm.well_known_symbol("iterator");
    define_method_symbol(realm, &prototype, iterator, "[Symbol.iterator]", 0, values);

    let constructor = realm.global_get("Array").as_object().unwrap();
    let target = constructor.to_ref();
    define_method(realm, &target, "isArray", 1, is_array);
    define_method(realm, &target, "from", 1, from);
    define_method(realm, &target, "of", 0, of);
}

fn constructor(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    if arguments.len() == 1
        && let Some(length) = arguments[0].as_number()
    {
        if length < 0.0 || length.fract() != 0.0 || length > 4_294_967_295.0 {
            return Err(realm.range_error("Invalid array length"));
        }
        return Ok(JsValue::Object(
            realm.array(vec![JsValue::Undefined; length as usize]),
        ));
    }
    Ok(JsValue::Object(realm.array(arguments.to_vec())))
}

fn is_array(_realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Bool(matches!(
        arg(arguments, 0),
        JsValue::Object(object) if object.borrow().is_array()
    )))
}

fn from(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let items = crate::vm::iterable_items(realm, arg(arguments, 0))?;
    let mapper = arg(arguments, 1);
    let this_arg = arg(arguments, 2);
    if mapper.is_function() {
        let mut mapped = Vec::with_capacity(items.len());
        for (index, item) in items.into_iter().enumerate() {
            mapped.push(super::call(
                realm,
                mapper.clone(),
                this_arg.clone(),
                &[item, JsValue::number(index as f64)],
            )?);
        }
        Ok(JsValue::Object(realm.array(mapped)))
    } else {
        Ok(JsValue::Object(realm.array(items)))
    }
}

fn of(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Object(realm.array(arguments.to_vec())))
}

fn push(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let mut items = array_items(realm, &object)?;
    items.extend_from_slice(arguments);
    let length = items.len();
    set_array_items(realm, &object, items);
    Ok(JsValue::number(length as f64))
}

fn pop(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let mut items = array_items(realm, &object)?;
    let value = items.pop().unwrap_or(JsValue::Undefined);
    set_array_items(realm, &object, items);
    Ok(value)
}

fn shift(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let mut items = array_items(realm, &object)?;
    let value = if items.is_empty() {
        JsValue::Undefined
    } else {
        items.remove(0)
    };
    set_array_items(realm, &object, items);
    Ok(value)
}

fn unshift(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let mut items = arguments.to_vec();
    items.extend(array_items(realm, &object)?);
    let length = items.len();
    set_array_items(realm, &object, items);
    Ok(JsValue::number(length as f64))
}

fn slice(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let length = items.len();
    let start = normalize_index(arg_number(realm, arguments, 0)?, length);
    let end = if arguments.get(1).is_some_and(|value| !value.is_undefined()) {
        normalize_index(arg_number(realm, arguments, 1)?, length)
    } else {
        length
    };
    let result = if start < end {
        items[start..end].to_vec()
    } else {
        Vec::new()
    };
    Ok(JsValue::Object(realm.array(result)))
}

fn splice(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let mut items = array_items(realm, &object)?;
    let length = items.len();
    let start = normalize_index(arg_number(realm, arguments, 0)?, length);
    // deleteCount defaults to `len - start` only with exactly one argument
    // (ES2022 23.1.3.26); with no arguments nothing is removed.
    let delete_count = if arguments.is_empty() {
        0
    } else if arguments.len() == 1 {
        length - start
    } else {
        (arg_number(realm, arguments, 1)?.max(0.0) as usize).min(length - start)
    };
    let removed: Vec<JsValue> = items
        .splice(
            start..start + delete_count,
            arguments.iter().skip(2).cloned(),
        )
        .collect();
    set_array_items(realm, &object, items);
    Ok(JsValue::Object(realm.array(removed)))
}

fn join(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let separator = if arg(arguments, 0).is_undefined() {
        ",".into()
    } else {
        crate::vm::to_string(realm, arg(arguments, 0))?
    };
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        parts.push(if item.is_nullish() {
            String::new()
        } else {
            crate::vm::to_string(realm, item)?
        });
    }
    Ok(JsValue::string(parts.join(&separator)))
}

fn to_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    join(realm, this, &[])
}

fn concat(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let mut items = array_items(realm, &object)?;
    for argument in arguments {
        if let JsValue::Object(value) = argument
            && value.borrow().is_array()
        {
            items.extend(array_items(realm, &Object::from_ref(value.clone()))?);
            continue;
        }
        items.push(argument.clone());
    }
    Ok(JsValue::Object(realm.array(items)))
}

fn index_of(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let target = arg(arguments, 0);
    let start = normalize_index(arg_number(realm, arguments, 1).unwrap_or(0.0), items.len());
    for (index, item) in items.iter().enumerate().skip(start) {
        if crate::vm::strict_equals(item, &target) {
            return Ok(JsValue::number(index as f64));
        }
    }
    Ok(JsValue::number(-1.0))
}

fn last_index_of(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let target = arg(arguments, 0);
    let start = if arguments.len() > 1 {
        normalize_index(arg_number(realm, arguments, 1)?, items.len())
    } else {
        items.len().saturating_sub(1)
    };
    for index in (0..=start).rev() {
        if let Some(item) = items.get(index)
            && crate::vm::strict_equals(item, &target)
        {
            return Ok(JsValue::number(index as f64));
        }
    }
    Ok(JsValue::number(-1.0))
}

fn includes(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let target = arg(arguments, 0);
    let start = normalize_index(arg_number(realm, arguments, 1).unwrap_or(0.0), items.len());
    for item in items.iter().skip(start) {
        if crate::vm::strict_equals(item, &target)
            || (item.is_number() && target.is_number() && {
                let a = item.as_number().unwrap();
                let b = target.as_number().unwrap();
                a.is_nan() && b.is_nan()
            })
        {
            return Ok(JsValue::Bool(true));
        }
    }
    Ok(JsValue::Bool(false))
}

fn for_each(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let mapper = arg(arguments, 0);
    let this_arg = arg(arguments, 1);
    let object = array_object(realm, this)?;
    for (index, item) in array_items(realm, &object)?.into_iter().enumerate() {
        super::call(
            realm,
            mapper.clone(),
            this_arg.clone(),
            &[item, JsValue::number(index as f64), this.clone()],
        )?;
    }
    Ok(JsValue::Undefined)
}

fn map(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let mapper = arg(arguments, 0);
    let this_arg = arg(arguments, 1);
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let mut result = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        result.push(super::call(
            realm,
            mapper.clone(),
            this_arg.clone(),
            &[item, JsValue::number(index as f64), this.clone()],
        )?);
    }
    Ok(JsValue::Object(realm.array(result)))
}

fn filter(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let callback = arg(arguments, 0);
    let this_arg = arg(arguments, 1);
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let mut result = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let keep = super::call(
            realm,
            callback.clone(),
            this_arg.clone(),
            &[item.clone(), JsValue::number(index as f64), this.clone()],
        )?;
        if crate::vm::to_boolean(&keep) {
            result.push(item);
        }
    }
    Ok(JsValue::Object(realm.array(result)))
}

fn reduce(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let callback = arg(arguments, 0);
    let mut index = 0;
    let mut accumulator = if arguments.len() > 1 {
        arg(arguments, 1)
    } else if let Some(first) = items.first() {
        index = 1;
        first.clone()
    } else {
        return Err(realm.type_error("Reduce of empty array with no initial value"));
    };
    while index < items.len() {
        accumulator = super::call(
            realm,
            callback.clone(),
            JsValue::Undefined,
            &[
                accumulator,
                items[index].clone(),
                JsValue::number(index as f64),
                this.clone(),
            ],
        )?;
        index += 1;
    }
    Ok(accumulator)
}

fn reduce_right(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let callback = arg(arguments, 0);
    let mut index = items.len() as isize - 1;
    let mut accumulator = if arguments.len() > 1 {
        arg(arguments, 1)
    } else if let Some(first) = items.last() {
        index -= 1;
        first.clone()
    } else {
        return Err(realm.type_error("Reduce of empty array with no initial value"));
    };
    while index >= 0 {
        accumulator = super::call(
            realm,
            callback.clone(),
            JsValue::Undefined,
            &[
                accumulator,
                items[index as usize].clone(),
                JsValue::number(index as f64),
                this.clone(),
            ],
        )?;
        index -= 1;
    }
    Ok(accumulator)
}

fn some(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let callback = arg(arguments, 0);
    let this_arg = arg(arguments, 1);
    let object = array_object(realm, this)?;
    for (index, item) in array_items(realm, &object)?.into_iter().enumerate() {
        let result = super::call(
            realm,
            callback.clone(),
            this_arg.clone(),
            &[item, JsValue::number(index as f64), this.clone()],
        )?;
        if crate::vm::to_boolean(&result) {
            return Ok(JsValue::Bool(true));
        }
    }
    Ok(JsValue::Bool(false))
}

fn every(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let callback = arg(arguments, 0);
    let this_arg = arg(arguments, 1);
    let object = array_object(realm, this)?;
    for (index, item) in array_items(realm, &object)?.into_iter().enumerate() {
        let result = super::call(
            realm,
            callback.clone(),
            this_arg.clone(),
            &[item, JsValue::number(index as f64), this.clone()],
        )?;
        if !crate::vm::to_boolean(&result) {
            return Ok(JsValue::Bool(false));
        }
    }
    Ok(JsValue::Bool(true))
}

fn find(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let callback = arg(arguments, 0);
    let this_arg = arg(arguments, 1);
    let object = array_object(realm, this)?;
    for (index, item) in array_items(realm, &object)?.into_iter().enumerate() {
        let result = super::call(
            realm,
            callback.clone(),
            this_arg.clone(),
            &[item.clone(), JsValue::number(index as f64), this.clone()],
        )?;
        if crate::vm::to_boolean(&result) {
            return Ok(item);
        }
    }
    Ok(JsValue::Undefined)
}

fn find_index(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let callback = arg(arguments, 0);
    let this_arg = arg(arguments, 1);
    let object = array_object(realm, this)?;
    for (index, item) in array_items(realm, &object)?.into_iter().enumerate() {
        let result = super::call(
            realm,
            callback.clone(),
            this_arg.clone(),
            &[item, JsValue::number(index as f64), this.clone()],
        )?;
        if crate::vm::to_boolean(&result) {
            return Ok(JsValue::number(index as f64));
        }
    }
    Ok(JsValue::number(-1.0))
}

fn reverse(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let mut items = array_items(realm, &object)?;
    items.reverse();
    set_array_items(realm, &object, items);
    Ok(this.clone())
}

fn sort(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let mut items = array_items(realm, &object)?;
    let comparator = arg(arguments, 0);
    // `undefined` elements never reach the comparator: they are removed
    // first and appended after sorting (ES2022 23.1.3.30).
    let undefined = items
        .iter()
        .filter(|item| item.is_undefined())
        .count();
    items.retain(|item| !item.is_undefined());
    if comparator.is_function() {
        let mut error: Option<JsValue> = None;
        items.sort_by(|left, right| {
            if error.is_some() {
                return Ordering::Equal;
            }
            match super::call(
                realm,
                comparator.clone(),
                JsValue::Undefined,
                &[left.clone(), right.clone()],
            ) {
                Ok(value) => {
                    let number = value.as_number().unwrap_or(f64::NAN);
                    if number.is_nan() {
                        Ordering::Equal
                    } else if number < 0.0 {
                        Ordering::Less
                    } else if number > 0.0 {
                        Ordering::Greater
                    } else {
                        Ordering::Equal
                    }
                }
                Err(value) => {
                    error = Some(value);
                    Ordering::Equal
                }
            }
        });
        if let Some(error) = error {
            return Err(error);
        }
    } else {
        // Default comparison uses UTF-16 code-unit order
        // (ES2022 7.2.13 IsLessThan).
        items.sort_by(|left, right| {
            let left = crate::vm::to_string(realm, left.clone()).unwrap_or_default();
            let right = crate::vm::to_string(realm, right.clone()).unwrap_or_default();
            crate::coerce::utf16_cmp(&left, &right)
        });
    }
    items.extend(std::iter::repeat(JsValue::Undefined).take(undefined));
    set_array_items(realm, &object, items);
    Ok(this.clone())
}

fn fill(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let mut items = array_items(realm, &object)?;
    let value = arg(arguments, 0);
    let start = normalize_index(arg_number(realm, arguments, 1).unwrap_or(0.0), items.len());
    let end = normalize_index(
        arg_number(realm, arguments, 2).unwrap_or(items.len() as f64),
        items.len(),
    );
    for item in &mut items[start..end] {
        *item = value.clone();
    }
    set_array_items(realm, &object, items);
    Ok(this.clone())
}

fn copy_within(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let mut items = array_items(realm, &object)?;
    let length = items.len();
    let target = normalize_index(arg_number(realm, arguments, 0)?, length);
    let start = normalize_index(arg_number(realm, arguments, 1)?, length);
    let end = normalize_index(arg_number(realm, arguments, 2).unwrap_or(length as f64), length);
    let count = (end.saturating_sub(start)).min(length.saturating_sub(target));
    let source: Vec<JsValue> = items[start..start + count].to_vec();
    items[target..target + count].clone_from_slice(&source);
    set_array_items(realm, &object, items);
    Ok(this.clone())
}

fn flatten(items: Vec<JsValue>, depth: usize) -> Vec<JsValue> {
    let mut result = Vec::new();
    for item in items {
        if depth > 0
            && let JsValue::Object(object) = &item
            && object.borrow().is_array()
        {
            let nested = object
                .borrow()
                .array_items()
                .map(|items| items.to_vec())
                .unwrap_or_default();
            result.extend(flatten(nested, depth - 1));
            continue;
        }
        result.push(item);
    }
    result
}

fn flat(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let depth = if arg(arguments, 0).is_undefined() {
        1
    } else {
        arg_number(realm, arguments, 0)?.max(0.0) as usize
    };
    let items = array_items(realm, &object)?;
    Ok(JsValue::Object(realm.array(flatten(items, depth))))
}

fn flat_map(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let callback = arg(arguments, 0);
    let this_arg = arg(arguments, 1);
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let mut result = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let value = super::call(
            realm,
            callback.clone(),
            this_arg.clone(),
            &[item, JsValue::number(index as f64), this.clone()],
        )?;
        result.push(value);
    }
    Ok(JsValue::Object(realm.array(flatten(result, 1))))
}

fn at(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let index = to_integer(arg_number(realm, arguments, 0)?);
    let index = if index < 0 {
        items.len() as i64 + index
    } else {
        index
    };
    Ok(if index >= 0 && (index as usize) < items.len() {
        items[index as usize].clone()
    } else {
        JsValue::Undefined
    })
}

fn keys(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let length = array_items(realm, &object)?.len();
    Ok(JsValue::Object(
        realm.iterator(
            (0..length)
                .map(|index| JsValue::number(index as f64))
                .collect(),
        ),
    ))
}

fn values(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    Ok(JsValue::Object(realm.iterator(items)))
}

fn entries(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let object = array_object(realm, this)?;
    let items = array_items(realm, &object)?;
    let entries = items
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            JsValue::Object(realm.array(vec![JsValue::number(index as f64), value]))
        })
        .collect();
    Ok(JsValue::Object(realm.iterator(entries)))
}
