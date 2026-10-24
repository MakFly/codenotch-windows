use tauri::Manager;

#[derive(Clone, Copy)]
pub enum Action {
    Show,
    Hide,
    Minimize,
}

/// Keep every dashboard visibility operation on the same native path. Mixing
/// ShowWindow with Tauri hide() leaves Tao's cached visibility out of sync.
pub fn apply(app: &tauri::AppHandle, action: Action) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Fenêtre de réglages introuvable.")?;
    #[cfg(windows)]
    {
        let target = window.clone();
        window
            .run_on_main_thread(move || {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    IsIconic, IsWindowVisible, ShowWindow, SW_HIDE, SW_MINIMIZE, SW_SHOWNOACTIVATE,
                };
                if let Ok(hwnd) = target.hwnd() {
                    unsafe {
                        match action {
                            Action::Show
                                if IsWindowVisible(hwnd.0) == 0 || IsIconic(hwnd.0) != 0 =>
                            {
                                ShowWindow(hwnd.0, SW_SHOWNOACTIVATE);
                            }
                            Action::Hide => {
                                ShowWindow(hwnd.0, SW_HIDE);
                            }
                            Action::Minimize => {
                                ShowWindow(hwnd.0, SW_MINIMIZE);
                            }
                            Action::Show => {}
                        }
                    }
                }
            })
            .map_err(|error| error.to_string())
    }
    #[cfg(not(windows))]
    {
        match action {
            Action::Show => window.show().and_then(|_| window.unminimize()),
            Action::Hide => window.hide(),
            Action::Minimize => window.minimize(),
        }
        .map_err(|error| error.to_string())
    }
}
