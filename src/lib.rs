mod keyboard;
/// cbindgen:ignore
pub mod platform;
pub use platform::{
    clip_cursor, get_cursor, get_cursor_data, get_cursor_pos, get_focused_display,
    set_cursor_pos, start_os_service,
};
/// cbindgen:ignore
mod server;
pub use self::server::*;
mod client;
mod lan;
mod rendezvous_mediator;
pub use self::rendezvous_mediator::*;
/// cbindgen:ignore
pub mod common;
pub mod ipc;
#[cfg(not(any(feature = "cli", feature = "flutter")))]
pub mod ui;
mod version;
pub use version::*;
#[cfg(feature = "flutter")]
mod bridge_generated;
#[cfg(feature = "flutter")]
pub mod flutter;
#[cfg(feature = "flutter")]
pub mod flutter_ffi;
use common::*;
mod auth_2fa;
#[cfg(feature = "cli")]
pub mod cli;
mod clipboard;
#[cfg(not(feature = "cli"))]
pub mod core_main;
mod custom_server;
mod lang;
mod port_forward;

#[cfg(all(feature = "flutter", feature = "plugin_framework"))]
pub mod plugin;

mod tray;

mod whiteboard;

mod updater;

mod ui_cm_interface;
mod ui_interface;
mod ui_session_interface;

mod hbbs_http;

pub mod clipboard_file;

pub mod privacy_mode;

mod kcp_stream;
