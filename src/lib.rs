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
//! a [`Runtime`] owns a [`Context`], values are cheap handles and scripts are
//! evaluated with [`Context::eval`].

pub mod ast;
pub mod builtins;
pub mod bytecode;
pub mod compiler;
pub mod lexer;
pub mod object;
pub mod parser;
pub mod runtime;
pub mod value;
pub mod vm;

pub use object::{Array, Function, Object, Property};
pub use runtime::{Context, Error, Result, Runtime};
pub use value::{FromJs, IntoJs, JsValue};
