//! OS actions the engine needs but must not implement itself (the engine has no Win32
//! dependency). `codenotch-platform` provides the real implementation; the app injects it.

use std::path::Path;

/// Side effects on the desktop. Called from the engine thread; implementations must be
/// thread-safe and must not block for long.
pub trait SystemActions: Send + Sync {
    /// Open a URL in the default browser (https only; callers validate).
    fn open_url(&self, url: &str) -> crate::Result<()>;
    /// Open a folder or file in Explorer.
    fn open_path(&self, path: &Path) -> crate::Result<()>;
    /// Register / unregister "Open CodeNotch at login".
    fn set_launch_at_startup(&self, enabled: bool) -> crate::Result<()>;
    /// Whether the app is registered to start at login.
    fn launch_at_startup(&self) -> bool;
}

/// Does nothing; records nothing. For tests and headless modes.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSystem;

impl SystemActions for NoopSystem {
    fn open_url(&self, _url: &str) -> crate::Result<()> {
        Ok(())
    }
    fn open_path(&self, _path: &Path) -> crate::Result<()> {
        Ok(())
    }
    fn set_launch_at_startup(&self, _enabled: bool) -> crate::Result<()> {
        Ok(())
    }
    fn launch_at_startup(&self) -> bool {
        false
    }
}
