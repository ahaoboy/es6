use crate::gc::Gc;

use super::*;
use crate::ast::StmtKind;
use crate::value::Env;

// QuickJS: JS_AddIntrinsicFunction / js_function_proto_* (quickjs.c).
pub fn install(realm: &mut Realm) {
    let prototype = realm.function_proto.clone();
    install_constructor(realm, "Function", 1, constructor, &prototype);
    define_method(realm, &prototype, "call", 1, call);
    define_method(realm, &prototype, "apply", 2, apply);
    define_method(realm, &prototype, "bind", 1, bind);
    define_method(realm, &prototype, "toString", 0, to_string);
    define_method(realm, &prototype, "hasInstance", 1, has_instance);
}

fn constructor(realm: &mut Realm, _this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let mut parameters = Vec::new();
    for argument in arguments.iter().take(arguments.len().saturating_sub(1)) {
        parameters.push(crate::vm::to_string(realm, argument.clone())?);
    }
    let body = arguments
        .last()
        .map(|value| crate::vm::to_string(realm, value.clone()))
        .transpose()?
        .unwrap_or_default();
    let source = format!(
        "(function anonymous({}) {{{}}})",
        parameters.join(","),
        body
    );
    let program =
        crate::parser::parse(&source).map_err(|error| realm.syntax_error(error.to_string()))?;
    let function = program
        .body
        .into_iter()
        .find_map(|statement| match statement.kind {
            StmtKind::Expr(expression) => match expression.kind {
                crate::ast::ExprKind::Function(function) => Some(function),
                _ => None,
            },
            _ => None,
        })
        .ok_or_else(|| realm.syntax_error("Invalid Function body"))?;
    let definition = crate::compiler::compile_function_source(&function)
        .map_err(|error| realm.syntax_error(error.to_string()))?;
    Ok(realm.make_js_function(definition, Env::new(0, None), None))
}

fn call(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let target = this.clone();
    if !target.is_function() {
        return Err(realm.type_error("Function.prototype.call called on non-function"));
    }
    let call_this = arg(arguments, 0);
    let args = arguments.get(1..).unwrap_or(&[]);
    super::call(realm, target, call_this, args)
}

fn apply(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let target = this.clone();
    if !target.is_function() {
        return Err(realm.type_error("Function.prototype.apply called on non-function"));
    }
    let call_this = arg(arguments, 0);
    let array = arg(arguments, 1);
    let args = if array.is_nullish() {
        Vec::new()
    } else {
        crate::vm::iterable_items(realm, array)?
    };
    super::call(realm, target, call_this, &args)
}

fn bind(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    if !this.is_function() {
        return Err(realm.type_error("Function.prototype.bind called on non-function"));
    }
    let name = get(realm, this, "name")?;
    let name = if let JsValue::String(name) = name {
        format!("bound {name}")
    } else {
        "bound function".into()
    };
    let length = get(realm, this, "length")?;
    let length = length.as_number().unwrap_or(0.0) as u32;
    let prefix = arguments.get(1..).unwrap_or(&[]).to_vec();
    let length = length.saturating_sub(prefix.len() as u32);
    Ok(bound_function(
        realm,
        this.clone(),
        arg(arguments, 0),
        prefix,
        &name,
        length,
    ))
}

fn to_string(realm: &mut Realm, this: &JsValue, _arguments: &[JsValue]) -> JsResult<JsValue> {
    let Some(function) = this.as_object() else {
        return Err(realm.type_error("not a function"));
    };
    let data = function.into_ref();
    let data = data.borrow();
    let Some(function) = data.function() else {
        return Ok(JsValue::string("function () { [native code] }"));
    };
    if let Some(definition) = &function.def {
        return Ok(JsValue::string(format!(
            "function {}() {{ /* bytecode: {} ops */ }}",
            definition.name,
            definition.code.len()
        )));
    }
    Ok(JsValue::string(format!(
        "function {}() {{ [native code] }}",
        function.name
    )))
}

fn has_instance(realm: &mut Realm, this: &JsValue, arguments: &[JsValue]) -> JsResult<JsValue> {
    let constructor = this
        .as_object()
        .ok_or_else(|| realm.type_error("Not a function"))?;
    let prototype = get(realm, this, "prototype")?;
    let JsValue::Object(prototype) = prototype else {
        return Err(realm.type_error("prototype is not an object"));
    };
    let mut current = match arg(arguments, 0) {
        JsValue::Object(object) => object.borrow().prototype.clone(),
        _ => return Ok(JsValue::Bool(false)),
    };
    while let Some(object) = current {
        if Gc::ptr_eq(&object, &prototype) {
            return Ok(JsValue::Bool(true));
        }
        current = object.borrow().prototype.clone();
    }
    let _ = constructor;
    Ok(JsValue::Bool(false))
}
