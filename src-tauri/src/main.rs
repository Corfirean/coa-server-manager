#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebKitGTK's DMA-BUF renderer ends the process with a Wayland "Protocol error" on some graphics stacks (seen with
    // an NVIDIA card on Wayland). The path without it works everywhere; a value set by the user is left alone.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    #[cfg(windows)]
    if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_none() {
        if let Some(dir) = std::env::var_os("COA_MANAGER_DATA_DIR") {
            let wv = std::path::PathBuf::from(dir).join("webview");
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", wv);
        }
    }
    coa_server_manager_lib::run()
}
