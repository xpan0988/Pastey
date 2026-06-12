# Hello Peer Safety Boundary

This addendum defines the minimum safety boundary for a Hello Peer v0
prototype. It is not a broad threat model and does not describe implemented
runtime behavior.

AI Slot Phase E1 can generate and evaluate an advisory plan, bind a local
confirmation to a visible canonical payload and hash, and build a validated
`HelloPeerRequest` outbound preview and `CapabilityRequestPreviewEnvelope`.
Actual room transport remains blocked. The inbound card is a local simulation,
not a peer receive path, and no peer-side execution behavior described below is
implemented.

## 1. What Does a Trusted Room Actually Trust?

A trusted room means the devices have an authenticated, current room
relationship sufficient for Pastey communication. It permits message exchange
and capability negotiation.

A trusted room does not grant execution authority. Room membership is necessary
but not sufficient for peer-side execution. Every capability request still
needs action-specific validation and peer consent. A stale, removed, or
invisible peer must not receive or execute capability requests.

**Conclusion:** trusted room = communication and identity/trust boundary, not
execution authorization.

## 2. What Counts as Peer Consent?

For Hello Peer v0, peer consent is explicit peer-side approval for one specific
request. The initial mode is **allow once** only.

Consent must bind to:

- request ID;
- target peer;
- requesting device;
- capability name;
- peer-selected runtime;
- message exactly `hello peer!`;
- execution constraints;
- expiry time.

Consent must not silently become long-lived authorization. Local user approval
on the requesting device does not replace peer approval. The peer may deny
without fallback or retry escalation.

The implemented local confirmation, E0 request preview, and E1 acknowledge
preview are therefore preparation for this boundary, not execution consent.
Acknowledging preview is not permission to execute and creates no long-lived
grant.

**Conclusion:** peer consent = specific, visible, one-time approval for a
bounded request.

## 3. How Is Request Replay Prevented?

Every request needs a unique request ID or nonce and a short expiry timestamp.
The peer must reject:

- duplicate request IDs;
- expired requests;
- requests from a non-current room or session;
- requests targeting another peer;
- requests whose capability, runtime, message, or constraints changed after
  approval.

The requesting device must revalidate peer visibility and advertised capability
immediately before dispatch. The peer should keep a current-session replay cache
only. Hello Peer v0 does not require a long-term hidden replay database.

Phase E1 adds current-session duplicate detection for envelope IDs and embedded
request IDs. It has no long-term database and no real peer replay cache because
actual transport is not implemented. Full replay prevention remains future
transport and peer-side work.

**Conclusion:** replay defense = request ID + expiry + current-session binding +
peer replay cache.

## 4. What Is the Minimum Executor Isolation?

The Hello Peer v0 executor:

- is not raw shell;
- is not arbitrary code execution;
- runs a fixed internal hello template only;
- permits only the exact output message `hello peer!`;
- accepts no command string from the model or requesting device;
- lets the peer choose the local safe runtime implementation;
- has no filesystem access, except an optional temp-only sandbox if strictly
  required by a reviewed implementation;
- has no network access;
- enforces a short timeout;
- caps stdout and stderr sizes;
- sanitizes stderr and error output to prevent disclosure of paths, environment
  variables, commands, or secrets.

If platform isolation is not good enough, execution must fail closed.

**Conclusion:** minimum isolation = fixed template, no shell, no arbitrary code,
no filesystem, no network, bounded output, and timeout.

## 5. How Can Audit Coexist With Pastey's Low-Trace/No-Hidden-History Principle?

Hello Peer needs a visible current-session action record so users can see what
was requested, approved, denied, executed, and returned. This record provides
accountability and must not become hidden behavioral history.

A v0 current-session record may include:

- request ID;
- requesting device;
- target peer;
- capability;
- status;
- timestamp;
- bounded stdout or status.

It must not include:

- API keys;
- room keys or codes;
- raw provider prompts;
- raw logs;
- the full environment;
- filesystem paths;
- a hidden persistent profile.

Hello Peer v0 should not create long-term hidden logs. Persistent audit
retention is a later product and security decision. If added later, persistence
must be explicit, visible, and user-controllable.

**Conclusion:** audit = visible current-session accountability, not hidden
long-term tracking.

## 6. Minimum Safe Hello Peer v0 Rule

Hello Peer v0 is allowed only if all are true:

- a current trusted room exists;
- the peer is current, visible, and trusted;
- the peer advertises `runtime.execute_hello_template`;
- model output validates as `request_peer_hello_demo`;
- the local `PolicyGate` accepts it;
- the local user confirms;
- the request has an ID or nonce and expiry;
- the peer `PolicyGate` accepts it;
- the peer user confirms once;
- the executor runs the fixed template only;
- output is bounded and sanitized.

Any ambiguity must fail closed.
