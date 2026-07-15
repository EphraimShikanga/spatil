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
    // The DRM backend blanks the screen, so a panic on stderr is invisible on a TTY.
    // Route it through tracing to land in the same redirectable log stream
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("PANIC: {info}");
        default_hook(info);
    }));
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
