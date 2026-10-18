#![deny(unsafe_code)]

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

/// Cargo package version, e.g. `0.1.0`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Abbreviated git commit hash embedded at compile time. A dirty worktree
/// adds a `-modified` suffix; `unknown` is used outside a git checkout.
/// Tags are excluded so this is always a commit hash, not a `git describe`
/// tag description.
pub const GIT_COMMIT_HASH: &str = git_version::git_version!(
    args = [
        "--always",
        "--abbrev=12",
        "--dirty=-modified",
        "--exclude=*"
    ],
    fallback = "unknown"
);

/// Combined version string: `<cargo-version>+<commit>`.
pub const VERSION_STRING: &str = const_str::concat!(VERSION, "+", GIT_COMMIT_HASH);

/// Human-readable product string, e.g. `es6 0.1.0+7e7939305a98`.
pub const VERSION_INFO: &str =
    const_str::concat!(env!("CARGO_PKG_NAME"), " ", VERSION, "+", GIT_COMMIT_HASH);

mod api;
pub mod ast;
pub mod builtins;
pub mod bytecode;
mod coerce;
pub mod compiler;
pub mod gc;
pub mod lexer;
mod module;
mod modules;
pub mod object;
pub mod parser;
mod promise;
pub mod runtime;
mod string;
pub mod value;
pub mod vm;

pub use api::{Array, Ctx, Function, Object, Value};
pub use module::Module;
pub use object::Property;
pub use runtime::{Context, Error, Result, Runtime};
pub use string::JsString;
pub use value::{FromJs, IntoJs, JsValue};
