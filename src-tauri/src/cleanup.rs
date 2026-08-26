use std::{sync::Arc, time::Duration};

use crate::{error::AppResult, host_runtime::HostRuntime as AppState, transfer};

const CLEANUP_INTERVAL_SECS: u64 = 120;

pub fn start_cleanup_scheduler(state: Arc<AppState>) {
    let scheduler_state = state.clone();
    state.spawn(async move {
        loop {
            if let Err(error) = run_cleanup_once(&scheduler_state).await {
                eprintln!("cleanup failed: {error}");
            }

            tokio::time::sleep(Duration::from_secs(CLEANUP_INTERVAL_SECS)).await;
        }
    });
}

pub async fn run_cleanup_once(state: &Arc<AppState>) -> AppResult<()> {
    let active_transfer_room_ids = transfer::active_transfer_room_ids(state);
    let expired_room_ids =
        crate::storage::cleanup_expired_rooms_except(&state.paths, &active_transfer_room_ids)?;

    for room_id in expired_room_ids {
        if let Err(error) = transfer::stop_room_server(state.clone(), &room_id).await {
            eprintln!("cleanup failed to stop room server for {room_id}: {error}");
        }
    }

    Ok(())
}
