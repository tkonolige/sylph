#![feature(try_blocks)]

extern crate anyhow;
extern crate fuzzy_matcher;
extern crate itertools;
#[macro_use]
extern crate rusqlite;
extern crate binary_heap_plus;
extern crate crossbeam_channel;
extern crate serde;
extern crate sublime_fuzzy;
#[macro_use]
extern crate mlua_derive;

mod ffi;
mod matcher;
pub use crate::ffi::*;
pub use crate::matcher::*;
