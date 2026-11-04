// QML parser: `lexer` tokenizes, `parser` builds the tree, `ast` defines it.
// JavaScript stays as source text for QuickJS (`js/`)

pub mod ast;
pub mod import;
pub mod lexer;
pub mod parser;

pub use parser::parse;
