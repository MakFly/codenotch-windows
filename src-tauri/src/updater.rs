use serde::Serialize;
use std::time::Duration;
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;

/// The release found by the last check, kept until the user installs it.
#[derive(Default)]
pub struct Pending {
    update: Mutex<Option<Update>>,
    notified: Mutex<Option<String>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current: String,
    pub notes: Option<String>,
}

/// Asks the release feed for a newer signed build. The feed is GitHub's latest release;
/// the installer is only accepted if its signature matches the public key in tauri.conf.json.
pub async fn check(app: &tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    let update = app
        .updater()
        .map_err(|_| "Vérification des mises à jour indisponible.")?
        .check()
        .await
        .map_err(|_| {
            "Impossible de vérifier les mises à jour. Vérifie la connexion, puis réessaie."
        })?;
    let info = update.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        current: u.current_version.clone(),
        notes: u.body.clone().filter(|b| !b.trim().is_empty()),
    });
    let pending = app.state::<Pending>();
    *pending.update.lock().await = update;
    if let Some(info) = &info {
        let _ = app.emit("update-available", info);
    }
    Ok(info)
}

/// Downloads, verifies and runs the signed installer, then restarts on the new version.
pub async fn install(app: &tauri::AppHandle) -> Result<(), String> {
    let update = app
        .state::<Pending>()
        .update
        .lock()
        .await
        .take()
        .ok_or("Aucune mise à jour en attente. Vérifie à nouveau.")?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|_| "Échec du téléchargement ou de la vérification de la mise à jour.")?;
    app.restart()
}

/// Background checks: shortly after launch, then twice a day. A notification announces
/// each new version once; errors stay silent since the settings offer a manual check.
pub fn watch(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            if let Ok(Some(info)) = check(&app).await {
                let pending = app.state::<Pending>();
                let mut notified = pending.notified.lock().await;
                if notified.as_deref() != Some(&info.version) {
                    *notified = Some(info.version.clone());
                    let french = app
                        .state::<crate::Shared>()
                        .data
                        .lock()
                        .await
                        .config
                        .settings
                        .language
                        == "fr";
                    let (title, body) = if french {
                        (
                            format!("CodeNotch {} est disponible", info.version),
                            "Ouvre les réglages pour mettre à jour.".to_owned(),
                        )
                    } else {
                        (
                            format!("CodeNotch {} is available", info.version),
                            "Open Settings to update.".to_owned(),
                        )
                    };
                    let _ = app.notification().builder().title(title).body(body).show();
                }
            }
            tokio::time::sleep(Duration::from_secs(12 * 3600)).await;
        }
    });
}
