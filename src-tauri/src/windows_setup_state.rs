//! Pure setup-state validation for the Windows restricted-principal backend.
//!
//! This module is compiled by non-Windows unit tests so the fail-closed setup
//! transaction can be exercised without pretending to provide native Windows
//! confinement evidence.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecoveryStageV1 {
    AccountPending,
    LegacyPasswordRotationPending,
    Bound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LegacyPrivilegeClassV1 {
    Guest,
    User,
    Administrator,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryEvidenceV1 {
    pub(crate) stage: RecoveryStageV1,
    pub(crate) account_sid: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SetupEvidenceV1 {
    pub(crate) final_account_sid: Option<String>,
    pub(crate) recovery: Option<RecoveryEvidenceV1>,
    pub(crate) local_account_sid: Option<String>,
    pub(crate) legacy_fingerprint_matches: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SetupPlanV1 {
    Fresh,
    Repeat { account_sid: String },
    ResumeCreate,
    ResumeAuthenticate { account_sid: String },
    ResumeLegacyRotation { account_sid: String },
    BeginLegacyRecovery { account_sid: String },
}

pub(crate) fn validate_exact_local_user_identity(
    requested_name: &str,
    returned_name: &str,
    valid_sid: bool,
    legacy_privilege: LegacyPrivilegeClassV1,
    exact_managed_flags: bool,
    operator_authority_clear: bool,
    local_group_membership_count: u32,
) -> Result<(), &'static str> {
    if !requested_name.eq_ignore_ascii_case(returned_name) {
        return Err("local account lookup returned a different account name");
    }
    if !valid_sid {
        return Err("local account lookup returned an invalid SID");
    }
    if !matches!(
        legacy_privilege,
        LegacyPrivilegeClassV1::Guest | LegacyPrivilegeClassV1::User
    ) {
        return Err("local account lookup reported an administrator or unknown privilege class");
    }
    if !exact_managed_flags {
        return Err("local account flags do not match the managed sandbox account shape");
    }
    if !operator_authority_clear {
        return Err("local account has operator authority");
    }
    if local_group_membership_count != 0 {
        return Err("local account has unexpected direct or indirect local-group membership");
    }
    Ok(())
}

pub(crate) fn select_setup_plan(evidence: SetupEvidenceV1) -> Result<SetupPlanV1, &'static str> {
    if let Some(final_sid) = evidence.final_account_sid {
        let local_sid = evidence
            .local_account_sid
            .ok_or("the finalized sandbox account is missing")?;
        if final_sid != local_sid {
            return Err("the finalized sandbox account SID was replaced");
        }
        if let Some(recovery) = evidence.recovery {
            if recovery.stage != RecoveryStageV1::Bound
                || recovery.account_sid.as_deref() != Some(local_sid.as_str())
            {
                return Err("provisional and finalized setup state disagree");
            }
        }
        return Ok(SetupPlanV1::Repeat {
            account_sid: local_sid,
        });
    }

    match (evidence.recovery, evidence.local_account_sid) {
        (None, None) => Ok(SetupPlanV1::Fresh),
        (None, Some(account_sid)) if evidence.legacy_fingerprint_matches => {
            Ok(SetupPlanV1::BeginLegacyRecovery { account_sid })
        }
        (None, Some(_)) => Err("an unrelated pre-existing sandbox account was found"),
        (Some(recovery), None) => match recovery.stage {
            RecoveryStageV1::AccountPending if recovery.account_sid.is_none() => {
                Ok(SetupPlanV1::ResumeCreate)
            }
            _ => Err("provisional setup refers to a missing sandbox account"),
        },
        (Some(recovery), Some(local_sid)) => match recovery.stage {
            RecoveryStageV1::AccountPending if recovery.account_sid.is_none() => {
                Ok(SetupPlanV1::ResumeAuthenticate {
                    account_sid: local_sid,
                })
            }
            RecoveryStageV1::LegacyPasswordRotationPending
                if recovery.account_sid.as_deref() == Some(local_sid.as_str())
                    && evidence.legacy_fingerprint_matches =>
            {
                Ok(SetupPlanV1::ResumeLegacyRotation {
                    account_sid: local_sid,
                })
            }
            RecoveryStageV1::Bound
                if recovery.account_sid.as_deref() == Some(local_sid.as_str()) =>
            {
                Ok(SetupPlanV1::ResumeAuthenticate {
                    account_sid: local_sid,
                })
            }
            _ => Err("provisional setup does not authenticate the current sandbox account"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> SetupEvidenceV1 {
        SetupEvidenceV1 {
            final_account_sid: None,
            recovery: None,
            local_account_sid: None,
            legacy_fingerprint_matches: false,
        }
    }

    #[test]
    fn expected_non_privileged_local_account_without_users_membership_is_accepted() {
        assert_eq!(
            validate_exact_local_user_identity(
                "PasteySandboxOffline",
                "PasteySandboxOffline",
                true,
                LegacyPrivilegeClassV1::Guest,
                true,
                true,
                0,
            ),
            Ok(())
        );
        assert_eq!(
            validate_exact_local_user_identity(
                "PasteySandboxOffline",
                "PasteySandboxOffline",
                true,
                LegacyPrivilegeClassV1::User,
                true,
                true,
                0,
            ),
            Ok(())
        );
    }

    #[test]
    fn administrator_operator_and_group_authority_are_rejected() {
        assert!(validate_exact_local_user_identity(
            "PasteySandboxOffline",
            "PasteySandboxOffline",
            true,
            LegacyPrivilegeClassV1::Administrator,
            true,
            true,
            0,
        )
        .is_err());
        assert!(validate_exact_local_user_identity(
            "PasteySandboxOffline",
            "PasteySandboxOffline",
            true,
            LegacyPrivilegeClassV1::Unknown,
            true,
            true,
            0,
        )
        .is_err());
        assert!(validate_exact_local_user_identity(
            "PasteySandboxOffline",
            "PasteySandboxOffline",
            true,
            LegacyPrivilegeClassV1::Guest,
            true,
            false,
            0,
        )
        .is_err());
        assert!(validate_exact_local_user_identity(
            "PasteySandboxOffline",
            "PasteySandboxOffline",
            true,
            LegacyPrivilegeClassV1::Guest,
            true,
            true,
            1,
        )
        .is_err());
    }

    #[test]
    fn changed_name_invalid_sid_and_changed_flags_are_rejected() {
        assert!(validate_exact_local_user_identity(
            "PasteySandboxOffline",
            "DOMAIN\\PasteySandboxOffline",
            true,
            LegacyPrivilegeClassV1::Guest,
            true,
            true,
            0,
        )
        .is_err());
        assert!(validate_exact_local_user_identity(
            "PasteySandboxOffline",
            "PasteySandboxOffline",
            false,
            LegacyPrivilegeClassV1::Guest,
            true,
            true,
            0,
        )
        .is_err());
        assert!(validate_exact_local_user_identity(
            "PasteySandboxOffline",
            "PasteySandboxOffline",
            true,
            LegacyPrivilegeClassV1::Guest,
            false,
            true,
            0,
        )
        .is_err());
    }

    #[test]
    fn fresh_setup_has_no_prior_authority() {
        assert_eq!(select_setup_plan(evidence()), Ok(SetupPlanV1::Fresh));
    }

    #[test]
    fn repeated_setup_requires_the_same_final_sid() {
        let mut value = evidence();
        value.final_account_sid = Some("S-1-5-21-1-1001".into());
        value.local_account_sid = Some("S-1-5-21-1-1001".into());
        assert_eq!(
            select_setup_plan(value),
            Ok(SetupPlanV1::Repeat {
                account_sid: "S-1-5-21-1-1001".into()
            })
        );

        let mut after_final_commit = evidence();
        after_final_commit.final_account_sid = Some("S-1-5-21-1-1001".into());
        after_final_commit.local_account_sid = Some("S-1-5-21-1-1001".into());
        after_final_commit.recovery = Some(RecoveryEvidenceV1 {
            stage: RecoveryStageV1::Bound,
            account_sid: Some("S-1-5-21-1-1001".into()),
        });
        assert!(matches!(
            select_setup_plan(after_final_commit),
            Ok(SetupPlanV1::Repeat { .. })
        ));
    }

    #[test]
    fn interrupted_setup_resumes_before_or_after_account_creation() {
        let mut before = evidence();
        before.recovery = Some(RecoveryEvidenceV1 {
            stage: RecoveryStageV1::AccountPending,
            account_sid: None,
        });
        assert_eq!(
            select_setup_plan(before.clone()),
            Ok(SetupPlanV1::ResumeCreate)
        );

        before.local_account_sid = Some("S-1-5-21-1-1002".into());
        assert_eq!(
            select_setup_plan(before),
            Ok(SetupPlanV1::ResumeAuthenticate {
                account_sid: "S-1-5-21-1-1002".into()
            })
        );
    }

    #[test]
    fn unrelated_preexisting_account_is_rejected() {
        let mut value = evidence();
        value.local_account_sid = Some("S-1-5-21-1-1003".into());
        assert_eq!(
            select_setup_plan(value),
            Err("an unrelated pre-existing sandbox account was found")
        );
    }

    #[test]
    fn unexpected_legacy_fingerprint_is_not_recoverable() {
        let mut value = evidence();
        value.local_account_sid = Some("S-1-5-21-1-1004".into());
        value.legacy_fingerprint_matches = false;
        assert!(select_setup_plan(value).is_err());
    }

    #[test]
    fn exact_legacy_partial_can_enter_recovery_but_cannot_change_sid() {
        let mut value = evidence();
        value.local_account_sid = Some("S-1-5-21-1-1004".into());
        value.legacy_fingerprint_matches = true;
        assert_eq!(
            select_setup_plan(value),
            Ok(SetupPlanV1::BeginLegacyRecovery {
                account_sid: "S-1-5-21-1-1004".into()
            })
        );

        let mut exact_resume = evidence();
        exact_resume.local_account_sid = Some("S-1-5-21-1-1004".into());
        exact_resume.legacy_fingerprint_matches = true;
        exact_resume.recovery = Some(RecoveryEvidenceV1 {
            stage: RecoveryStageV1::LegacyPasswordRotationPending,
            account_sid: Some("S-1-5-21-1-1004".into()),
        });
        assert_eq!(
            select_setup_plan(exact_resume),
            Ok(SetupPlanV1::ResumeLegacyRotation {
                account_sid: "S-1-5-21-1-1004".into()
            })
        );

        let mut resumed = evidence();
        resumed.local_account_sid = Some("S-1-5-21-1-9999".into());
        resumed.legacy_fingerprint_matches = true;
        resumed.recovery = Some(RecoveryEvidenceV1 {
            stage: RecoveryStageV1::LegacyPasswordRotationPending,
            account_sid: Some("S-1-5-21-1-1004".into()),
        });
        assert!(select_setup_plan(resumed).is_err());
    }

    #[test]
    fn stale_or_replaced_final_and_bound_sids_are_rejected() {
        let mut finalized = evidence();
        finalized.final_account_sid = Some("S-1-5-21-1-1005".into());
        finalized.local_account_sid = Some("S-1-5-21-1-1006".into());
        assert!(select_setup_plan(finalized).is_err());

        let mut provisional = evidence();
        provisional.local_account_sid = Some("S-1-5-21-1-1006".into());
        provisional.recovery = Some(RecoveryEvidenceV1 {
            stage: RecoveryStageV1::Bound,
            account_sid: Some("S-1-5-21-1-1005".into()),
        });
        assert!(select_setup_plan(provisional).is_err());
    }
}
