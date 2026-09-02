//! Diagnostic policy for the Codex-derived Windows ExecutionWorld verifier.
//!
//! Production availability deliberately discards native probe details. The
//! human-invoked verifier may preserve a bounded, single-line reason, but it
//! must not echo paths or authority/credential-bearing state.

pub(crate) const PRODUCTION_UNAVAILABLE_REASON: &str =
    "The Host-owned Codex Windows sandbox setup or Pastey native resource-projection, environment, standard-I/O, process-session, cancellation, and NoRawNetwork conformance probe did not complete successfully.";

const WITHHELD_DIAGNOSTIC: &str =
    "Windows native conformance failed, but its detail was withheld by the verifier diagnostic boundary.";
const MAX_DIAGNOSTIC_BYTES: usize = 1_024;
const MAX_PROBE_REPORT_BYTES: usize = 512;

pub(crate) const PROBE_DIAGNOSTIC_FILENAME: &str = "probe-diagnostics.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeNetworkDiagnostic {
    Connected,
    Denied(Option<i32>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeProbeDiagnosticReport {
    pub(crate) input_read: bool,
    pub(crate) output_write: bool,
    pub(crate) explicit_env: bool,
    pub(crate) host_secret_absent: bool,
    pub(crate) handle_not_inherited: bool,
    pub(crate) external_network: ProbeNetworkDiagnostic,
    pub(crate) loopback_network: ProbeNetworkDiagnostic,
}

impl NativeProbeDiagnosticReport {
    pub(crate) fn render(&self) -> String {
        format!(
            "input_read={}\noutput_write={}\nexplicit_env={}\nhost_secret_absent={}\nhandle_not_inherited={}\nexternal_network={}\nloopback_network={}\n",
            self.input_read,
            self.output_write,
            self.explicit_env,
            self.host_secret_absent,
            self.handle_not_inherited,
            render_network(self.external_network),
            render_network(self.loopback_network),
        )
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "input_read={}; output_write={}; explicit_env={}; host_secret_absent={}; handle_not_inherited={}; external_network={}; loopback_network={}",
            self.input_read,
            self.output_write,
            self.explicit_env,
            self.host_secret_absent,
            self.handle_not_inherited,
            render_network(self.external_network),
            render_network(self.loopback_network),
        )
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        if value.len() > MAX_PROBE_REPORT_BYTES {
            return None;
        }
        let mut lines = value.lines();
        let report = Self {
            input_read: parse_bool_line(lines.next()?, "input_read=")?,
            output_write: parse_bool_line(lines.next()?, "output_write=")?,
            explicit_env: parse_bool_line(lines.next()?, "explicit_env=")?,
            host_secret_absent: parse_bool_line(lines.next()?, "host_secret_absent=")?,
            handle_not_inherited: parse_bool_line(lines.next()?, "handle_not_inherited=")?,
            external_network: parse_network_line(lines.next()?, "external_network=")?,
            loopback_network: parse_network_line(lines.next()?, "loopback_network=")?,
        };
        if lines.next().is_some() {
            return None;
        }
        Some(report)
    }
}

fn render_network(value: ProbeNetworkDiagnostic) -> String {
    match value {
        ProbeNetworkDiagnostic::Connected => "connected".into(),
        ProbeNetworkDiagnostic::Denied(Some(code)) => format!("denied({code})"),
        ProbeNetworkDiagnostic::Denied(None) => "denied(no_raw_os_error)".into(),
    }
}

fn parse_bool_line(line: &str, prefix: &str) -> Option<bool> {
    match line.strip_prefix(prefix)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_network_line(line: &str, prefix: &str) -> Option<ProbeNetworkDiagnostic> {
    let value = line.strip_prefix(prefix)?;
    if value == "connected" {
        return Some(ProbeNetworkDiagnostic::Connected);
    }
    if value == "denied(no_raw_os_error)" {
        return Some(ProbeNetworkDiagnostic::Denied(None));
    }
    let code = value
        .strip_prefix("denied(")?
        .strip_suffix(')')?
        .parse()
        .ok()?;
    Some(ProbeNetworkDiagnostic::Denied(Some(code)))
}

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

    #[test]
    fn probe_report_round_trips_only_fixed_safe_fields() {
        let report = NativeProbeDiagnosticReport {
            input_read: true,
            output_write: true,
            explicit_env: true,
            host_secret_absent: true,
            handle_not_inherited: true,
            external_network: ProbeNetworkDiagnostic::Denied(Some(10013)),
            loopback_network: ProbeNetworkDiagnostic::Denied(Some(10061)),
        };

        assert_eq!(
            NativeProbeDiagnosticReport::parse(&report.render()),
            Some(report.clone())
        );
        assert_eq!(
            report.summary(),
            "input_read=true; output_write=true; explicit_env=true; host_secret_absent=true; handle_not_inherited=true; external_network=denied(10013); loopback_network=denied(10061)"
        );
    }

    #[test]
    fn probe_report_rejects_untrusted_extra_or_secret_content() {
        let valid = NativeProbeDiagnosticReport {
            input_read: true,
            output_write: true,
            explicit_env: true,
            host_secret_absent: true,
            handle_not_inherited: true,
            external_network: ProbeNetworkDiagnostic::Denied(Some(10013)),
            loopback_network: ProbeNetworkDiagnostic::Denied(Some(10013)),
        }
        .render();
        assert!(
            NativeProbeDiagnosticReport::parse(&format!("{valid}secret=do-not-print")).is_none()
        );
        assert!(NativeProbeDiagnosticReport::parse(
            &valid.replace("denied(10013)", "denied(oops)")
        )
        .is_none());
    }
}
