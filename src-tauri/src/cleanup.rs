use std::{sync::Arc, time::Duration};

use tauri::{AppHandle, Manager};

use crate::{error::AppResult, transfer, AppState};

const CLEANUP_INTERVAL_SECS: u64 = 120;

pub fn start_cleanup_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(error) = run_cleanup_once(&app).await {
                eprintln!("cleanup failed: {error}");
            }

            tokio::time::sleep(Duration::from_secs(CLEANUP_INTERVAL_SECS)).await;
        }
    });
}

pub async fn run_cleanup_once(app: &AppHandle) -> AppResult<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let expired_room_ids = crate::storage::cleanup_expired_rooms(&state.paths)?;

    for room_id in expired_room_ids {
        if let Err(error) = transfer::stop_room_server(state.clone(), &room_id).await {
            eprintln!("cleanup failed to stop room server for {room_id}: {error}");
        }
    }

    Ok(())
}
