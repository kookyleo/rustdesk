#[cfg(quartz)]
extern crate block;
#[macro_use]
extern crate cfg_if;
pub use hbb_common::libc;

pub use common::*;

#[cfg(quartz)]
pub mod quartz;

mod common;
