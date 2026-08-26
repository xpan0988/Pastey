//! Phase 5 Step 7 Host-owned network brokerage.
//!
//! This module owns all managed sockets. Execution worlds remain raw-network
//! denied and receive only opaque action facts. The broker is process-local,
//! unattached to live v2 dispatch, and never derives authority from a tool,
//! executable, task, capability observation, or secret handle.

#![allow(dead_code)] // Step 8 attaches Core-owned v2 grants/results.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    io::{Read, Write},
    net::{
        IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
    },
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{
    effect_authority::{
        network_destination_ref_for_context, network_scope_ref_for_context, AuthorityContextRefV1,
        BackendApplyV1, BackendEffectOutcomeV1, CurrentHostAuthorityV1, EffectAuthorityStateV1,
        EffectDecisionV1, EffectFactsV1, EffectPreconditionV1, EffectRequestKindV1,
        EffectRequestV1, HostEffectBackendV1, ManagedRunDraftV1, ManagedRunRefV1, NetworkEffectV1,
        NetworkGrantV1, NetworkScopeRefV1, NetworkVerbV1,
    },
    error::{AppError, AppResult},
    host_identity::HostRef,
};

const NETWORK_BROKER_VERSION: &str = "pastey-network-broker-v1";
const TCP_TRANSPORT_REF: &str = "pastey-network-transport:v1:tcp";
const MAX_HOSTNAME_BYTES: usize = 253;
const MAX_STAGED_REQUEST_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NetworkScopeKindV1 {
    NameResolution,
    Loopback,
    Lan,
    Internet,
}

impl NetworkScopeKindV1 {
    fn label(self) -> &'static str {
        match self {
            Self::NameResolution => "name_resolution",
            Self::Loopback => "loopback",
            Self::Lan => "lan",
            Self::Internet => "internet",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NetworkTransportV1 {
    Tcp,
}

impl NetworkTransportV1 {
    pub(crate) fn transport_ref(self) -> &'static str {
        match self {
            Self::Tcp => TCP_TRANSPORT_REF,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NetworkDnsModeV1 {
    Denied,
    SystemPinned,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NetworkProxyModeV1 {
    DirectOnly,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NetworkScopeBindingSpecV1 {
    pub(crate) scope_ref: NetworkScopeRefV1,
    pub(crate) kind: NetworkScopeKindV1,
    pub(crate) allowed_verbs: BTreeSet<NetworkVerbV1>,
    pub(crate) transports: BTreeSet<NetworkTransportV1>,
    pub(crate) dns_mode: NetworkDnsModeV1,
    pub(crate) proxy_mode: NetworkProxyModeV1,
    pub(crate) expires_at: i64,
}

impl NetworkScopeBindingSpecV1 {
    pub(crate) fn new(
        draft: &ManagedRunDraftV1,
        kind: NetworkScopeKindV1,
        allowed_verbs: BTreeSet<NetworkVerbV1>,
        transports: BTreeSet<NetworkTransportV1>,
        dns_mode: NetworkDnsModeV1,
        expires_at: i64,
    ) -> AppResult<Self> {
        let digest = scope_descriptor_digest(
            kind,
            &allowed_verbs,
            &transports,
            dns_mode,
            NetworkProxyModeV1::DirectOnly,
            expires_at,
        )?;
        Ok(Self {
            scope_ref: network_scope_ref_for_context(
                &draft.context_ref,
                &draft.run_control_ref,
                &draft.context.host_ref,
                &digest,
            )?,
            kind,
            allowed_verbs,
            transports,
            dns_mode,
            proxy_mode: NetworkProxyModeV1::DirectOnly,
            expires_at,
        })
    }

    fn validate_identity(
        &self,
        context_ref: &AuthorityContextRefV1,
        run_ref: &ManagedRunRefV1,
        host_ref: &HostRef,
    ) -> AppResult<()> {
        let digest = scope_descriptor_digest(
            self.kind,
            &self.allowed_verbs,
            &self.transports,
            self.dns_mode,
            self.proxy_mode,
            self.expires_at,
        )?;
        if self.scope_ref != network_scope_ref_for_context(context_ref, run_ref, host_ref, &digest)?
            || self.allowed_verbs.is_empty()
            || self.transports.is_empty()
            || self.proxy_mode != NetworkProxyModeV1::DirectOnly
        {
            return invalid("Network scope identity or policy is invalid.");
        }
        let supported = match self.kind {
            NetworkScopeKindV1::NameResolution => {
                self.allowed_verbs
                    .iter()
                    .all(|verb| *verb == NetworkVerbV1::Resolve)
                    && self.dns_mode == NetworkDnsModeV1::SystemPinned
            }
            NetworkScopeKindV1::Loopback | NetworkScopeKindV1::Lan => self
                .allowed_verbs
                .iter()
                .all(|verb| matches!(verb, NetworkVerbV1::Connect | NetworkVerbV1::Bind)),
            NetworkScopeKindV1::Internet => self
                .allowed_verbs
                .iter()
                .all(|verb| *verb == NetworkVerbV1::Connect),
        };
        if !supported
            || (self.kind != NetworkScopeKindV1::NameResolution
                && self.dns_mode != NetworkDnsModeV1::Denied)
        {
            return invalid("Network scope mixes independent zone or DNS authority.");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "hostKind", content = "value")]
pub(crate) enum NetworkEndpointHostV1 {
    Literal(IpAddr),
    Hostname(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NetworkDestinationBindingSpecV1 {
    pub(crate) destination_ref: String,
    pub(crate) host: NetworkEndpointHostV1,
    pub(crate) port: u16,
    pub(crate) transport: NetworkTransportV1,
    pub(crate) expected_scope: NetworkScopeKindV1,
    pub(crate) expires_at: i64,
}

impl NetworkDestinationBindingSpecV1 {
    pub(crate) fn new(
        draft: &ManagedRunDraftV1,
        host: NetworkEndpointHostV1,
        port: u16,
        transport: NetworkTransportV1,
        expected_scope: NetworkScopeKindV1,
        expires_at: i64,
    ) -> AppResult<Self> {
        validate_endpoint_host(&host)?;
        let digest =
            destination_descriptor_digest(&host, port, transport, expected_scope, expires_at)?;
        Ok(Self {
            destination_ref: network_destination_ref_for_context(
                &draft.context_ref,
                &draft.run_control_ref,
                &draft.context.host_ref,
                &digest,
            )?,
            host,
            port,
            transport,
            expected_scope,
            expires_at,
        })
    }

    fn validate_identity(
        &self,
        context_ref: &AuthorityContextRefV1,
        run_ref: &ManagedRunRefV1,
        host_ref: &HostRef,
    ) -> AppResult<()> {
        validate_endpoint_host(&self.host)?;
        if self.expected_scope == NetworkScopeKindV1::NameResolution {
            return invalid("Name resolution is a scope, not a destination zone.");
        }
        if let NetworkEndpointHostV1::Literal(address) = self.host {
            if classify_address(address)? != self.expected_scope {
                return invalid("Literal destination does not match its exact network scope.");
            }
        }
        let digest = destination_descriptor_digest(
            &self.host,
            self.port,
            self.transport,
            self.expected_scope,
            self.expires_at,
        )?;
        if self.destination_ref
            != network_destination_ref_for_context(context_ref, run_ref, host_ref, &digest)?
        {
            return invalid("Network destination identity was substituted.");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NetworkBrokerAccessV1 {
    pub(crate) envelope_ref: crate::effect_authority::EffectEnvelopeRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) context: crate::effect_authority::AuthorityContextV1,
    pub(crate) current: CurrentHostAuthorityV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StagedNetworkExchangeV1 {
    pub(crate) request_bytes: Vec<u8>,
    pub(crate) max_response_bytes: u64,
}

impl StagedNetworkExchangeV1 {
    pub(crate) fn request_digest(&self) -> AppResult<String> {
        domain_hash(
            "pastey-network-request-bytes-v1",
            &(
                blake3::hash(&self.request_bytes).to_hex().to_string(),
                self.max_response_bytes,
            ),
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NetworkBrokerAvailabilityV1 {
    pub(crate) available: bool,
    pub(crate) identity_digest: String,
    pub(crate) unavailable_reason: Option<String>,
}

trait HostNameResolverV1: Send + Sync {
    fn availability(&self) -> NetworkBrokerAvailabilityV1;
    fn resolve(&self, hostname: &str, port: u16) -> std::io::Result<Vec<SocketAddr>>;
}

#[derive(Default)]
struct SystemHostNameResolverV1;

impl HostNameResolverV1 for SystemHostNameResolverV1 {
    fn availability(&self) -> NetworkBrokerAvailabilityV1 {
        NetworkBrokerAvailabilityV1 {
            available: true,
            identity_digest: format!("{NETWORK_BROKER_VERSION}:host-owned-tcp-system-dns"),
            unavailable_reason: None,
        }
    }

    fn resolve(&self, hostname: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        (hostname, port)
            .to_socket_addrs()
            .map(|items| items.collect())
    }
}

#[derive(Clone, Debug)]
struct NetworkOwnerV1 {
    envelope_ref: crate::effect_authority::EffectEnvelopeRefV1,
    run_ref: ManagedRunRefV1,
    context_ref: AuthorityContextRefV1,
    host_ref: HostRef,
    bridge_id: String,
    session_binding_ref: String,
}

#[derive(Clone, Debug)]
struct ResolutionPinV1 {
    scope_ref: NetworkScopeRefV1,
    generation_ref: String,
    addresses: Vec<SocketAddr>,
}

#[derive(Clone, Debug)]
struct StagedNetworkRequestV1 {
    owner: NetworkOwnerV1,
    effect: NetworkEffectV1,
    exchange: Option<StagedNetworkExchangeV1>,
    logical_now: i64,
}

struct ProvisionedNetworkGrantV1 {
    owner: NetworkOwnerV1,
    grant: NetworkGrantV1,
    scopes: HashMap<NetworkScopeRefV1, NetworkScopeBindingSpecV1>,
    destinations: HashMap<String, NetworkDestinationBindingSpecV1>,
    resolutions: HashMap<String, ResolutionPinV1>,
    staged: HashMap<crate::effect_authority::EffectRequestIdV1, StagedNetworkRequestV1>,
    revoked: bool,
}

struct ManagedConnectionV1 {
    owner: NetworkOwnerV1,
    stream: TcpStream,
}

struct ManagedListenerV1 {
    owner: NetworkOwnerV1,
    listener: TcpListener,
}

#[derive(Default)]
struct NetworkBrokerStateV1 {
    grants: HashMap<ManagedRunRefV1, ProvisionedNetworkGrantV1>,
    connections: HashMap<String, ManagedConnectionV1>,
    listeners: HashMap<String, ManagedListenerV1>,
    revoked_runs: HashSet<ManagedRunRefV1>,
}

/// Shared process-local broker. Its sockets are never returned to a Worker or
/// mounted into an ExecutionWorld.
pub(crate) struct NetworkBrokerServiceV1 {
    state: Mutex<NetworkBrokerStateV1>,
    resolver: Arc<dyn HostNameResolverV1>,
}

impl Default for NetworkBrokerServiceV1 {
    fn default() -> Self {
        Self {
            state: Mutex::new(NetworkBrokerStateV1::default()),
            resolver: Arc::new(SystemHostNameResolverV1),
        }
    }
}

impl NetworkBrokerServiceV1 {
    pub(crate) fn platform_availability(&self) -> NetworkBrokerAvailabilityV1 {
        self.resolver.availability()
    }

    pub(crate) fn provision_grant(
        &self,
        authority: &EffectAuthorityStateV1,
        access: NetworkBrokerAccessV1,
        scopes: Vec<NetworkScopeBindingSpecV1>,
        destinations: Vec<NetworkDestinationBindingSpecV1>,
    ) -> AppResult<()> {
        let availability = self.platform_availability();
        if !availability.available {
            return unavailable("Host network brokerage is unavailable.");
        }
        let first_scope = scopes
            .first()
            .ok_or_else(|| AppError::InvalidInput("Network scope bindings are empty.".into()))?;
        let first_destination = destinations.first().ok_or_else(|| {
            AppError::InvalidInput("Network destination bindings are empty.".into())
        })?;
        let grant = authority.validate_network_attachment(
            &first_scope.scope_ref,
            &first_destination.destination_ref,
            &access.envelope_ref,
            &access.run_control_ref,
            &access.context,
            &access.current,
        )?;
        let context_ref = access.context.context_ref()?;
        let mut scope_map = HashMap::new();
        let mut represented_verbs = BTreeSet::new();
        for scope in scopes {
            scope.validate_identity(
                &context_ref,
                &access.run_control_ref,
                &access.context.host_ref,
            )?;
            represented_verbs.extend(scope.allowed_verbs.iter().copied());
            if scope.expires_at > grant.expires_at
                || scope.expires_at <= access.current.now
                || !scope.allowed_verbs.is_subset(&grant.allowed_verbs)
                || scope_map.insert(scope.scope_ref.clone(), scope).is_some()
            {
                return invalid("Network scope binding widens or duplicates its grant.");
            }
        }
        let mut destination_map = HashMap::new();
        for destination in destinations {
            destination.validate_identity(
                &context_ref,
                &access.run_control_ref,
                &access.context.host_ref,
            )?;
            if destination.expires_at > grant.expires_at
                || destination.expires_at <= access.current.now
                || destination_map
                    .insert(destination.destination_ref.clone(), destination)
                    .is_some()
            {
                return invalid("Network destination binding widens or duplicates its grant.");
            }
        }
        if grant.host_ref != access.context.host_ref
            || scope_map.keys().cloned().collect::<BTreeSet<_>>() != grant.scope_refs
            || destination_map.keys().cloned().collect::<BTreeSet<_>>() != grant.destination_refs
            || represented_verbs != grant.allowed_verbs
        {
            return invalid("Host-resolved network topology does not exactly match the grant.");
        }
        let owner = NetworkOwnerV1 {
            envelope_ref: access.envelope_ref,
            run_ref: access.run_control_ref.clone(),
            context_ref,
            host_ref: access.context.host_ref,
            bridge_id: access.context.bridge_id,
            session_binding_ref: access.context.session_binding_ref,
        };
        let mut state = self.state.lock();
        if state.revoked_runs.contains(&access.run_control_ref)
            || state.grants.contains_key(&access.run_control_ref)
        {
            return invalid("Network grant is revoked or already provisioned.");
        }
        state.grants.insert(
            access.run_control_ref,
            ProvisionedNetworkGrantV1 {
                owner,
                grant,
                scopes: scope_map,
                destinations: destination_map,
                resolutions: HashMap::new(),
                staged: HashMap::new(),
                revoked: false,
            },
        );
        Ok(())
    }

    pub(crate) fn prepare_request(
        &self,
        authority: &EffectAuthorityStateV1,
        request: &EffectRequestV1,
        current: &CurrentHostAuthorityV1,
        exchange: Option<StagedNetworkExchangeV1>,
    ) -> AppResult<()> {
        self.reconcile_expired(current.now);
        let grant = authority.validate_network_request_attachment(request, current)?;
        let EffectRequestKindV1::Network(effect) = &request.effect else {
            return invalid("Expected a brokered network request.");
        };
        validate_network_budget_shape(request, exchange.as_ref())?;
        if let Some(exchange) = exchange.as_ref() {
            if exchange.request_bytes.len() > MAX_STAGED_REQUEST_BYTES
                || exchange.request_digest().ok().as_deref() != effect.request_digest.as_deref()
            {
                return invalid("Staged network request bytes are oversized or mismatched.");
            }
        } else if effect.request_digest.is_some() {
            return invalid("Network request bytes were not staged.");
        }
        let mut state = self.state.lock();
        let record = state
            .grants
            .get_mut(&request.run_control_ref)
            .ok_or_else(|| {
                AppError::InvalidInput("Network grant resolution is unavailable.".into())
            })?;
        validate_owner_request(&record.owner, request)?;
        let scope = record.scopes.get(&effect.scope_ref).ok_or_else(|| {
            AppError::InvalidInput("Network scope resolution is unavailable.".into())
        })?;
        let destination = record
            .destinations
            .get(&effect.destination_ref)
            .ok_or_else(|| {
                AppError::InvalidInput("Network destination resolution is unavailable.".into())
            })?;
        if record.revoked
            || grant != record.grant
            || scope.expires_at <= current.now
            || destination.expires_at <= current.now
            || !scope.allowed_verbs.contains(&effect.verb)
            || !scope.transports.contains(&destination.transport)
            || effect.transport_ref != destination.transport.transport_ref()
            || record.staged.contains_key(&request.request_id)
        {
            return invalid("Network request widens, substitutes, or replays its binding.");
        }
        validate_scope_for_effect(scope, destination, effect)?;
        record.staged.insert(
            request.request_id.clone(),
            StagedNetworkRequestV1 {
                owner: record.owner.clone(),
                effect: effect.clone(),
                exchange,
                logical_now: current.now,
            },
        );
        Ok(())
    }

    fn apply_network(&self, request: &EffectRequestV1) -> AppResult<BackendEffectOutcomeV1> {
        let availability = self.platform_availability();
        if !availability.available {
            return Ok(unavailable_outcome("network_broker_platform_unavailable"));
        }
        let staged = {
            let mut state = self.state.lock();
            let record = state
                .grants
                .get_mut(&request.run_control_ref)
                .ok_or_else(|| AppError::InvalidInput("Network grant is unavailable.".into()))?;
            validate_owner_request(&record.owner, request)?;
            if record.revoked {
                return invalid("Network grant is revoked.");
            }
            record.staged.remove(&request.request_id).ok_or_else(|| {
                AppError::InvalidInput("Exact staged network request is unavailable.".into())
            })?
        };
        validate_owner_request(&staged.owner, request)?;
        match staged.effect.verb {
            NetworkVerbV1::Resolve => self.apply_resolve(request, staged),
            NetworkVerbV1::Connect => self.apply_connect(request, staged),
            NetworkVerbV1::Bind => self.apply_bind(request, staged),
        }
    }

    fn apply_resolve(
        &self,
        request: &EffectRequestV1,
        staged: StagedNetworkRequestV1,
    ) -> AppResult<BackendEffectOutcomeV1> {
        let (scope, destination) = self.bound_specs(request, &staged.effect)?;
        let NetworkEndpointHostV1::Hostname(hostname) = &destination.host else {
            return invalid("Literal destinations do not require name resolution.");
        };
        let started = Instant::now();
        let timeout = network_timeout(request)?;
        let addresses =
            resolve_with_timeout(self.resolver.clone(), hostname, destination.port, timeout)?;
        let addresses =
            validate_resolved_addresses(addresses, destination.expected_scope, destination.port)?;
        let generation_ref = resolution_generation(&destination.destination_ref, &addresses)?;
        let endpoint_refs = endpoint_refs(&addresses)?;
        let pin = ResolutionPinV1 {
            scope_ref: scope.scope_ref.clone(),
            generation_ref: generation_ref.clone(),
            addresses,
        };
        let mut state = self.state.lock();
        let record = state
            .grants
            .get_mut(&request.run_control_ref)
            .ok_or_else(|| {
                AppError::InvalidInput("Network grant was revoked during resolution.".into())
            })?;
        if record.revoked || record.owner.context_ref != staged.owner.context_ref {
            return invalid("Network grant was revoked during resolution.");
        }
        record
            .resolutions
            .insert(destination.destination_ref.clone(), pin);
        Ok(network_outcome(
            request,
            &scope,
            &destination,
            "resolved",
            endpoint_refs,
            Some(generation_ref),
            None,
            0,
            0,
            elapsed_millis(started),
            false,
            true,
            "network_destination_resolved",
        )?)
    }

    fn apply_connect(
        &self,
        request: &EffectRequestV1,
        staged: StagedNetworkRequestV1,
    ) -> AppResult<BackendEffectOutcomeV1> {
        let (scope, destination) = self.bound_specs(request, &staged.effect)?;
        let started = Instant::now();
        let timeout = network_timeout(request)?;
        let (addresses, generation, dns_revalidated) = match &destination.host {
            NetworkEndpointHostV1::Literal(address) => (
                vec![SocketAddr::new(*address, destination.port)],
                None,
                false,
            ),
            NetworkEndpointHostV1::Hostname(hostname) => {
                let pin = {
                    let state = self.state.lock();
                    state
                        .grants
                        .get(&request.run_control_ref)
                        .and_then(|record| record.resolutions.get(&destination.destination_ref))
                        .cloned()
                        .ok_or_else(|| {
                            AppError::InvalidInput(
                                "Hostname connection lacks a pinned resolution.".into(),
                            )
                        })?
                };
                if staged.effect.resolution_generation_ref.as_deref()
                    != Some(pin.generation_ref.as_str())
                    || !request.preconditions.iter().any(|precondition| matches!(
                        precondition,
                        EffectPreconditionV1::DestinationGeneration { scope_ref, generation_ref }
                            if *scope_ref == pin.scope_ref && generation_ref == &pin.generation_ref
                    ))
                {
                    return invalid("Hostname connection substituted its resolution generation.");
                }
                let fresh = resolve_with_timeout(
                    self.resolver.clone(),
                    hostname,
                    destination.port,
                    timeout,
                )?;
                let fresh = validate_resolved_addresses(
                    fresh,
                    destination.expected_scope,
                    destination.port,
                )?;
                if fresh != pin.addresses
                    || resolution_generation(&destination.destination_ref, &fresh)?
                        != pin.generation_ref
                {
                    return invalid("DNS resolution changed before connect.");
                }
                (fresh, Some(pin.generation_ref), true)
            }
        };
        let address = *addresses
            .first()
            .ok_or_else(|| AppError::InvalidInput("No validated destination remains.".into()))?;
        let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(|_| {
            AppError::InvalidInput("Brokered connection failed within its exact scope.".into())
        })?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let action_ref = action_ref(request)?;
        {
            let mut state = self.state.lock();
            if state.revoked_runs.contains(&request.run_control_ref)
                || !state.grants.contains_key(&request.run_control_ref)
            {
                let _ = stream.shutdown(Shutdown::Both);
                return invalid("Network run was revoked during connect.");
            }
            state.connections.insert(
                action_ref.clone(),
                ManagedConnectionV1 {
                    owner: staged.owner.clone(),
                    stream: stream.try_clone()?,
                },
            );
        }
        let mut bytes_sent = 0_u64;
        let mut bytes_received = 0_u64;
        let mut closed = false;
        if let Some(exchange) = staged.exchange {
            let exchange_result = (|| -> AppResult<(u64, u64)> {
                stream.write_all(&exchange.request_bytes)?;
                let sent = exchange.request_bytes.len() as u64;
                let mut received = 0;
                if exchange.max_response_bytes > 0 {
                    let mut response = Vec::new();
                    let limit = exchange.max_response_bytes.saturating_add(1);
                    (&mut stream).take(limit).read_to_end(&mut response)?;
                    if response.len() as u64 > exchange.max_response_bytes {
                        return invalid("Brokered response exceeded its reserved byte budget.");
                    }
                    received = response.len() as u64;
                }
                Ok((sent, received))
            })();
            let _ = stream.shutdown(Shutdown::Both);
            if let Some(connection) = self.state.lock().connections.remove(&action_ref) {
                let _ = connection.stream.shutdown(Shutdown::Both);
            }
            (bytes_sent, bytes_received) = exchange_result?;
            closed = true;
        }
        if bytes_sent.saturating_add(bytes_received) > request.requested_budget_slice.network_bytes
        {
            let _ = stream.shutdown(Shutdown::Both);
            return invalid("Brokered network bytes exceeded the reserved budget.");
        }
        let state = self.state.lock();
        if state.revoked_runs.contains(&request.run_control_ref)
            || !state.grants.contains_key(&request.run_control_ref)
        {
            drop(state);
            let _ = stream.shutdown(Shutdown::Both);
            return invalid("Network run was revoked during connect.");
        }
        drop(state);
        Ok(network_outcome_with_action(
            request,
            &scope,
            &destination,
            action_ref,
            "connected",
            endpoint_refs(&addresses)?,
            generation,
            None,
            bytes_sent,
            bytes_received,
            elapsed_millis(started),
            dns_revalidated,
            closed,
            "network_destination_connected",
        ))
    }

    fn apply_bind(
        &self,
        request: &EffectRequestV1,
        staged: StagedNetworkRequestV1,
    ) -> AppResult<BackendEffectOutcomeV1> {
        let (scope, destination) = self.bound_specs(request, &staged.effect)?;
        let NetworkEndpointHostV1::Literal(address) = destination.host else {
            return invalid("Brokered bind requires an exact literal Host address.");
        };
        let started = Instant::now();
        let listener =
            TcpListener::bind(SocketAddr::new(address, destination.port)).map_err(|_| {
                AppError::InvalidInput("Brokered bind failed within its exact scope.".into())
            })?;
        listener.set_nonblocking(true)?;
        let local = listener.local_addr()?;
        if classify_address(local.ip())? != scope.kind {
            return invalid("Brokered bind produced an endpoint outside its scope.");
        }
        let action_ref = action_ref(request)?;
        let mut state = self.state.lock();
        if state.revoked_runs.contains(&request.run_control_ref) {
            return invalid("Network run was revoked during bind.");
        }
        state.listeners.insert(
            action_ref.clone(),
            ManagedListenerV1 {
                owner: staged.owner,
                listener,
            },
        );
        Ok(network_outcome_with_action(
            request,
            &scope,
            &destination,
            action_ref,
            "bound",
            Vec::new(),
            None,
            Some(endpoint_ref(local)?),
            0,
            0,
            elapsed_millis(started),
            false,
            false,
            "network_endpoint_bound",
        ))
    }

    fn bound_specs(
        &self,
        request: &EffectRequestV1,
        effect: &NetworkEffectV1,
    ) -> AppResult<(NetworkScopeBindingSpecV1, NetworkDestinationBindingSpecV1)> {
        let state = self.state.lock();
        let record = state
            .grants
            .get(&request.run_control_ref)
            .ok_or_else(|| AppError::InvalidInput("Network grant is unavailable.".into()))?;
        validate_owner_request(&record.owner, request)?;
        let scope = record
            .scopes
            .get(&effect.scope_ref)
            .cloned()
            .ok_or_else(|| {
                AppError::InvalidInput("Network scope resolution is unavailable.".into())
            })?;
        let destination = record
            .destinations
            .get(&effect.destination_ref)
            .cloned()
            .ok_or_else(|| {
                AppError::InvalidInput("Network destination resolution is unavailable.".into())
            })?;
        Ok((scope, destination))
    }

    pub(crate) fn terminate_run(&self, run_ref: &ManagedRunRefV1) -> usize {
        self.terminate_matching(|owner| owner.run_ref == *run_ref)
    }

    pub(crate) fn run_is_quiescent(&self, run_ref: &ManagedRunRefV1) -> bool {
        let state = self.state.lock();
        !state
            .connections
            .values()
            .any(|connection| connection.owner.run_ref == *run_ref)
            && !state
                .listeners
                .values()
                .any(|listener| listener.owner.run_ref == *run_ref)
    }

    pub(crate) fn terminate_bridge(&self, bridge_id: &str) -> usize {
        self.terminate_matching(|owner| owner.bridge_id == bridge_id)
    }

    pub(crate) fn terminate_session(&self, session_binding_ref: &str) -> usize {
        self.terminate_matching(|owner| owner.session_binding_ref == session_binding_ref)
    }

    pub(crate) fn run_refs_for_session(
        &self,
        session_binding_ref: &str,
    ) -> BTreeSet<ManagedRunRefV1> {
        self.state
            .lock()
            .grants
            .values()
            .filter(|record| record.owner.session_binding_ref == session_binding_ref)
            .map(|record| record.owner.run_ref.clone())
            .collect()
    }

    pub(crate) fn terminate_all(&self) -> usize {
        self.terminate_matching(|_| true)
    }

    pub(crate) fn reconcile_expired(&self, now: i64) -> usize {
        let expired = self
            .state
            .lock()
            .grants
            .values()
            .filter(|record| record.grant.expires_at <= now)
            .map(|record| record.owner.run_ref.clone())
            .collect::<BTreeSet<_>>();
        expired
            .iter()
            .map(|run_ref| self.terminate_run(run_ref))
            .sum()
    }

    fn terminate_matching(&self, predicate: impl Fn(&NetworkOwnerV1) -> bool) -> usize {
        let mut state = self.state.lock();
        let runs = state
            .grants
            .values()
            .filter(|record| predicate(&record.owner))
            .map(|record| record.owner.run_ref.clone())
            .collect::<BTreeSet<_>>();
        for run_ref in &runs {
            state.revoked_runs.insert(run_ref.clone());
            if let Some(record) = state.grants.get_mut(run_ref) {
                record.revoked = true;
                record.staged.clear();
                record.resolutions.clear();
            }
        }
        state.connections.retain(|_, connection| {
            if runs.contains(&connection.owner.run_ref) {
                let _ = connection.stream.shutdown(Shutdown::Both);
                false
            } else {
                true
            }
        });
        state
            .listeners
            .retain(|_, listener| !runs.contains(&listener.owner.run_ref));
        for run_ref in &runs {
            state.grants.remove(run_ref);
        }
        runs.len()
    }
}

impl Drop for NetworkBrokerServiceV1 {
    fn drop(&mut self) {
        self.terminate_all();
    }
}

pub(crate) struct HostManagedNetworkBackendV1<'a> {
    broker: &'a NetworkBrokerServiceV1,
}

impl<'a> HostManagedNetworkBackendV1<'a> {
    pub(crate) fn new(broker: &'a NetworkBrokerServiceV1) -> Self {
        Self { broker }
    }
}

impl HostEffectBackendV1 for HostManagedNetworkBackendV1<'_> {
    fn apply(&mut self, request: &EffectRequestV1) -> BackendApplyV1 {
        let outcome = match &request.effect {
            EffectRequestKindV1::Network(_) => self.broker.apply_network(request),
            _ => Ok(unavailable_outcome(
                "network_backend_has_no_resource_or_process_authority",
            )),
        }
        .unwrap_or_else(|_| BackendEffectOutcomeV1 {
            decision: EffectDecisionV1::Denied,
            actual_effect_summary: "brokered_network_effect_denied".into(),
            facts: EffectFactsV1::None,
        });
        BackendApplyV1::Completed(outcome)
    }
}

fn validate_scope_for_effect(
    scope: &NetworkScopeBindingSpecV1,
    destination: &NetworkDestinationBindingSpecV1,
    effect: &NetworkEffectV1,
) -> AppResult<()> {
    let valid = match effect.verb {
        NetworkVerbV1::Resolve => {
            scope.kind == NetworkScopeKindV1::NameResolution
                && scope.dns_mode == NetworkDnsModeV1::SystemPinned
                && matches!(destination.host, NetworkEndpointHostV1::Hostname(_))
                && effect.resolution_generation_ref.is_none()
                && effect.request_digest.is_none()
        }
        NetworkVerbV1::Connect => {
            scope.kind == destination.expected_scope
                && scope.kind != NetworkScopeKindV1::NameResolution
                && (matches!(destination.host, NetworkEndpointHostV1::Literal(_))
                    || effect.resolution_generation_ref.is_some())
        }
        NetworkVerbV1::Bind => {
            matches!(
                scope.kind,
                NetworkScopeKindV1::Loopback | NetworkScopeKindV1::Lan
            ) && scope.kind == destination.expected_scope
                && matches!(destination.host, NetworkEndpointHostV1::Literal(_))
                && effect.resolution_generation_ref.is_none()
                && effect.request_digest.is_none()
        }
    };
    if !valid {
        return invalid("Network verb, DNS, and destination scope are not independently aligned.");
    }
    Ok(())
}

fn validate_network_budget_shape(
    request: &EffectRequestV1,
    exchange: Option<&StagedNetworkExchangeV1>,
) -> AppResult<()> {
    let EffectRequestKindV1::Network(effect) = &request.effect else {
        return invalid("Expected network budget accounting.");
    };
    let budget = request.requested_budget_slice;
    if budget.requests != 1 || budget.network_time_millis == 0 {
        return invalid("Network effect requires one request and a non-zero time budget.");
    }
    let valid = match effect.verb {
        NetworkVerbV1::Resolve => {
            budget.network_resolutions == 1
                && budget.network_connections == 0
                && budget.network_binds == 0
                && budget.network_requests == 0
                && budget.network_bytes == 0
                && exchange.is_none()
        }
        NetworkVerbV1::Connect => {
            let (requests, bytes) = exchange.map_or((0, 0), |exchange| {
                (
                    1,
                    exchange.request_bytes.len() as u64 + exchange.max_response_bytes,
                )
            });
            budget.network_resolutions == 0
                && budget.network_connections == 1
                && budget.network_binds == 0
                && budget.network_requests == requests
                && budget.network_bytes >= bytes
        }
        NetworkVerbV1::Bind => {
            budget.network_resolutions == 0
                && budget.network_connections == 0
                && budget.network_binds == 1
                && budget.network_requests == 0
                && budget.network_bytes == 0
                && exchange.is_none()
        }
    };
    if !valid {
        return invalid("Network budget slice is inconsistent with the exact broker verb.");
    }
    Ok(())
}

fn validate_owner_request(owner: &NetworkOwnerV1, request: &EffectRequestV1) -> AppResult<()> {
    if owner.envelope_ref != request.envelope_ref
        || owner.run_ref != request.run_control_ref
        || owner.context_ref != request.context.context_ref()?
        || owner.host_ref != request.context.host_ref
        || owner.bridge_id != request.context.bridge_id
        || owner.session_binding_ref != request.context.session_binding_ref
    {
        return invalid("Brokered network request owner context was substituted.");
    }
    Ok(())
}

fn validate_endpoint_host(host: &NetworkEndpointHostV1) -> AppResult<()> {
    if let NetworkEndpointHostV1::Hostname(hostname) = host {
        let normalized = hostname.trim().to_ascii_lowercase();
        if normalized != *hostname
            || normalized.is_empty()
            || normalized.len() > MAX_HOSTNAME_BYTES
            || normalized.starts_with('.')
            || normalized.ends_with('.')
            || normalized.contains('/')
            || normalized.contains('@')
            || normalized.contains(':')
            || normalized.chars().any(char::is_control)
            || normalized.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|byte| byte == b'-' || byte.is_ascii_alphanumeric())
            })
        {
            return invalid("Network hostname is not normalized or bounded.");
        }
    }
    Ok(())
}

fn classify_address(address: IpAddr) -> AppResult<NetworkScopeKindV1> {
    if let IpAddr::V6(address) = address {
        if let Some(mapped) = address.to_ipv4_mapped() {
            return classify_address(IpAddr::V4(mapped));
        }
    }
    if address.is_unspecified() || address.is_multicast() {
        return invalid("Unspecified and multicast endpoints are unavailable.");
    }
    if match address {
        IpAddr::V4(address) => address == Ipv4Addr::BROADCAST || is_ipv4_special(address),
        IpAddr::V6(address) => is_ipv6_special(address),
    } {
        return invalid("Broadcast, documentation, and special-use endpoints are unavailable.");
    }
    if address.is_loopback() {
        return Ok(NetworkScopeKindV1::Loopback);
    }
    let lan = match address {
        IpAddr::V4(address) => address.is_private() || address.is_link_local(),
        IpAddr::V6(address) => is_ipv6_unique_local(address) || address.is_unicast_link_local(),
    };
    Ok(if lan {
        NetworkScopeKindV1::Lan
    } else {
        NetworkScopeKindV1::Internet
    })
}

fn is_ipv6_unique_local(address: Ipv6Addr) -> bool {
    address.octets()[0] & 0xfe == 0xfc
}

fn is_ipv4_special(address: Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    first == 0
        || (first == 100 && (64..=127).contains(&second))
        || (first == 192 && second == 0 && third == 0)
        || (first == 192 && second == 0 && third == 2)
        || (first == 198 && (second == 18 || second == 19))
        || (first == 198 && second == 51 && third == 100)
        || (first == 203 && second == 0 && third == 113)
        || first >= 224
}

fn is_ipv6_special(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    (segments[0] == 0x2001 && segments[1] == 0x0db8) || (segments[0] & 0xffc0) == 0xfec0
}

fn validate_resolved_addresses(
    addresses: Vec<SocketAddr>,
    expected: NetworkScopeKindV1,
    port: u16,
) -> AppResult<Vec<SocketAddr>> {
    let mut addresses = addresses
        .into_iter()
        .filter(|address| address.port() == port)
        .collect::<Vec<_>>();
    addresses.sort();
    addresses.dedup();
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| classify_address(address.ip()).ok() != Some(expected))
    {
        return invalid("Resolved destination crossed or mixed network scopes.");
    }
    Ok(addresses)
}

fn resolve_with_timeout(
    resolver: Arc<dyn HostNameResolverV1>,
    hostname: &str,
    port: u16,
    timeout: Duration,
) -> AppResult<Vec<SocketAddr>> {
    let hostname = hostname.to_owned();
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("pastey-network-resolve".into())
        .spawn(move || {
            let _ = sender.send(resolver.resolve(&hostname, port));
        })?;
    receiver
        .recv_timeout(timeout)
        .map_err(|_| AppError::InvalidInput("Brokered name resolution timed out.".into()))?
        .map_err(|_| AppError::InvalidInput("Brokered name resolution failed.".into()))
}

fn network_timeout(request: &EffectRequestV1) -> AppResult<Duration> {
    let millis = request.requested_budget_slice.network_time_millis;
    if millis == 0 {
        return invalid("Network time budget is empty.");
    }
    Ok(Duration::from_millis(millis))
}

fn scope_descriptor_digest(
    kind: NetworkScopeKindV1,
    verbs: &BTreeSet<NetworkVerbV1>,
    transports: &BTreeSet<NetworkTransportV1>,
    dns: NetworkDnsModeV1,
    proxy: NetworkProxyModeV1,
    expires_at: i64,
) -> AppResult<String> {
    domain_hash(
        "pastey-network-scope-descriptor-v1",
        &(kind, verbs, transports, dns, proxy, expires_at),
    )
}

fn destination_descriptor_digest(
    host: &NetworkEndpointHostV1,
    port: u16,
    transport: NetworkTransportV1,
    expected_scope: NetworkScopeKindV1,
    expires_at: i64,
) -> AppResult<String> {
    domain_hash(
        "pastey-network-destination-descriptor-v1",
        &(host, port, transport, expected_scope, expires_at),
    )
}

fn resolution_generation(destination_ref: &str, addresses: &[SocketAddr]) -> AppResult<String> {
    domain_hash(
        "pastey-network-resolution-generation-v1",
        &(destination_ref, addresses),
    )
}

fn endpoint_ref(address: SocketAddr) -> AppResult<String> {
    domain_hash("pastey-network-endpoint-evidence-v1", &address.to_string())
}

fn endpoint_refs(addresses: &[SocketAddr]) -> AppResult<Vec<String>> {
    addresses.iter().copied().map(endpoint_ref).collect()
}

fn action_ref(request: &EffectRequestV1) -> AppResult<String> {
    domain_hash(
        "pastey-network-action-v1",
        &(
            request.run_control_ref.as_str(),
            request.request_id.as_str(),
            request.sequence,
        ),
    )
}

#[allow(clippy::too_many_arguments)]
fn network_outcome(
    request: &EffectRequestV1,
    scope: &NetworkScopeBindingSpecV1,
    destination: &NetworkDestinationBindingSpecV1,
    state: &str,
    endpoint_refs: Vec<String>,
    generation: Option<String>,
    local_endpoint_ref: Option<String>,
    bytes_sent: u64,
    bytes_received: u64,
    elapsed_millis: u64,
    dns_revalidated: bool,
    closed: bool,
    summary: &str,
) -> AppResult<BackendEffectOutcomeV1> {
    Ok(network_outcome_with_action(
        request,
        scope,
        destination,
        action_ref(request)?,
        state,
        endpoint_refs,
        generation,
        local_endpoint_ref,
        bytes_sent,
        bytes_received,
        elapsed_millis,
        dns_revalidated,
        closed,
        summary,
    ))
}

#[allow(clippy::too_many_arguments)]
fn network_outcome_with_action(
    _request: &EffectRequestV1,
    scope: &NetworkScopeBindingSpecV1,
    destination: &NetworkDestinationBindingSpecV1,
    action_ref: String,
    state: &str,
    endpoint_refs: Vec<String>,
    generation: Option<String>,
    local_endpoint_ref: Option<String>,
    bytes_sent: u64,
    bytes_received: u64,
    elapsed_millis: u64,
    dns_revalidated: bool,
    closed: bool,
    summary: &str,
) -> BackendEffectOutcomeV1 {
    BackendEffectOutcomeV1 {
        decision: EffectDecisionV1::Allowed,
        actual_effect_summary: summary.into(),
        facts: EffectFactsV1::BrokeredNetwork {
            scope_ref: scope.scope_ref.clone(),
            destination_ref: destination.destination_ref.clone(),
            action_ref,
            state: state.into(),
            scope_kind: scope.kind.label().into(),
            transport_ref: destination.transport.transport_ref().into(),
            resolved_endpoint_refs: endpoint_refs,
            resolution_generation_ref: generation,
            local_endpoint_ref,
            bytes_sent,
            bytes_received,
            elapsed_millis,
            dns_revalidated,
            proxy_ref: None,
            redirects_followed: 0,
            closed,
        },
    }
}

fn unavailable_outcome(summary: &str) -> BackendEffectOutcomeV1 {
    BackendEffectOutcomeV1 {
        decision: EffectDecisionV1::Unavailable,
        actual_effect_summary: summary.into(),
        facts: EffectFactsV1::None,
    }
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn domain_hash(domain: &str, value: &impl Serialize) -> AppResult<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    hasher.update(&serde_json::to_vec(value)?);
    Ok(format!("{domain}:{}", hasher.finalize().to_hex()))
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

fn unavailable<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        effect_authority::{
            compile_effect_envelope, execution_world_ref_for, lower_tool_request,
            AuthorityCeilingV1, AuthorityContextV1, ConfinementPropertyV1, EffectBoundV1,
            EffectBudgetsV1, EffectCapabilityV1, EffectEnvelopeCompileRequestV1, EffectEnvelopeV1,
            EffectPreconditionV1, ExecutionWorldGrantV1, ManagedInputRevisionV1,
            ManagedSemanticOperationV1, NetworkAuthorityV1, ResourceGrantSpecV1, ResourceKindV1,
            ResourceVerbV1, ResultContractV1, StepWorkDescriptorV1, ToolEffectIntentV1,
            ToolRequestV1, EFFECT_AUTHORITY_VERSION,
        },
        host_identity::{HostRef, HostSessionBinding, PlanParticipantRef},
        host_runtime::{DeveloperHostRef, DeveloperTerminalBinding},
    };
    use std::net::Ipv4Addr;

    struct BrokerFixture {
        state: EffectAuthorityStateV1,
        envelope: EffectEnvelopeV1,
        context: AuthorityContextV1,
        current: CurrentHostAuthorityV1,
        broker: NetworkBrokerServiceV1,
        name_scope: NetworkScopeRefV1,
        loopback_scope: NetworkScopeRefV1,
        lan_scope: NetworkScopeRefV1,
        internet_scope: NetworkScopeRefV1,
        loopback_connect: String,
        loopback_bind: String,
        localhost_name: String,
        lan_destination: String,
        internet_destination: String,
    }

    fn total_budget() -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests: 16,
            network_resolutions: 4,
            network_connections: 4,
            network_binds: 4,
            network_requests: 4,
            network_bytes: 4096,
            network_time_millis: 4_000,
            ..EffectBudgetsV1::default()
        }
    }

    fn per_request_budget() -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests: 1,
            network_resolutions: 1,
            network_connections: 1,
            network_binds: 1,
            network_requests: 1,
            network_bytes: 2048,
            network_time_millis: 1_000,
            ..EffectBudgetsV1::default()
        }
    }

    fn fixture(connect_port: u16, network: bool) -> BrokerFixture {
        fixture_with_resolver(connect_port, network, Arc::new(SystemHostNameResolverV1))
    }

    fn fixture_with_resolver(
        connect_port: u16,
        network: bool,
        resolver: Arc<dyn HostNameResolverV1>,
    ) -> BrokerFixture {
        let local = HostRef::from_device_id("phase5-network-local").unwrap();
        let peer = HostRef::from_device_id("phase5-network-peer").unwrap();
        let binding = HostSessionBinding::new(
            "bridge-phase5-network",
            local.clone(),
            peer,
            "local-network-session",
            "peer-network-session",
            "peer-network-route",
            10_000,
        )
        .unwrap();
        let context = AuthorityContextV1 {
            contract_version: EFFECT_AUTHORITY_VERSION.into(),
            bridge_id: "bridge-phase5-network".into(),
            plan_id: "plan-phase5-network".into(),
            revision_id: "revision-phase5-network".into(),
            revision_hash: "revision-hash-phase5-network".into(),
            approval_id: "approval-phase5-network".into(),
            attempt_id: "attempt-phase5-network".into(),
            step_id: "transform-phase5-network".into(),
            semantic_operation: ManagedSemanticOperationV1::Transform,
            participant_ref: PlanParticipantRef::for_host("plan-phase5-network", &local).unwrap(),
            host_ref: local.clone(),
            admission_ref: "admission-phase5-network".into(),
            session_binding_ref: binding.binding_ref.clone(),
            input_revisions: vec![ManagedInputRevisionV1 {
                logical_object_id: "object-phase5-network".into(),
                revision: 1,
                host_ref: local.clone(),
            }],
            issued_at: 100,
            expires_at: 9_000,
        };
        let mut state = EffectAuthorityStateV1::default();
        let draft = state.begin_run(context.clone()).unwrap();
        let output = state
            .mint_resource_grant(
                &draft,
                ResourceGrantSpecV1 {
                    host_ref: local.clone(),
                    kind: ResourceKindV1::OutputSlot,
                    safe_identity_ref: "safe-output:v1:network-fixture".into(),
                    selector_prefix: ".".into(),
                    allowed_verbs: [ResourceVerbV1::Create].into_iter().collect(),
                    budgets: total_budget(),
                    expires_at: 9_000,
                },
            )
            .unwrap();
        let name = NetworkScopeBindingSpecV1::new(
            &draft,
            NetworkScopeKindV1::NameResolution,
            [NetworkVerbV1::Resolve].into_iter().collect(),
            [NetworkTransportV1::Tcp].into_iter().collect(),
            NetworkDnsModeV1::SystemPinned,
            9_000,
        )
        .unwrap();
        let loopback = NetworkScopeBindingSpecV1::new(
            &draft,
            NetworkScopeKindV1::Loopback,
            [NetworkVerbV1::Connect, NetworkVerbV1::Bind]
                .into_iter()
                .collect(),
            [NetworkTransportV1::Tcp].into_iter().collect(),
            NetworkDnsModeV1::Denied,
            9_000,
        )
        .unwrap();
        let lan = NetworkScopeBindingSpecV1::new(
            &draft,
            NetworkScopeKindV1::Lan,
            [NetworkVerbV1::Connect, NetworkVerbV1::Bind]
                .into_iter()
                .collect(),
            [NetworkTransportV1::Tcp].into_iter().collect(),
            NetworkDnsModeV1::Denied,
            9_000,
        )
        .unwrap();
        let internet = NetworkScopeBindingSpecV1::new(
            &draft,
            NetworkScopeKindV1::Internet,
            [NetworkVerbV1::Connect].into_iter().collect(),
            [NetworkTransportV1::Tcp].into_iter().collect(),
            NetworkDnsModeV1::Denied,
            9_000,
        )
        .unwrap();
        let destinations = vec![
            NetworkDestinationBindingSpecV1::new(
                &draft,
                NetworkEndpointHostV1::Literal(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                connect_port,
                NetworkTransportV1::Tcp,
                NetworkScopeKindV1::Loopback,
                9_000,
            )
            .unwrap(),
            NetworkDestinationBindingSpecV1::new(
                &draft,
                NetworkEndpointHostV1::Literal(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                0,
                NetworkTransportV1::Tcp,
                NetworkScopeKindV1::Loopback,
                9_000,
            )
            .unwrap(),
            NetworkDestinationBindingSpecV1::new(
                &draft,
                NetworkEndpointHostV1::Hostname("localhost".into()),
                connect_port,
                NetworkTransportV1::Tcp,
                NetworkScopeKindV1::Loopback,
                9_000,
            )
            .unwrap(),
            NetworkDestinationBindingSpecV1::new(
                &draft,
                NetworkEndpointHostV1::Literal(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))),
                443,
                NetworkTransportV1::Tcp,
                NetworkScopeKindV1::Lan,
                9_000,
            )
            .unwrap(),
            NetworkDestinationBindingSpecV1::new(
                &draft,
                NetworkEndpointHostV1::Literal(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))),
                443,
                NetworkTransportV1::Tcp,
                NetworkScopeKindV1::Internet,
                9_000,
            )
            .unwrap(),
        ];
        let scopes = vec![
            name.clone(),
            loopback.clone(),
            lan.clone(),
            internet.clone(),
        ];
        let grant = NetworkGrantV1 {
            context_ref: draft.context_ref.clone(),
            run_control_ref: draft.run_control_ref.clone(),
            host_ref: local.clone(),
            scope_refs: scopes.iter().map(|scope| scope.scope_ref.clone()).collect(),
            allowed_verbs: [
                NetworkVerbV1::Resolve,
                NetworkVerbV1::Connect,
                NetworkVerbV1::Bind,
            ]
            .into_iter()
            .collect(),
            destination_refs: destinations
                .iter()
                .map(|destination| destination.destination_ref.clone())
                .collect(),
            budgets: total_budget(),
            expires_at: 9_000,
        };
        let world = ExecutionWorldGrantV1 {
            world_ref: execution_world_ref_for(&draft, "network-test-world:v1").unwrap(),
            context_ref: draft.context_ref.clone(),
            run_control_ref: draft.run_control_ref.clone(),
            world_identity_digest: "network-test-world:v1".into(),
            mounted_resources: BTreeSet::new(),
            executable_resources: BTreeSet::new(),
            required_properties: [ConfinementPropertyV1::NoRawNetwork].into_iter().collect(),
            budgets: total_budget(),
            expires_at: 9_000,
        };
        let ceiling = AuthorityCeilingV1 {
            context_ref: draft.context_ref.clone(),
            source_snapshot_ref: "network-authority-source:v1".into(),
            resources: vec![output.clone()],
            world,
            effect_bounds: [
                NetworkVerbV1::Resolve,
                NetworkVerbV1::Connect,
                NetworkVerbV1::Bind,
            ]
            .into_iter()
            .map(|verb| EffectBoundV1 {
                capability: EffectCapabilityV1::Network(verb),
                max_per_request: per_request_budget(),
            })
            .collect(),
            budgets: total_budget(),
            network: if network {
                NetworkAuthorityV1::Scoped(Box::new(grant))
            } else {
                NetworkAuthorityV1::Denied
            },
            expires_at: 9_000,
        };
        let envelope = compile_effect_envelope(EffectEnvelopeCompileRequestV1 {
            context: context.clone(),
            run_control_ref: draft.run_control_ref.clone(),
            semantic_ceiling: ceiling.clone(),
            admission_ceiling: ceiling.clone(),
            host_policy_ceiling: ceiling.clone(),
            confinement_ceiling: ceiling,
            host_policy_snapshot_ref: "network-host-policy:v1".into(),
            result_contract: ResultContractV1::Transform {
                input: context.input_revisions[0].clone(),
                output_revision: 2,
                output_slot: output.handle_ref,
            },
        })
        .unwrap();
        state.install_envelope(draft, envelope.clone()).unwrap();
        state.activate_run(&envelope.run_control_ref, 110).unwrap();
        let current = CurrentHostAuthorityV1 {
            session_binding: binding,
            bridge_active: true,
            burned: false,
            disconnected: false,
            restarted: false,
            now: 110,
        };
        let broker = NetworkBrokerServiceV1 {
            state: Mutex::new(NetworkBrokerStateV1::default()),
            resolver,
        };
        if network {
            broker
                .provision_grant(
                    &state,
                    NetworkBrokerAccessV1 {
                        envelope_ref: envelope.envelope_ref.clone(),
                        run_control_ref: envelope.run_control_ref.clone(),
                        context: context.clone(),
                        current: current.clone(),
                    },
                    scopes,
                    destinations.clone(),
                )
                .unwrap();
        }
        BrokerFixture {
            state,
            envelope,
            context,
            current,
            broker,
            name_scope: name.scope_ref,
            loopback_scope: loopback.scope_ref,
            lan_scope: lan.scope_ref,
            internet_scope: internet.scope_ref,
            loopback_connect: destinations[0].destination_ref.clone(),
            loopback_bind: destinations[1].destination_ref.clone(),
            localhost_name: destinations[2].destination_ref.clone(),
            lan_destination: destinations[3].destination_ref.clone(),
            internet_destination: destinations[4].destination_ref.clone(),
        }
    }

    fn request(
        fixture: &BrokerFixture,
        sequence: u64,
        effect: NetworkEffectV1,
        budget: EffectBudgetsV1,
        preconditions: Vec<EffectPreconditionV1>,
    ) -> EffectRequestV1 {
        lower_tool_request(
            &StepWorkDescriptorV1 {
                contract_version: EFFECT_AUTHORITY_VERSION.into(),
                context: fixture.context.clone(),
                envelope_ref: fixture.envelope.envelope_ref.clone(),
                run_control_ref: fixture.envelope.run_control_ref.clone(),
                first_sequence: sequence,
            },
            &ToolRequestV1 {
                tool_name: "non-authoritative-synthetic-network-tool".into(),
                adapter_version_ref: "network-test-adapter:v1".into(),
                intents: vec![ToolEffectIntentV1 {
                    effect: EffectRequestKindV1::Network(effect),
                    requested_budget_slice: budget,
                    preconditions,
                }],
            },
        )
        .unwrap()
        .remove(0)
    }

    fn resolve_budget() -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests: 1,
            network_resolutions: 1,
            network_time_millis: 500,
            ..EffectBudgetsV1::default()
        }
    }

    fn connect_budget(bytes: u64, has_request: bool) -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests: 1,
            network_connections: 1,
            network_requests: u64::from(has_request),
            network_bytes: bytes,
            network_time_millis: 500,
            ..EffectBudgetsV1::default()
        }
    }

    fn bind_budget() -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests: 1,
            network_binds: 1,
            network_time_millis: 500,
            ..EffectBudgetsV1::default()
        }
    }

    fn enforce(
        fixture: &mut BrokerFixture,
        request: &EffectRequestV1,
        exchange: Option<StagedNetworkExchangeV1>,
    ) -> crate::effect_authority::EffectEvidenceV1 {
        fixture
            .broker
            .prepare_request(&fixture.state, request, &fixture.current, exchange)
            .unwrap();
        fixture
            .state
            .enforce(
                request,
                &fixture.current,
                &mut HostManagedNetworkBackendV1::new(&fixture.broker),
            )
            .unwrap()
    }

    #[test]
    fn real_broker_resolve_connect_bind_and_ordered_evidence_share_one_contract() {
        let Ok(server) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else {
            eprintln!("native loopback sockets are unavailable in the enclosing sandbox");
            return;
        };
        let port = server.local_addr().unwrap().port();
        let server_thread = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            let mut request = [0_u8; 4];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").unwrap();
        });
        let mut fixture = fixture(port, true);
        let resolve = request(
            &fixture,
            0,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Resolve,
                scope_ref: fixture.name_scope.clone(),
                destination_ref: fixture.localhost_name.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: None,
            },
            resolve_budget(),
            Vec::new(),
        );
        let resolved = enforce(&mut fixture, &resolve, None);
        let generation = match resolved.facts {
            EffectFactsV1::BrokeredNetwork {
                resolution_generation_ref: Some(generation),
                redirects_followed,
                proxy_ref,
                ..
            } => {
                assert_eq!(redirects_followed, 0);
                assert!(proxy_ref.is_none());
                generation
            }
            _ => panic!("expected brokered resolution evidence"),
        };
        let exchange = StagedNetworkExchangeV1 {
            request_bytes: b"ping".to_vec(),
            max_response_bytes: 4,
        };
        let connect = request(
            &fixture,
            1,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Connect,
                scope_ref: fixture.loopback_scope.clone(),
                destination_ref: fixture.loopback_connect.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: Some(exchange.request_digest().unwrap()),
            },
            connect_budget(8, true),
            Vec::new(),
        );
        let connected = enforce(&mut fixture, &connect, Some(exchange));
        assert!(matches!(
            connected.facts,
            EffectFactsV1::BrokeredNetwork {
                bytes_sent: 4,
                bytes_received: 4,
                redirects_followed: 0,
                closed: true,
                ..
            }
        ));
        let bind = request(
            &fixture,
            2,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Bind,
                scope_ref: fixture.loopback_scope.clone(),
                destination_ref: fixture.loopback_bind.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: None,
            },
            bind_budget(),
            Vec::new(),
        );
        let bound = enforce(&mut fixture, &bind, None);
        assert!(matches!(
            bound.facts,
            EffectFactsV1::BrokeredNetwork {
                local_endpoint_ref: Some(_),
                closed: false,
                ..
            }
        ));
        fixture
            .state
            .validate_evidence_chain(&fixture.envelope.run_control_ref)
            .unwrap();
        assert_eq!(
            fixture
                .state
                .validate_network_attachment(
                    &fixture.loopback_scope,
                    &fixture.loopback_connect,
                    &fixture.envelope.envelope_ref,
                    &fixture.envelope.run_control_ref,
                    &fixture.context,
                    &fixture.current,
                )
                .unwrap()
                .host_ref,
            fixture.context.host_ref
        );
        assert_eq!(
            generation.split(':').next(),
            Some("pastey-network-resolution-generation-v1")
        );
        server_thread.join().unwrap();
    }

    #[test]
    fn default_deny_and_scope_destination_substitution_fail_before_socket_access() {
        assert_eq!(
            classify_address(IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped())).unwrap(),
            NetworkScopeKindV1::Loopback
        );
        assert!(classify_address(IpAddr::V4(Ipv4Addr::BROADCAST)).is_err());
        let denied = fixture(9, false);
        assert!(denied
            .state
            .validate_network_attachment(
                &denied.loopback_scope,
                &denied.loopback_connect,
                &denied.envelope.envelope_ref,
                &denied.envelope.run_control_ref,
                &denied.context,
                &denied.current,
            )
            .is_err());

        let fixture = fixture(9, true);
        for (scope, destination) in [
            (
                fixture.internet_scope.clone(),
                fixture.loopback_connect.clone(),
            ),
            (
                fixture.loopback_scope.clone(),
                fixture.lan_destination.clone(),
            ),
            (
                fixture.lan_scope.clone(),
                fixture.internet_destination.clone(),
            ),
            (fixture.name_scope.clone(), fixture.loopback_connect.clone()),
        ] {
            let request = request(
                &fixture,
                0,
                NetworkEffectV1 {
                    verb: NetworkVerbV1::Connect,
                    scope_ref: scope,
                    destination_ref: destination,
                    transport_ref: TCP_TRANSPORT_REF.into(),
                    resolution_generation_ref: None,
                    request_digest: None,
                },
                connect_budget(0, false),
                Vec::new(),
            );
            assert!(fixture
                .broker
                .prepare_request(&fixture.state, &request, &fixture.current, None)
                .is_err());
        }

        let valid = request(
            &fixture,
            0,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Connect,
                scope_ref: fixture.loopback_scope.clone(),
                destination_ref: fixture.loopback_connect.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: None,
            },
            connect_budget(0, false),
            Vec::new(),
        );
        let mut wrong_host = valid.clone();
        wrong_host.context.host_ref = HostRef::from_device_id("substituted-network-host").unwrap();
        wrong_host.context.participant_ref =
            PlanParticipantRef::for_host(&wrong_host.context.plan_id, &wrong_host.context.host_ref)
                .unwrap();
        for input in &mut wrong_host.context.input_revisions {
            input.host_ref = wrong_host.context.host_ref.clone();
        }
        assert!(fixture
            .broker
            .prepare_request(&fixture.state, &wrong_host, &fixture.current, None)
            .is_err());
    }

    #[test]
    fn connection_request_byte_and_time_budget_shapes_fail_closed() {
        let fixture = fixture(9, true);
        let exchange = StagedNetworkExchangeV1 {
            request_bytes: b"hello".to_vec(),
            max_response_bytes: 5,
        };
        let digest = exchange.request_digest().unwrap();
        let cases = [
            EffectBudgetsV1 {
                requests: 1,
                network_time_millis: 100,
                ..EffectBudgetsV1::default()
            },
            EffectBudgetsV1 {
                requests: 1,
                network_connections: 1,
                network_bytes: 10,
                network_time_millis: 100,
                ..EffectBudgetsV1::default()
            },
            EffectBudgetsV1 {
                requests: 1,
                network_connections: 1,
                network_requests: 1,
                network_bytes: 9,
                network_time_millis: 100,
                ..EffectBudgetsV1::default()
            },
            EffectBudgetsV1 {
                requests: 1,
                network_connections: 1,
                network_requests: 1,
                network_bytes: 10,
                network_time_millis: 0,
                ..EffectBudgetsV1::default()
            },
        ];
        for budget in cases {
            let request = request(
                &fixture,
                0,
                NetworkEffectV1 {
                    verb: NetworkVerbV1::Connect,
                    scope_ref: fixture.loopback_scope.clone(),
                    destination_ref: fixture.loopback_connect.clone(),
                    transport_ref: TCP_TRANSPORT_REF.into(),
                    resolution_generation_ref: None,
                    request_digest: Some(digest.clone()),
                },
                budget,
                Vec::new(),
            );
            assert!(fixture
                .broker
                .prepare_request(
                    &fixture.state,
                    &request,
                    &fixture.current,
                    Some(exchange.clone()),
                )
                .is_err());
        }
    }

    struct SequenceResolver {
        answers: Mutex<Vec<Vec<SocketAddr>>>,
    }

    struct StaticResolver {
        address: SocketAddr,
    }

    impl HostNameResolverV1 for StaticResolver {
        fn availability(&self) -> NetworkBrokerAvailabilityV1 {
            NetworkBrokerAvailabilityV1 {
                available: true,
                identity_digest: "test-static-resolver:v1".into(),
                unavailable_reason: None,
            }
        }

        fn resolve(&self, _hostname: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
            Ok(vec![self.address])
        }
    }

    impl HostNameResolverV1 for SequenceResolver {
        fn availability(&self) -> NetworkBrokerAvailabilityV1 {
            NetworkBrokerAvailabilityV1 {
                available: true,
                identity_digest: "test-sequence-resolver:v1".into(),
                unavailable_reason: None,
            }
        }

        fn resolve(&self, _hostname: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
            Ok(self.answers.lock().remove(0))
        }
    }

    #[test]
    fn dns_rebinding_and_changed_resolution_are_denied_before_connect() {
        let port = 4242;
        let resolver = Arc::new(SequenceResolver {
            answers: Mutex::new(vec![
                vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)],
                vec![SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)),
                    port,
                )],
            ]),
        });
        let mut fixture = fixture_with_resolver(port, true, resolver);
        let resolve = request(
            &fixture,
            0,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Resolve,
                scope_ref: fixture.name_scope.clone(),
                destination_ref: fixture.localhost_name.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: None,
            },
            resolve_budget(),
            Vec::new(),
        );
        let evidence = enforce(&mut fixture, &resolve, None);
        let generation = match evidence.facts {
            EffectFactsV1::BrokeredNetwork {
                resolution_generation_ref: Some(value),
                ..
            } => value,
            _ => panic!("expected resolution generation"),
        };
        let connect = request(
            &fixture,
            1,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Connect,
                scope_ref: fixture.loopback_scope.clone(),
                destination_ref: fixture.localhost_name.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: Some(generation.clone()),
                request_digest: None,
            },
            connect_budget(0, false),
            vec![EffectPreconditionV1::DestinationGeneration {
                scope_ref: fixture.name_scope.clone(),
                generation_ref: generation,
            }],
        );
        let denied = enforce(&mut fixture, &connect, None);
        assert_eq!(denied.decision, EffectDecisionV1::Denied);
        assert!(fixture.broker.state.lock().connections.is_empty());
    }

    #[test]
    fn redirects_and_new_endpoints_never_inherit_destination_authority() {
        let Ok(target) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else {
            eprintln!("native loopback sockets are unavailable in the enclosing sandbox");
            return;
        };
        let target_port = target.local_addr().unwrap().port();
        let Ok(redirector) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else {
            eprintln!("native loopback sockets are unavailable in the enclosing sandbox");
            return;
        };
        let redirect_port = redirector.local_addr().unwrap().port();
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = redirector.accept().unwrap();
            let mut request = [0_u8; 3];
            stream.read_exact(&mut request).unwrap();
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/\r\nContent-Length: 0\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let mut fixture = fixture(redirect_port, true);
        let exchange = StagedNetworkExchangeV1 {
            request_bytes: b"get".to_vec(),
            max_response_bytes: 256,
        };
        let connect = request(
            &fixture,
            0,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Connect,
                scope_ref: fixture.loopback_scope.clone(),
                destination_ref: fixture.loopback_connect.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: Some(exchange.request_digest().unwrap()),
            },
            connect_budget(259, true),
            Vec::new(),
        );
        let evidence = enforce(&mut fixture, &connect, Some(exchange));
        assert!(matches!(
            evidence.facts,
            EffectFactsV1::BrokeredNetwork {
                redirects_followed: 0,
                closed: true,
                ..
            }
        ));
        let ungranted_destination = format!("network-destination-ungranted:{target_port}");
        let follow = request(
            &fixture,
            1,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Connect,
                scope_ref: fixture.loopback_scope.clone(),
                destination_ref: ungranted_destination,
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: None,
            },
            connect_budget(0, false),
            Vec::new(),
        );
        assert!(fixture
            .broker
            .prepare_request(&fixture.state, &follow, &fixture.current, None)
            .is_err());
        target.set_nonblocking(true).unwrap();
        assert!(target.accept().is_err());
        thread.join().unwrap();
    }

    #[test]
    fn expiry_reconciliation_closes_retained_connections_and_listeners() {
        let Ok(server) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else {
            eprintln!("native loopback sockets are unavailable in the enclosing sandbox");
            return;
        };
        let port = server.local_addr().unwrap().port();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut byte = [0_u8; 1];
            matches!(stream.read(&mut byte), Ok(0) | Err(_))
        });
        let mut fixture = fixture(port, true);
        let connect = request(
            &fixture,
            0,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Connect,
                scope_ref: fixture.loopback_scope.clone(),
                destination_ref: fixture.loopback_connect.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: None,
            },
            connect_budget(0, false),
            Vec::new(),
        );
        assert!(matches!(
            enforce(&mut fixture, &connect, None).facts,
            EffectFactsV1::BrokeredNetwork { closed: false, .. }
        ));
        let bind = request(
            &fixture,
            1,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Bind,
                scope_ref: fixture.loopback_scope.clone(),
                destination_ref: fixture.loopback_bind.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: None,
            },
            bind_budget(),
            Vec::new(),
        );
        assert_eq!(
            enforce(&mut fixture, &bind, None).decision,
            EffectDecisionV1::Allowed
        );
        assert_eq!(fixture.broker.state.lock().connections.len(), 1);
        assert_eq!(fixture.broker.state.lock().listeners.len(), 1);
        assert_eq!(
            fixture.broker.reconcile_expired(fixture.context.expires_at),
            1
        );
        assert!(fixture.broker.state.lock().connections.is_empty());
        assert!(fixture.broker.state.lock().listeners.is_empty());
        assert!(peer.join().unwrap());
    }

    #[test]
    fn budget_exhaustion_replay_and_cross_run_grants_fail_closed() {
        let port = 9;
        let mut primary = fixture_with_resolver(
            port,
            true,
            Arc::new(StaticResolver {
                address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            }),
        );
        let mut first = None;
        for sequence in 0..5 {
            let next = request(
                &primary,
                sequence,
                NetworkEffectV1 {
                    verb: NetworkVerbV1::Resolve,
                    scope_ref: primary.name_scope.clone(),
                    destination_ref: primary.localhost_name.clone(),
                    transport_ref: TCP_TRANSPORT_REF.into(),
                    resolution_generation_ref: None,
                    request_digest: None,
                },
                resolve_budget(),
                Vec::new(),
            );
            let evidence = enforce(&mut primary, &next, None);
            if sequence < 4 {
                assert_eq!(evidence.decision, EffectDecisionV1::Allowed);
            } else {
                assert_eq!(evidence.decision, EffectDecisionV1::Denied);
            }
            if sequence == 0 {
                first = Some(next);
            }
        }
        let first = first.unwrap();
        assert!(primary
            .broker
            .prepare_request(&primary.state, &first, &primary.current, None)
            .is_err());

        let other = fixture(10, true);
        let cross_run = request(
            &other,
            0,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Resolve,
                scope_ref: other.name_scope.clone(),
                destination_ref: other.localhost_name.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: None,
            },
            resolve_budget(),
            Vec::new(),
        );
        assert!(primary
            .broker
            .prepare_request(&other.state, &cross_run, &other.current, None)
            .is_err());

        let oversized = StagedNetworkExchangeV1 {
            request_bytes: vec![0; 5],
            max_response_bytes: 5,
        };
        let malformed = request(
            &other,
            0,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Connect,
                scope_ref: other.loopback_scope.clone(),
                destination_ref: other.loopback_connect.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: Some(oversized.request_digest().unwrap()),
            },
            connect_budget(9, true),
            Vec::new(),
        );
        assert!(other
            .broker
            .prepare_request(&other.state, &malformed, &other.current, Some(oversized))
            .is_err());
    }

    #[test]
    fn cancellation_burn_expiry_and_session_substitution_revoke_broker_state() {
        for lifecycle in ["cancel", "burn", "expiry", "session"] {
            let mut fixture = fixture(9, true);
            match lifecycle {
                "cancel" => {
                    fixture
                        .broker
                        .terminate_run(&fixture.envelope.run_control_ref);
                    fixture
                        .state
                        .cancel_run(&fixture.envelope.run_control_ref)
                        .unwrap();
                }
                "burn" => {
                    fixture.broker.terminate_bridge(&fixture.context.bridge_id);
                    fixture.state.revoke_bridge(&fixture.context.bridge_id);
                    fixture.current.burned = true;
                }
                "expiry" => fixture.current.now = fixture.context.expires_at,
                "session" => {
                    fixture.current.session_binding = HostSessionBinding::new(
                        "bridge-phase5-network",
                        fixture.context.host_ref.clone(),
                        HostRef::from_device_id("phase5-network-peer").unwrap(),
                        "substituted-local-session",
                        "substituted-peer-session",
                        "substituted-route",
                        10_000,
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
            let request = request(
                &fixture,
                0,
                NetworkEffectV1 {
                    verb: NetworkVerbV1::Bind,
                    scope_ref: fixture.loopback_scope.clone(),
                    destination_ref: fixture.loopback_bind.clone(),
                    transport_ref: TCP_TRANSPORT_REF.into(),
                    resolution_generation_ref: None,
                    request_digest: None,
                },
                bind_budget(),
                Vec::new(),
            );
            assert!(fixture
                .broker
                .prepare_request(&fixture.state, &request, &fixture.current, None)
                .is_err());
        }
    }

    struct UnavailableResolver;

    impl HostNameResolverV1 for UnavailableResolver {
        fn availability(&self) -> NetworkBrokerAvailabilityV1 {
            NetworkBrokerAvailabilityV1 {
                available: false,
                identity_digest: "network-broker-unavailable:v1".into(),
                unavailable_reason: Some("test platform cannot enforce brokerage".into()),
            }
        }

        fn resolve(&self, _hostname: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
            unreachable!("unavailable broker must not resolve")
        }
    }

    #[test]
    fn unavailable_platform_secret_separation_and_terminal_types_fail_closed() {
        let mut unavailable = fixture(9, true);
        unavailable.broker.resolver = Arc::new(UnavailableResolver);
        let resolve = request(
            &unavailable,
            0,
            NetworkEffectV1 {
                verb: NetworkVerbV1::Resolve,
                scope_ref: unavailable.name_scope.clone(),
                destination_ref: unavailable.localhost_name.clone(),
                transport_ref: TCP_TRANSPORT_REF.into(),
                resolution_generation_ref: None,
                request_digest: None,
            },
            resolve_budget(),
            Vec::new(),
        );
        let evidence = enforce(&mut unavailable, &resolve, None);
        assert_eq!(evidence.decision, EffectDecisionV1::Unavailable);
        assert!(unavailable.broker.state.lock().connections.is_empty());
        assert!(unavailable.broker.state.lock().listeners.is_empty());

        let fixture = fixture(9, true);
        let grant_json = serde_json::to_string(&fixture.envelope.network).unwrap();
        let broker_json = serde_json::to_string(
            &fixture
                .broker
                .state
                .lock()
                .grants
                .get(&fixture.envelope.run_control_ref)
                .unwrap()
                .destinations,
        )
        .unwrap();
        assert!(!grant_json.contains("secret"));
        assert!(!broker_json.contains("credential"));
        assert!(!broker_json.contains("secret"));

        let terminal = DeveloperTerminalBinding::new("bridge", "controller", "target", "route");
        assert!(HostRef::parse(terminal.controller_host.0).is_err());
        assert!(HostRef::parse(terminal.target_host.0).is_err());
        assert!(HostRef::parse(DeveloperHostRef("developer-host:v0:network".into()).0).is_err());
    }

    #[test]
    fn scoped_broker_authority_does_not_remove_execution_world_raw_network_denial() {
        let fixture = fixture(9, true);
        let (world, _) = fixture
            .state
            .validate_execution_world_attachment(
                &fixture.envelope.world.world_ref,
                &fixture.envelope.envelope_ref,
                &fixture.envelope.run_control_ref,
                &fixture.context,
                &fixture.current,
            )
            .unwrap();
        assert!(world
            .required_properties
            .contains(&ConfinementPropertyV1::NoRawNetwork));
        assert!(matches!(
            fixture.envelope.network,
            NetworkAuthorityV1::Scoped(_)
        ));
    }
}
