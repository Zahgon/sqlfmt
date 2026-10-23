//! sqlfmt formats your dbt SQL files so you don't have to.
//!
//! The library lexes SQL (with embedded jinja) into lines of nodes, splits and merges
//! those lines to the configured line length, and renders the result. [`api`] holds the
//! entry points; [`cli`] is the `sqlfmt` command line. The other modules are internal.

mod analyzer;
pub mod api;
mod cache;
pub mod cli;
mod comment;
mod config;
mod difflib;
mod encoding;
pub mod error;
mod glob;
mod jinja_lexer;
mod jinjafmt;
mod line;
mod merger;
pub mod mode;
mod node;
mod node_manager;
mod operator_precedence;
pub mod pathutil;
mod pyint;
mod pyre;
mod pystr;
mod query;
mod query_formatter;
pub mod report;
mod rules;
mod segment;
mod splitter;
mod textwrap;
mod tokens;
