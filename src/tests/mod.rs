use fixture::Fixture;

mod client;
mod fixture;
mod server;
mod test_input_backend;

mod animations;
mod binds;
mod configure_state;
mod crash_regressions;
mod data_device;
#[cfg(feature = "dbus")]
mod display_config;
mod floating;
mod foreign_toplevel;
mod fullscreen;
mod idle_inhibit;
mod ipc;
mod layer_shell;
mod output_enter;
mod output_management;
mod pick;
mod pointer_inactivity;
mod pointer_redraws;
mod remove_output;
mod screenshot;
mod session_lock;
mod tiled_state;
mod transactions;
mod virtual_pointer;
mod window_cast;
mod window_opening;
