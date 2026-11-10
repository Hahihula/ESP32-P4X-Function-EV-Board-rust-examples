//! App-level helpers for the demos: start-up (`demo`). Kept out of the
//! `esp32-p4-function-ev` library so a firmware using the board doesn't pull
//! in what only the demos need.
//!
//! Hardware-independent modules also build for a PC, where they are
//! unit-tested: `cargo test -p esp32-p4-function-ev-examples --lib --target
//! <host triple>`.

#![no_std]

#[cfg(target_arch = "riscv32")]
pub mod demo;
