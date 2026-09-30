//! Device bindings compiled into this Host, and the witness registry Core is
//! started with. No production binding is compiled in: every completion and
//! handover contract has no witness and can never be verified (fail-closed).
//!
//! A development build with the `physical-sim` feature can offer one
//! simulated reference body instead, chosen by `PASTEY_PHYSICAL_SIM`
//! (`flat` or `cup`). Release builds refuse that feature.
use crate::error::AppResult;
use crate::host_identity::HostRef;
use crate::physical::{binding::BindingClockV1, evidence::WitnessRegistryV1};
use std::sync::Arc;

pub(crate) fn witnesses() -> WitnessRegistryV1 {
    WitnessRegistryV1::default()
}

/// The bindings this Host runs, decided once at startup.
pub(crate) struct HostBindingsV1 {
    #[cfg(feature = "physical-sim")]
    body: Option<crate::physical::bindings::dev::ReferenceBodyV1>,
}
impl HostBindingsV1 {
    pub(crate) fn from_environment() -> AppResult<Self> {
        #[cfg(feature = "physical-sim")]
        {
            let body = std::env::var("PASTEY_PHYSICAL_SIM")
                .ok()
                .map(|name| crate::physical::bindings::dev::ReferenceBodyV1::parse(&name))
                .transpose()?;
            Ok(Self { body })
        }
        #[cfg(not(feature = "physical-sim"))]
        Ok(Self {})
    }
    /// The witnesses Core is started with; fixed for its lifetime.
    pub(crate) fn witnesses(&self) -> AppResult<WitnessRegistryV1> {
        #[cfg(feature = "physical-sim")]
        if let Some(body) = self.body {
            return body.witnesses();
        }
        Ok(witnesses())
    }
    /// Attaches the bindings to the new Core, on the Core's own clock.
    pub(crate) fn attach(
        &self,
        core: &mut super::PhysicalControlServiceV1,
        host: &HostRef,
        clock: Arc<dyn BindingClockV1>,
    ) -> AppResult<()> {
        #[cfg(feature = "physical-sim")]
        if let Some(body) = self.body {
            eprintln!("Development build: offering the simulated reference body {body:?}");
            return body.attach(core, host, clock);
        }
        let _ = (core, host, clock);
        Ok(())
    }
}
