//! rquickjs-shaped public handles.
//!
//! Internally the engine uses `JsValue` and `Object` directly. The handles in
//! this module add the lifetime relationship between a context and the values
//! created from it, so callers cannot keep a handle after its context is gone.

use std::marker::PhantomData;

use crate::object::PropKey;
use crate::runtime::{Context, Error, Result};
use crate::value::{FromJs, IntoJs, JsValue};

/// A borrowed context, equivalent to `rquickjs::Ctx<'js>`.
#[derive(Clone, Copy)]
pub struct Ctx<'js> {
    context: &'js Context,
    _marker: PhantomData<&'js ()>,
}

impl<'js> Ctx<'js> {
    pub(crate) fn new(context: &'js Context) -> Self {
        Self {
            context,
            _marker: PhantomData,
        }
    }

    pub fn eval<T: FromJs, S: AsRef<str>>(&self, source: S) -> Result<T> {
        let value = self.context.eval(source.as_ref())?;
        T::from_js(value).ok_or_else(|| Error::message("Unexpected JavaScript value type"))
    }

    pub fn eval_file<T: FromJs, P: AsRef<std::path::Path>>(&self, path: P) -> Result<T> {
        let value = self.context.eval_file(path)?;
        T::from_js(value).ok_or_else(|| Error::message("Unexpected JavaScript value type"))
    }

    pub fn globals(&self) -> Object<'js> {
        Object {
            raw: self.context.global(),
            ctx: *self,
        }
    }

    pub fn object(&self) -> Object<'js> {
        Object {
            raw: self.context.object(),
            ctx: *self,
        }
    }

    pub fn array(&self) -> Array<'js> {
        Array {
            object: Object {
                raw: self.context.array(std::iter::empty()),
                ctx: *self,
            },
        }
    }

    pub fn value<T: IntoJs>(&self, value: T) -> Value<'js> {
        Value {
            raw: value.into_js(),
            ctx: *self,
        }
    }

    pub fn eval_value<S: AsRef<str>>(&self, source: S) -> Result<Value<'js>> {
        Ok(Value {
            raw: self.context.eval(source.as_ref())?,
            ctx: *self,
        })
    }

    pub fn undefined(&self) -> Value<'js> {
        self.value(JsValue::Undefined)
    }

    pub fn null(&self) -> Value<'js> {
        self.value(JsValue::Null)
    }

    pub fn to_string(&self, value: &Value<'js>) -> Result<String> {
        self.context.to_string(&value.raw)
    }

    pub fn to_number(&self, value: &Value<'js>) -> Result<f64> {
        self.context.to_number(&value.raw)
    }

    /// Run one pending promise/await job, like
    /// `rquickjs::AsyncContext::execute_pending_job`.
    pub fn execute_pending_job(&self) -> Result<bool> {
        self.context.execute_pending_job()
    }

    pub fn is_job_pending(&self) -> bool {
        self.context.is_job_pending()
    }

    pub fn run_jobs(&self) -> Result<()> {
        self.context.run_jobs()
    }
}

/// A JavaScript value tied to the context that created it.
#[derive(Clone)]
pub struct Value<'js> {
    pub(crate) raw: JsValue,
    pub(crate) ctx: Ctx<'js>,
}

impl<'js> Value<'js> {
    pub fn is_undefined(&self) -> bool {
        self.raw.is_undefined()
    }

    pub fn is_null(&self) -> bool {
        self.raw.is_null()
    }

    pub fn is_nullish(&self) -> bool {
        self.raw.is_nullish()
    }

    pub fn is_bool(&self) -> bool {
        self.raw.is_bool()
    }

    pub fn is_number(&self) -> bool {
        self.raw.is_number()
    }

    pub fn is_string(&self) -> bool {
        self.raw.is_string()
    }

    pub fn is_object(&self) -> bool {
        self.raw.is_object()
    }

    pub fn is_function(&self) -> bool {
        self.raw.is_function()
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.raw {
            JsValue::Bool(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        self.raw.as_number()
    }

    pub fn as_string(&self) -> Option<String> {
        self.raw.as_string().map(|value| value.to_string())
    }

    pub fn as_object(&self) -> Option<Object<'js>> {
        self.raw
            .as_object()
            .map(|raw| Object { raw, ctx: self.ctx })
    }

    pub fn as_function(&self) -> Option<Function<'js>> {
        self.raw.as_function().map(|function| Function {
            object: Object {
                raw: function.as_object(),
                ctx: self.ctx,
            },
        })
    }

    pub fn into_raw(self) -> JsValue {
        self.raw
    }

    pub fn ctx(&self) -> Ctx<'js> {
        self.ctx
    }
}

impl<'js> std::fmt::Debug for Value<'js> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.raw.fmt(formatter)
    }
}

/// Object handle with the context attached, so property access can run
/// accessors and coercion like `rquickjs::Object<'js>`.
#[derive(Clone)]
pub struct Object<'js> {
    pub(crate) raw: crate::object::Object,
    pub(crate) ctx: Ctx<'js>,
}

impl<'js> Object<'js> {
    pub fn from_value(value: &Value<'js>) -> Option<Self> {
        value.as_object()
    }

    pub fn get<V: FromJs>(&self, key: &str) -> Result<V> {
        let mut realm = self.ctx.context.realm.borrow_mut();
        let value = crate::vm::get_value_property(
            &mut realm,
            &JsValue::Object(self.raw.to_ref()),
            &PropKey::from(key),
        )
        .map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })?;
        drop(realm);
        V::from_js(value).ok_or_else(|| Error::message("Unexpected JavaScript value type"))
    }

    pub fn set<V: IntoJs>(&self, key: &str, value: V) -> Result<()> {
        let mut realm = self.ctx.context.realm.borrow_mut();
        crate::vm::set_value_property(
            &mut realm,
            &JsValue::Object(self.raw.to_ref()),
            &PropKey::from(key),
            value.into_js(),
            true,
        )
        .map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })
    }

    pub fn is_array(&self) -> bool {
        self.raw.is_array()
    }

    pub fn is_function(&self) -> bool {
        self.raw.to_ref().borrow().is_function()
    }

    pub fn into_value(self) -> Value<'js> {
        Value {
            raw: JsValue::Object(self.raw.into_ref()),
            ctx: self.ctx,
        }
    }

    pub fn into_raw(self) -> crate::object::Object {
        self.raw
    }
}

/// Array handle.
#[derive(Clone)]
pub struct Array<'js> {
    object: Object<'js>,
}

impl<'js> Array<'js> {
    pub fn new(ctx: Ctx<'js>, items: impl IntoIterator<Item = JsValue>) -> Self {
        Self {
            object: Object {
                raw: ctx.context.array(items),
                ctx,
            },
        }
    }

    pub fn len(&self) -> usize {
        self.object.raw.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get<V: FromJs>(&self, index: usize) -> Result<V> {
        self.object.get(&index.to_string())
    }

    pub fn set<V: IntoJs>(&self, index: usize, value: V) -> Result<()> {
        self.object.set(&index.to_string(), value)
    }

    pub fn into_object(self) -> Object<'js> {
        self.object
    }

    pub fn into_value(self) -> Value<'js> {
        self.object.into_value()
    }
}

/// Function handle.
#[derive(Clone)]
pub struct Function<'js> {
    object: Object<'js>,
}

impl<'js> Function<'js> {
    pub fn from_value(value: &Value<'js>) -> Option<Self> {
        value.as_function()
    }

    pub fn call<R: FromJs>(&self, arguments: &[Value<'js>]) -> Result<R> {
        let arguments: Vec<JsValue> = arguments.iter().map(|value| value.raw.clone()).collect();
        let mut realm = self.object.ctx.context.realm.borrow_mut();
        let result = crate::vm::call_value(
            &mut realm,
            JsValue::Object(self.object.raw.to_ref()),
            JsValue::Undefined,
            &arguments,
        )
        .map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })?;
        realm.drain_jobs().map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })?;
        drop(realm);
        R::from_js(result).ok_or_else(|| Error::message("Unexpected JavaScript value type"))
    }

    pub fn construct<R: FromJs>(&self, arguments: &[Value<'js>]) -> Result<R> {
        let arguments: Vec<JsValue> = arguments.iter().map(|value| value.raw.clone()).collect();
        let mut realm = self.object.ctx.context.realm.borrow_mut();
        let result = crate::vm::construct_value(
            &mut realm,
            JsValue::Object(self.object.raw.to_ref()),
            &arguments,
        )
        .map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })?;
        realm.drain_jobs().map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })?;
        drop(realm);
        R::from_js(result).ok_or_else(|| Error::message("Unexpected JavaScript value type"))
    }

    pub fn into_object(self) -> Object<'js> {
        self.object
    }

    pub fn into_value(self) -> Value<'js> {
        self.object.into_value()
    }
}
