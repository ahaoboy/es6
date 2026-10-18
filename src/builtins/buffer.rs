use std::rc::Rc;

use super::*;
use crate::object::{
    ArrayBufferData, ByteBuffer, DataViewData, ObjectKind, TypedArrayData, TypedArrayKind,
};
use crate::value::JsValue;

// QuickJS: JS_AddIntrinsicTypedArrays plus `js_array_buffer_constructor`
// (quickjs.c:59297), `js_array_buffer_slice` (59704),
// `js_typed_array_constructor` (1442), `js_typed_array_set` (60257) and
// `js_typed_array_subarray` (61210).
pub fn install(realm: &mut Realm) {
    install_array_buffer(realm);
    install_data_view(realm);
    install_typed_arrays(realm);
}

fn install_array_buffer(realm: &mut Realm) {
    let prototype = realm.array_buffer_proto.clone();
    let constructor = install_constructor(
        realm,
        "ArrayBuffer",
        1,
        array_buffer_constructor,
        &prototype,
    );
    let target = constructor.as_object().unwrap().to_ref();
    define_method(realm, &target, "isView", 1, is_view);
    define_method(realm, &prototype, "slice", 2, array_buffer_slice);
    define_accessor(realm, &prototype, "byteLength", array_buffer_length, None);
}

fn install_data_view(realm: &mut Realm) {
    let prototype = realm.data_view_proto.clone();
    install_constructor(realm, "DataView", 1, data_view_constructor, &prototype);
    define_accessor(realm, &prototype, "buffer", data_view_buffer, None);
    define_accessor(realm, &prototype, "byteOffset", data_view_offset, None);
    define_accessor(realm, &prototype, "byteLength", data_view_length, None);
    define_method(realm, &prototype, "getUint8", 1, get_u8);
    define_method(realm, &prototype, "getInt8", 1, get_i8);
    define_method(realm, &prototype, "getUint16", 2, get_u16);
    define_method(realm, &prototype, "getInt16", 2, get_i16);
    define_method(realm, &prototype, "getUint32", 2, get_u32);
    define_method(realm, &prototype, "getInt32", 2, get_i32);
    define_method(realm, &prototype, "getFloat32", 2, get_f32);
    define_method(realm, &prototype, "getFloat64", 2, get_f64);
    define_method(realm, &prototype, "setUint8", 2, set_u8);
    define_method(realm, &prototype, "setInt8", 2, set_i8);
    define_method(realm, &prototype, "setUint16", 3, set_u16);
    define_method(realm, &prototype, "setInt16", 3, set_i16);
    define_method(realm, &prototype, "setUint32", 3, set_u32);
    define_method(realm, &prototype, "setInt32", 3, set_i32);
    define_method(realm, &prototype, "setFloat32", 3, set_f32);
    define_method(realm, &prototype, "setFloat64", 3, set_f64);
}

type NativeConstructor = fn(&mut Realm, &JsValue, &[JsValue]) -> JsResult<JsValue>;

fn install_typed_arrays(realm: &mut Realm) {
    let kinds: [(TypedArrayKind, NativeConstructor); 9] = [
        (TypedArrayKind::Uint8, uint8_constructor),
        (TypedArrayKind::Uint8Clamped, uint8_clamped_constructor),
        (TypedArrayKind::Int8, int8_constructor),
        (TypedArrayKind::Uint16, uint16_constructor),
        (TypedArrayKind::Int16, int16_constructor),
        (TypedArrayKind::Uint32, uint32_constructor),
        (TypedArrayKind::Int32, int32_constructor),
        (TypedArrayKind::Float32, float32_constructor),
        (TypedArrayKind::Float64, float64_constructor),
    ];
    for (kind, constructor) in kinds {
        let prototype = realm.typed_array_protos.get(&kind).cloned().unwrap();
        let name = kind.name();
        let value = install_constructor(realm, name, 3, constructor, &prototype);
        define_value(
            &value.as_object().unwrap().to_ref(),
            "BYTES_PER_ELEMENT",
            JsValue::number(kind.bytes_per_element() as f64),
        );
        define_value(
            &prototype,
            "BYTES_PER_ELEMENT",
            JsValue::number(kind.bytes_per_element() as f64),
        );
        define_method(realm, &prototype, "set", 1, typed_set);
        define_method(realm, &prototype, "subarray", 2, typed_subarray);
        define_method(realm, &prototype, "slice", 2, typed_slice);
        define_method(realm, &prototype, "values", 0, typed_values);
        define_method(realm, &prototype, "keys", 0, typed_keys);
        define_method(realm, &prototype, "entries", 0, typed_entries);
        let iterator = realm.well_known_symbol("iterator");
        define_method_symbol(
            realm,
            &prototype,
            iterator,
            "[Symbol.iterator]",
            0,
            typed_values,
        );
        let constructor = value.as_object().unwrap().to_ref();
        define_method(realm, &constructor, "from", 1, typed_from);
        define_method(realm, &constructor, "of", 0, typed_of);
    }
}

fn array_buffer_constructor(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let length = arg_number(realm, arguments, 0).unwrap_or(0.0);
    if length < 0.0 || length.fract() != 0.0 || length > 4_294_967_295.0 {
        return Err(realm.range_error("Invalid array buffer length"));
    }
    Ok(JsValue::Object(realm.array_buffer(length as usize)))
}

fn is_view(_realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Bool(matches!(
        arg(arguments, 0),
        JsValue::Object(object)
            if matches!(
                object.borrow().kind,
                ObjectKind::DataView(_) | ObjectKind::TypedArray(_)
            )
    )))
}

fn array_buffer_bytes(
    realm: &Realm,
    value: &JsValue,
) -> JsResult<(crate::object::ObjRef, ByteBuffer)> {
    let object = value
        .as_object()
        .ok_or_else(|| realm.type_error("Expected an ArrayBuffer"))?;
    let reference = object.to_ref();
    let bytes = {
        let data = reference.borrow();
        match &data.kind {
            ObjectKind::ArrayBuffer(ArrayBufferData { bytes }) => Some(bytes.clone()),
            _ => None,
        }
    };
    bytes
        .map(|bytes| (reference, bytes))
        .ok_or_else(|| realm.type_error("Expected an ArrayBuffer"))
}

fn array_buffer_slice(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let (_, bytes) = array_buffer_bytes(realm, this)?;
    let bytes = bytes.borrow();
    let length = bytes.len();
    let start = normalize_index(arg_number(realm, arguments, 0).unwrap_or(0.0), length);
    let end = normalize_index(
        arg_number(realm, arguments, 1).unwrap_or(length as f64),
        length,
    );
    let end = end.max(start);
    Ok(JsValue::Object(
        realm.array_buffer_from(bytes[start..end].to_vec()),
    ))
}

fn array_buffer_length(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    let (_, bytes) = array_buffer_bytes(realm, this)?;
    Ok(JsValue::number(bytes.borrow().len() as f64))
}

fn data_view_of(realm: &Realm, value: &JsValue) -> JsResult<(crate::object::ObjRef, DataViewData)> {
    let object = value
        .as_object()
        .ok_or_else(|| realm.type_error("Expected a DataView"))?;
    let reference = object.to_ref();
    let data = {
        let data = reference.borrow();
        match &data.kind {
            ObjectKind::DataView(view) => Some(view.clone()),
            _ => None,
        }
    };
    data.map(|data| (reference, data))
        .ok_or_else(|| realm.type_error("Expected a DataView"))
}

fn data_view_constructor(
    realm: &mut Realm,
    _this: &JsValue,
    arguments: &[JsValue],
) -> JsResult<JsValue> {
    let (buffer, bytes) = array_buffer_bytes(realm, &arg(arguments, 0))?;
    let length = bytes.borrow().len();
    let offset = if arg(arguments, 1).is_undefined() {
        0.0
    } else {
        arg_number(realm, arguments, 1)?
    };
    if offset < 0.0 || offset.fract() != 0.0 || offset as usize > length {
        return Err(realm.range_error("Invalid DataView offset"));
    }
    let offset = offset as usize;
    let byte_length = if arg(arguments, 2).is_undefined() {
        length - offset
    } else {
        let value = arg_number(realm, arguments, 2)?;
        if value < 0.0 || value.fract() != 0.0 || offset + value as usize > length {
            return Err(realm.range_error("Invalid DataView length"));
        }
        value as usize
    };
    Ok(JsValue::Object(realm.data_view(
        buffer,
        bytes,
        offset,
        byte_length,
    )))
}

fn data_view_buffer(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    Ok(JsValue::Object(data_view_of(realm, this)?.1.buffer))
}

fn data_view_offset(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    Ok(JsValue::number(
        data_view_of(realm, this)?.1.byte_offset as f64,
    ))
}

fn data_view_length(
    realm: &mut Realm,
    this: &JsValue,
    _arguments: &[JsValue],
) -> JsResult<JsValue> {
    Ok(JsValue::number(
        data_view_of(realm, this)?.1.byte_length as f64,
    ))
}

fn read_bytes(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
    width: usize,
) -> JsResult<([u8; 8], bool)> {
    let (_, view) = data_view_of(realm, this)?;
    let index = arg_number(realm, arguments, 0)?;
    if index < 0.0 || index.fract() != 0.0 {
        return Err(realm.range_error("Invalid DataView offset"));
    }
    let index = index as usize;
    if index + width > view.byte_length {
        return Err(realm.range_error("Offset is outside the bounds of the DataView"));
    }
    let mut buffer = [0u8; 8];
    let bytes = view.bytes.borrow();
    let start = view.byte_offset + index;
    buffer[..width].copy_from_slice(&bytes[start..start + width]);
    let little_endian = arg(arguments, 1).is_truthy();
    if !little_endian {
        buffer[..width].reverse();
    }
    Ok((buffer, little_endian))
}

fn write_bytes(
    realm: &mut Realm,
    this: &JsValue,
    arguments: &[JsValue],
    width: usize,
    value: [u8; 8],
) -> JsResult<JsValue> {
    let (_, view) = data_view_of(realm, this)?;
    let index = arg_number(realm, arguments, 0)?;
    if index < 0.0 || index.fract() != 0.0 {
        return Err(realm.range_error("Invalid DataView offset"));
    }
    let index = index as usize;
    if index + width > view.byte_length {
        return Err(realm.range_error("Offset is outside the bounds of the DataView"));
    }
    let little_endian = arg(arguments, 2).is_truthy();
    let mut output = value;
    if !little_endian {
        output[..width].reverse();
    }
    let mut bytes = view.bytes.borrow_mut();
    let start = view.byte_offset + index;
    bytes[start..start + width].copy_from_slice(&output[..width]);
    Ok(JsValue::Undefined)
}

fn get_u8(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let (bytes, _) = read_bytes(realm, this, arguments, 1)?;
    Ok(JsValue::Int(bytes[0] as i32))
}
fn get_i8(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let (bytes, _) = read_bytes(realm, this, arguments, 1)?;
    Ok(JsValue::Int(bytes[0] as i8 as i32))
}
fn get_u16(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let (bytes, _) = read_bytes(realm, this, arguments, 2)?;
    Ok(JsValue::Int(u16::from_le_bytes([bytes[0], bytes[1]]) as i32))
}
fn get_i16(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let (bytes, _) = read_bytes(realm, this, arguments, 2)?;
    Ok(JsValue::Int(i16::from_le_bytes([bytes[0], bytes[1]]) as i32))
}
fn get_u32(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let (bytes, _) = read_bytes(realm, this, arguments, 4)?;
    Ok(JsValue::number(
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64,
    ))
}
fn get_i32(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let (bytes, _) = read_bytes(realm, this, arguments, 4)?;
    Ok(JsValue::Int(i32::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
    ])))
}
fn get_f32(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let (bytes, _) = read_bytes(realm, this, arguments, 4)?;
    Ok(JsValue::number(
        f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64,
    ))
}
fn get_f64(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let (bytes, _) = read_bytes(realm, this, arguments, 8)?;
    Ok(JsValue::number(f64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])))
}

fn set_u8(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_uint32(arg_number(realm, arguments, 1)?) as u8;
    write_bytes(realm, this, arguments, 1, [value; 8])
}
fn set_i8(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_int32(arg_number(realm, arguments, 1)?) as i8 as u8;
    write_bytes(realm, this, arguments, 1, [value; 8])
}
fn set_u16(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_uint32(arg_number(realm, arguments, 1)?) as u16;
    let mut bytes = [0; 8];
    bytes[..2].copy_from_slice(&value.to_le_bytes());
    write_bytes(realm, this, arguments, 2, bytes)
}
fn set_i16(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_int32(arg_number(realm, arguments, 1)?) as i16;
    let mut bytes = [0; 8];
    bytes[..2].copy_from_slice(&value.to_le_bytes());
    write_bytes(realm, this, arguments, 2, bytes)
}
fn set_u32(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_uint32(arg_number(realm, arguments, 1)?);
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&value.to_le_bytes());
    write_bytes(realm, this, arguments, 4, bytes)
}
fn set_i32(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = crate::vm::to_int32(arg_number(realm, arguments, 1)?);
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&value.to_le_bytes());
    write_bytes(realm, this, arguments, 4, bytes)
}
fn set_f32(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = arg_number(realm, arguments, 1)? as f32;
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&value.to_le_bytes());
    write_bytes(realm, this, arguments, 4, bytes)
}
fn set_f64(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let value = arg_number(realm, arguments, 1)?;
    write_bytes(realm, this, arguments, 8, value.to_le_bytes())
}

trait Truthy {
    fn is_truthy(&self) -> bool;
}
impl Truthy for JsValue {
    fn is_truthy(&self) -> bool {
        crate::vm::to_boolean(self)
    }
}

fn typed_array_of(realm: &Realm, value: &JsValue) -> JsResult<TypedArrayData> {
    let object = value
        .as_object()
        .ok_or_else(|| realm.type_error("Expected a typed array"))?;
    let reference = object.to_ref();
    let data = {
        let data = reference.borrow();
        match &data.kind {
            ObjectKind::TypedArray(data) => Some(data.clone()),
            _ => None,
        }
    };
    data.ok_or_else(|| realm.type_error("Expected a typed array"))
}

fn make_typed(realm: &mut Realm, kind: TypedArrayKind, arguments: &[JsValue]) -> JsResult<JsValue> {
    let bytes_per_element = kind.bytes_per_element();
    let first = arg(arguments, 0);
    if let JsValue::Object(object) = &first {
        let is_array_buffer = matches!(object.borrow().kind, ObjectKind::ArrayBuffer(_));
        if is_array_buffer {
            let (buffer, bytes) = array_buffer_bytes(realm, &first)?;
            let buffer_length = bytes.borrow().len();
            let offset = if arg(arguments, 1).is_undefined() {
                0.0
            } else {
                arg_number(realm, arguments, 1)?
            };
            if offset < 0.0 || offset.fract() != 0.0 || offset as usize > buffer_length {
                return Err(realm.range_error("Invalid typed array offset"));
            }
            let offset = offset as usize;
            let length = if arg(arguments, 2).is_undefined() {
                if !(buffer_length - offset).is_multiple_of(bytes_per_element) {
                    return Err(realm.range_error("Invalid typed array length"));
                }
                (buffer_length - offset) / bytes_per_element
            } else {
                let length = arg_number(realm, arguments, 2)?;
                if length < 0.0 || length.fract() != 0.0 {
                    return Err(realm.range_error("Invalid typed array length"));
                }
                length as usize
            };
            if offset + length * bytes_per_element > buffer_length {
                return Err(realm.range_error("Invalid typed array length"));
            }
            let data = TypedArrayData {
                kind,
                buffer,
                bytes,
                byte_offset: offset,
                length,
            };
            return Ok(JsValue::Object(realm.typed_array(data)));
        }
    }
    let values = if arg(arguments, 0).is_undefined() {
        Vec::new()
    } else if let Some(number) = first.as_number() {
        if number < 0.0 || number.fract() != 0.0 {
            return Err(realm.range_error("Invalid typed array length"));
        }
        vec![JsValue::Undefined; number as usize]
    } else {
        crate::vm::iterable_items(realm, first)?
    };
    let length = values.len();
    let byte_length = length * bytes_per_element;
    let buffer = realm.array_buffer(byte_length);
    let (_, bytes) = array_buffer_bytes(realm, &JsValue::Object(buffer.clone()))?;
    let data = TypedArrayData {
        kind,
        buffer,
        bytes,
        byte_offset: 0,
        length,
    };
    for (index, value) in values.into_iter().enumerate() {
        data.set(realm, index, &value);
    }
    Ok(JsValue::Object(realm.typed_array(data)))
}

macro_rules! typed_constructor {
    ($name:ident, $kind:ident) => {
        fn $name(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
            make_typed(realm, TypedArrayKind::$kind, arguments)
        }
    };
}

typed_constructor!(uint8_constructor, Uint8);
typed_constructor!(uint8_clamped_constructor, Uint8Clamped);
typed_constructor!(int8_constructor, Int8);
typed_constructor!(uint16_constructor, Uint16);
typed_constructor!(int16_constructor, Int16);
typed_constructor!(uint32_constructor, Uint32);
typed_constructor!(int32_constructor, Int32);
typed_constructor!(float32_constructor, Float32);
typed_constructor!(float64_constructor, Float64);

fn kind_from_constructor(realm: &mut Realm, this: &JsValue) -> JsResult<TypedArrayKind> {
    let prototype = get(realm, this, "prototype")?;
    let JsValue::Object(prototype) = prototype else {
        return Err(realm.type_error("Invalid typed array constructor"));
    };
    for (kind, candidate) in &realm.typed_array_protos {
        if Rc::ptr_eq(candidate, &prototype) {
            return Ok(*kind);
        }
    }
    Err(realm.type_error("Invalid typed array constructor"))
}

fn typed_from(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let kind = kind_from_constructor(realm, this)?;
    let values = crate::vm::iterable_items(realm, arg(arguments, 0))?;
    make_typed(realm, kind, &[JsValue::Object(realm.array(values))])
}

fn typed_of(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let kind = kind_from_constructor(realm, this)?;
    make_typed(
        realm,
        kind,
        &[JsValue::Object(realm.array(arguments.to_vec()))],
    )
}

fn typed_set(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let target = typed_array_of(realm, this)?;
    let values = crate::vm::iterable_items(realm, arg(arguments, 0))?;
    let offset = if arg(arguments, 1).is_undefined() {
        0.0
    } else {
        arg_number(realm, arguments, 1)?
    };
    if offset < 0.0 || offset.fract() != 0.0 {
        return Err(realm.range_error("Invalid typed array offset"));
    }
    let offset = offset as usize;
    if offset + values.len() > target.length {
        return Err(realm.range_error("Source is too large"));
    }
    for (index, value) in values.into_iter().enumerate() {
        target.set(realm, offset + index, &value);
    }
    Ok(JsValue::Undefined)
}

fn typed_subarray(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let target = typed_array_of(realm, this)?;
    let start = normalize_index(
        arg_number(realm, arguments, 0).unwrap_or(0.0),
        target.length,
    );
    let end = normalize_index(
        arg_number(realm, arguments, 1).unwrap_or(target.length as f64),
        target.length,
    );
    let end = end.max(start);
    let data = TypedArrayData {
        kind: target.kind,
        buffer: target.buffer,
        bytes: target.bytes,
        byte_offset: target.byte_offset + start * target.kind.bytes_per_element(),
        length: end - start,
    };
    Ok(JsValue::Object(realm.typed_array(data)))
}

fn typed_slice(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let target = typed_array_of(realm, this)?;
    let start = normalize_index(
        arg_number(realm, arguments, 0).unwrap_or(0.0),
        target.length,
    );
    let end = normalize_index(
        arg_number(realm, arguments, 1).unwrap_or(target.length as f64),
        target.length,
    );
    let end = end.max(start);
    make_typed(
        realm,
        target.kind,
        &[JsValue::Object(
            realm.array(target.values()[start..end].to_vec()),
        )],
    )
}

fn typed_values(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    Ok(JsValue::Object(
        realm.iterator(typed_array_of(realm, this)?.values()),
    ))
}

fn typed_keys(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let target = typed_array_of(realm, this)?;
    Ok(JsValue::Object(
        realm.iterator(
            (0..target.length)
                .map(|index| JsValue::number(index as f64))
                .collect(),
        ),
    ))
}

fn typed_entries(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let target = typed_array_of(realm, this)?;
    let entries = target
        .values()
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            JsValue::Object(realm.array(vec![JsValue::number(index as f64), value]))
        })
        .collect();
    Ok(JsValue::Object(realm.iterator(entries)))
}
