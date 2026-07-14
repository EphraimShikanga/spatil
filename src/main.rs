mod backend;
mod config;
mod handlers;
mod input;
mod ipc;
mod renderer;
mod scene;
mod state;

use config::SpatilConfig;

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Spatil Compositor");

    // Ignoring SIGCHLD makes the kernel auto-reap children, so apps launched by
    // `exec` keybindings don't pile up as zombies.
    unsafe {
        libc::signal(libc::SIGCHLD, libc::SIG_IGN);
    }

    config::bootstrap_config_dir();
    let config = SpatilConfig::load();

    // Autostart entries and keybinding `exec`s inherit this so hooks find the
    // socket without hardcoding its name.
    unsafe {
        std::env::set_var("SPATIL_SOCKET", &config.general.socket_name);
    }

    backend::run(config);
}
