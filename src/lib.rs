#[macro_use]
extern crate tracing;

#[cfg(feature = "dbus")]
pub mod a11y;
pub mod animation;
pub mod backend;
pub mod cli;
pub(crate) mod collection_state_writer;
pub mod cursor;
pub mod cursor_shake;
#[cfg(feature = "dbus")]
pub mod dbus;
pub(crate) mod edge_triggers;
pub mod frame_clock;
pub(crate) mod frame_trace;
pub mod handlers;
pub mod input;
pub mod ipc;
mod launch;
mod launch_process;
pub mod layer;
pub mod layout;
pub mod niri;
pub mod protocols;
pub mod render_helpers;
pub mod rubber_band;
#[cfg(feature = "xdp-gnome-screencast")]
pub mod screencasting;
pub mod ui;
pub mod utils;
pub mod window;

#[cfg(test)]
mod tests;
