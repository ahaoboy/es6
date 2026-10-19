//! ES module declaration and evaluation.
//!
//! The engine compiles a module body into a function whose first argument is
//! the exports namespace. Declaring a module parses and compiles it once;
//! evaluating it runs the compiled body. This mirrors the QuickJS split
//! between `JS_NewModuleDef`/`JS_AddModuleExport` (quickjs.c:4415) and
//! `JS_EvaluateModule` (quickjs.c:39564), while the namespace object plays the
//! role of the `JSModuleDef` bindings.
//!
//! ```text
//! let module = ctx.declare_module("rcm-kit", source)?;
//! let exports = module.eval()?;
//! let default: Value = exports.get_value("default")?;
//! ```

use std::rc::Rc;

use crate::api::{Ctx, Object};
use crate::bytecode::FunctionDef;
use crate::compiler;
use crate::parser;
use crate::runtime::{Error, Result};
use crate::value::{Env, JsValue};

/// A declared (parsed and compiled) ES module.
pub struct Module<'js> {
    ctx: Ctx<'js>,
    name: Rc<str>,
    source: Rc<str>,
    definition: Rc<FunctionDef>,
}

impl<'js> Module<'js> {
    /// `JS_NewModuleDef` (quickjs.c:4415): parse and compile a module.
    pub fn declare(ctx: Ctx<'js>, name: &str, source: &str) -> Result<Self> {
        let program = parser::parse(source).map_err(|error| {
            Error::syntax(error.message, error.offset, error.line, error.column)
        })?;
        let definition = compiler::compile_module(&program).map_err(|error| {
            if error.offset > 0 || error.line > 0 {
                Error::syntax(error.message, error.offset, error.line, error.column)
            } else {
                Error::message(error.to_string())
            }
        })?;
        Ok(Self {
            ctx,
            name: Rc::from(name),
            source: Rc::from(source),
            definition,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn context(&self) -> Ctx<'js> {
        self.ctx
    }

    /// `JS_EvaluateModule` (quickjs.c:39564): run the module body and return
    /// its exports namespace.
    pub fn eval(&self) -> Result<Object<'js>> {
        let context = self.ctx.context;
        let mut realm = context.realm.borrow_mut();
        let exports = realm.object();
        let function = realm.make_js_function(self.definition.clone(), Env::new(0, None), None);
        crate::vm::call_value(
            &mut realm,
            function,
            JsValue::Undefined,
            &[JsValue::Object(exports.clone())],
        )
        .map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            let mut error = Error::from_value(value, message);
            if let Some(offset) = crate::vm::take_error_offset() {
                let (line, column) = crate::runtime::line_column(&self.source, offset);
                error = error.with_position(offset, line, column);
            }
            error
        })?;
        realm.drain_jobs().map_err(|value| {
            let message = crate::vm::display_error(&realm, value.clone());
            Error::from_value(value, message)
        })?;
        Ok(Object::from_raw(
            self.ctx,
            crate::object::Object::from_ref(exports),
        ))
    }

    /// Evaluate the module and convert its namespace through [`FromJs`].
    pub fn eval_value<T: crate::value::FromJs>(&self) -> Result<T> {
        let exports = self.eval()?;
        let value = JsValue::Object(exports.into_raw().into_ref());
        T::from_js(value).ok_or_else(|| Error::message("Unexpected JavaScript value type"))
    }
}
