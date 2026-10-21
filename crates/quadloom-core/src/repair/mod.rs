//! Stage 0b: the repair ladder. Each step is a pure function over index buffers
//! and reports exactly what it changed (design §6.1: never repair silently).

pub mod degenerate;
pub mod weld;
