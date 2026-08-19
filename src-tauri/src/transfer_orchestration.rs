//! Shared Layer 3 transport-capacity admission.
//!
//! Layer 5 decides whether a managed Transfer step is semantically eligible.
//! This module sees only bounded transport resource facts and cannot create,
//! reorder, approve, or inspect Bridge Plan steps or private filesystem paths.

use std::{collections::HashMap, sync::Arc};

use parking_lot::Mutex;
use serde::Serialize;

use crate::error::{AppError, AppResult};

pub(crate) const GLOBAL_TRANSFER_WINDOW_BUDGET: usize = 8;
pub(crate) const ACTIVE_TRANSFER_SAFETY_CAP: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransferCapacityOrigin {
    Ordinary,
    Managed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransferCapacityRequest {
    pub(crate) transfer_id: String,
    pub(crate) room_id: String,
    pub(crate) size_bytes: u64,
    pub(crate) requested_window: usize,
    pub(crate) origin: TransferCapacityOrigin,
    pub(crate) diagnostic_override: bool,
}

#[derive(Default)]
pub(crate) struct TransferCapacityCoordinator {
    reservations: Mutex<HashMap<String, usize>>,
}

pub(crate) struct TransferCapacityLease {
    coordinator: Arc<TransferCapacityCoordinator>,
    transfer_id: String,
    effective_window: usize,
}

impl TransferCapacityCoordinator {
    pub(crate) fn admit(
        self: &Arc<Self>,
        request: TransferCapacityRequest,
    ) -> AppResult<TransferCapacityLease> {
        if request.transfer_id.is_empty()
            || request.transfer_id.len() > 256
            || request.room_id.is_empty()
            || request.room_id.len() > 128
        {
            return Err(AppError::InvalidInput(
                "Transfer capacity request is invalid.".into(),
            ));
        }
        let requested = if request.diagnostic_override {
            crate::transfer_tuning::clamp_transfer_window(request.requested_window)
        } else {
            request
                .requested_window
                .clamp(1, GLOBAL_TRANSFER_WINDOW_BUDGET)
        };
        let mut reservations = self.reservations.lock();
        if reservations.contains_key(&request.transfer_id) {
            return Err(AppError::InvalidInput(
                "Transfer already has transport capacity.".into(),
            ));
        }
        if reservations.len() >= ACTIVE_TRANSFER_SAFETY_CAP {
            return Err(AppError::InvalidInput(
                "Transfer capacity is temporarily unavailable.".into(),
            ));
        }
        let used = reservations.values().sum::<usize>();
        let available = GLOBAL_TRANSFER_WINDOW_BUDGET.saturating_sub(used);
        if available == 0 {
            return Err(AppError::InvalidInput(
                "Transfer capacity is temporarily unavailable.".into(),
            ));
        }
        let effective_window = if request.diagnostic_override {
            requested
        } else {
            requested.min(available)
        };
        reservations.insert(request.transfer_id.clone(), effective_window);
        drop(reservations);
        Ok(TransferCapacityLease {
            coordinator: self.clone(),
            transfer_id: request.transfer_id,
            effective_window,
        })
    }

    pub(crate) fn resize(&self, transfer_id: &str, requested_window: usize) -> Option<usize> {
        let mut reservations = self.reservations.lock();
        let current = *reservations.get(transfer_id)?;
        let used_without_current = reservations.values().sum::<usize>().saturating_sub(current);
        let available = GLOBAL_TRANSFER_WINDOW_BUDGET.saturating_sub(used_without_current);
        let effective = requested_window
            .clamp(1, GLOBAL_TRANSFER_WINDOW_BUDGET)
            .min(available.max(1));
        reservations.insert(transfer_id.into(), effective);
        Some(effective)
    }
}

impl TransferCapacityLease {
    pub(crate) fn effective_window(&self) -> usize {
        self.effective_window
    }
}

impl Drop for TransferCapacityLease {
    fn drop(&mut self) {
        self.coordinator
            .reservations
            .lock()
            .remove(&self.transfer_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: &str, origin: TransferCapacityOrigin, window: usize) -> TransferCapacityRequest {
        TransferCapacityRequest {
            transfer_id: id.into(),
            room_id: "room".into(),
            size_bytes: 1024,
            requested_window: window,
            origin,
            diagnostic_override: false,
        }
    }

    #[test]
    fn ordinary_and_managed_transfers_share_one_capacity_budget() {
        let coordinator = Arc::new(TransferCapacityCoordinator::default());
        let ordinary = coordinator
            .admit(request("ordinary", TransferCapacityOrigin::Ordinary, 7))
            .unwrap();
        let managed = coordinator
            .admit(request("managed", TransferCapacityOrigin::Managed, 7))
            .unwrap();

        assert_eq!(ordinary.effective_window(), 7);
        assert_eq!(managed.effective_window(), 1);
        assert!(coordinator
            .admit(request("third", TransferCapacityOrigin::Ordinary, 1))
            .is_err());
        drop(ordinary);
        assert_eq!(
            coordinator
                .admit(request("third", TransferCapacityOrigin::Ordinary, 1))
                .unwrap()
                .effective_window(),
            1
        );
    }

    #[test]
    fn capacity_request_contains_only_bounded_resource_facts() {
        let encoded = serde_json::to_value(request(
            "opaque-transfer",
            TransferCapacityOrigin::Managed,
            8,
        ))
        .unwrap();
        let object = encoded.as_object().unwrap();
        assert_eq!(
            object
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            [
                "diagnosticOverride",
                "origin",
                "requestedWindow",
                "roomId",
                "sizeBytes",
                "transferId"
            ]
            .into_iter()
            .map(String::from)
            .collect()
        );
        for forbidden in ["path", "objectRef", "plan", "approval", "step"] {
            assert!(!encoded.to_string().contains(forbidden));
        }
    }

    #[test]
    fn resizing_cannot_exceed_the_shared_budget() {
        let coordinator = Arc::new(TransferCapacityCoordinator::default());
        let _ordinary = coordinator
            .admit(request("ordinary", TransferCapacityOrigin::Ordinary, 6))
            .unwrap();
        let _managed = coordinator
            .admit(request("managed", TransferCapacityOrigin::Managed, 2))
            .unwrap();

        assert_eq!(coordinator.resize("managed", 8), Some(2));
        assert_eq!(coordinator.resize("ordinary", 1), Some(1));
        assert_eq!(coordinator.resize("managed", 8), Some(7));
    }

    #[test]
    fn explicit_diagnostic_override_keeps_its_existing_window_precedence() {
        let coordinator = Arc::new(TransferCapacityCoordinator::default());
        let mut override_request = request("diagnostic", TransferCapacityOrigin::Ordinary, 16);
        override_request.diagnostic_override = true;
        assert_eq!(
            coordinator
                .admit(override_request)
                .unwrap()
                .effective_window(),
            16
        );
    }
}
