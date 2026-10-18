#![forbid(unsafe_code)]

//! A small, self-contained ECMAScript engine written in Rust.
//!
//! The implementation follows the same execution pipeline as QuickJS:
//!
//! ```text
//! source -> lexer -> parser -> bytecode compiler -> stack VM
//! ```
//!
//! The public facade intentionally mirrors the shape of `rquickjs`:
//! a [`Runtime`] owns a [`Context`], scripts are evaluated through
//! [`Ctx::eval`] inside [`Context::with`], and [`Value`], [`Object`],
//! [`Array`] and [`Function`] carry the context lifetime.
//!
//! Promise reactions and `async`/`await` continuations use the QuickJS-style
//! job queue in [`Realm`](runtime::Realm). [`Context::eval`] drains it before
//! returning; hosts driving their own event loop can use
//! [`Context::execute_pending_job`] and [`Context::run_jobs`].

mod api;
pub mod ast;
pub mod builtins;
pub mod bytecode;
mod coerce;
pub mod compiler;
pub mod gc;
pub mod lexer;
pub mod object;
pub mod parser;
mod promise;
pub mod runtime;
pub mod value;
pub mod vm;

pub use api::{Array, Ctx, Function, Object, Value};
pub use object::Property;
pub use runtime::{Context, Error, Result, Runtime};
pub use value::{FromJs, IntoJs, JsValue};
