//! Structured model of a PlayMaker FSM ([`types`]) and the decoder that builds it ([`decode`]).

mod decode;
mod float;
mod types;

pub use decode::*;
pub use types::*;
