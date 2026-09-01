//! Pure diagnostic policy for the explicit Windows ExecutionWorld verifier.
//!
//! Production availability deliberately discards native probe details. The
//! human-invoked verifier may preserve a bounded, single-line reason, but it
//! must not echo paths or authority/credential-bearing state.

pub(crate) const PRODUCTION_UNAVAILABLE_REASON: &str =
    "The elevated PasteySandboxOffline setup or native restricted-token, ACL, Firewall, handle-list, Job, descendant, filesystem, and NoRawNetwork conformance probe did not complete successfully.";

const WITHHELD_DIAGNOSTIC: &str =
    "Windows native conformance failed, but its detail was withheld by the verifier diagnostic boundary.";
const MAX_DIAGNOSTIC_BYTES: usize = 1_024;

pub(crate) fn production_unavailable_reason(_underlying: &str) -> String {
    PRODUCTION_UNAVAILABLE_REASON.into()
}

pub(crate) fn verifier_failure_reason(underlying: &str) -> String {
    let reason = underlying.trim();
    if reason.is_empty()
        || reason.len() > MAX_DIAGNOSTIC_BYTES
        || reason.chars().any(char::is_control)
        || contains_private_path(reason)
        || contains_sensitive_state(reason)
    {
        return WITHHELD_DIAGNOSTIC.into();
    }
    reason.into()
}

pub(crate) fn probe_parent_failure_reason(code: Option<i32>) -> &'static str {
    match code {
        Some(92) => "Windows Worker probe arguments or filesystem confinement check failed.",
        Some(93) => "Windows Worker probe environment confinement check failed.",
        Some(94) => "Windows Worker probe inherited-handle confinement check failed.",
        Some(95) => "Windows Worker probe NoRawNetwork check failed.",
        Some(96) => "Windows Worker probe Job breakaway check failed.",
        Some(97) => "Windows Worker probe could not create its contained descendant.",
        Some(98) => "Windows Worker probe descendant was not contained in a Job.",
        Some(99) => "Windows Worker probe Job active-process limit check failed.",
        _ => "Windows Worker confinement probe exited unsuccessfully.",
    }
}

pub(crate) fn create_process_with_logon_failure_reason(code: u32) -> String {
    let label = match code {
        2 => "application file not found",
        3 => "application path not found",
        5 => "access denied",
        193 => "invalid executable format",
        1314 => "required privilege not held",
        1326 => "user name or password incorrect",
        1327 => "account restriction",
        1331 => "account disabled",
        1385 => "logon type not granted",
        _ => "unclassified process-logon failure",
    };
    format!("CreateProcessWithLogonW failed with Win32 error {code} ({label}).")
}

fn contains_private_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    let windows_drive_path = bytes.windows(3).any(|window| {
        window[0].is_ascii_alphabetic() && window[1] == b':' && matches!(window[2], b'\\' | b'/')
    });
    let lower = value.to_ascii_lowercase();
    windows_drive_path
        || value.contains("\\\\")
        || lower.contains("/users/")
        || lower.contains("/home/")
        || lower.contains("/private/")
}

fn contains_sensitive_state(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "password=",
        "password:",
        "credential=",
        "credential:",
        "encrypted_password",
        "objectref",
        "object_ref",
        "grant=",
        "grant:",
        "evidence=",
        "evidence:",
        "secret=",
        "secret:",
        "token=",
        "token:",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_availability_failure_remains_generic() {
        let underlying = "Windows native conformance probe spawn failed at LogonUserW error 1326.";
        let reason = production_unavailable_reason(underlying);
        assert_eq!(reason, PRODUCTION_UNAVAILABLE_REASON);
        assert!(!reason.contains(underlying));
    }

    #[test]
    fn explicit_verifier_preserves_a_safe_bounded_native_reason() {
        let underlying =
            "Windows native conformance probe spawn failed: LogonUserW returned Win32 error 1326.";
        assert_eq!(verifier_failure_reason(underlying), underlying);
    }

    #[test]
    fn create_process_with_logon_diagnostic_retains_code_and_only_static_context() {
        assert_eq!(
            create_process_with_logon_failure_reason(1385),
            "CreateProcessWithLogonW failed with Win32 error 1385 (logon type not granted)."
        );
        assert_eq!(
            create_process_with_logon_failure_reason(5),
            "CreateProcessWithLogonW failed with Win32 error 5 (access denied)."
        );
        assert_eq!(
            create_process_with_logon_failure_reason(65_535),
            "CreateProcessWithLogonW failed with Win32 error 65535 (unclassified process-logon failure)."
        );
    }

    #[test]
    fn explicit_verifier_withholds_secret_authority_and_private_path_state() {
        for underlying in [
            "probe failed: password=do-not-print",
            "probe failed: encrypted_password=do-not-print",
            "probe failed: ObjectRef=object-do-not-print",
            "probe failed: grant=grant-do-not-print",
            r"probe failed at C:\Users\Host\private-state.json",
        ] {
            let reason = verifier_failure_reason(underlying);
            assert_eq!(reason, WITHHELD_DIAGNOSTIC);
            assert!(!reason.contains("do-not-print"));
            assert!(!reason.contains("Host"));
        }
    }
}
