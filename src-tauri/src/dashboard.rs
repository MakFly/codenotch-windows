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
                use windows_sys::Win32::UI::{
                    Input::KeyboardAndMouse::{
                        keybd_event, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VK_MENU,
                    },
                    WindowsAndMessaging::{
                        BringWindowToTop, IsIconic, IsWindowVisible, SetForegroundWindow,
                        ShowWindow, SW_HIDE, SW_MINIMIZE, SW_RESTORE, SW_SHOW,
                    },
                };
                if let Ok(hwnd) = target.hwnd() {
                    unsafe {
                        match action {
                            // Every Show comes from the user (tray, notch, relaunch, first run),
                            // so the settings come to the front instead of opening behind.
                            Action::Show => {
                                if IsIconic(hwnd.0) != 0 {
                                    ShowWindow(hwnd.0, SW_RESTORE);
                                } else if IsWindowVisible(hwnd.0) == 0 {
                                    ShowWindow(hwnd.0, SW_SHOW);
                                }
                                if SetForegroundWindow(hwnd.0) == 0 {
                                    // Windows denies focus to a background process, e.g. after a
                                    // relaunch; a synthetic Alt press lifts that lock, as Tao does.
                                    keybd_event(VK_MENU as u8, 0, KEYEVENTF_EXTENDEDKEY, 0);
                                    keybd_event(
                                        VK_MENU as u8,
                                        0,
                                        KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP,
                                        0,
                                    );
                                    SetForegroundWindow(hwnd.0);
                                }
                                BringWindowToTop(hwnd.0);
                            }
                            Action::Hide => {
                                ShowWindow(hwnd.0, SW_HIDE);
                            }
                            Action::Minimize => {
                                ShowWindow(hwnd.0, SW_MINIMIZE);
                            }
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
