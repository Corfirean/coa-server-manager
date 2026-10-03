#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebKitGTK's DMA-BUF renderer ends the process with a Wayland "Protocol error" on some graphics stacks (seen with
    // an NVIDIA card on Wayland). The path without it works everywhere; a value set by the user is left alone.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    coa_server_manager_lib::run()
}
