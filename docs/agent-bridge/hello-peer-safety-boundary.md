# Hello Peer Safety Boundary

This addendum defines the minimum safety boundary for a Hello Peer v0
prototype. It is not a broad threat model. CL-5 implements receiver-side
PolicyGate and explicit one-time consent. CL-6 implements only the fixed
host-owned `runtime.execute_hello_template` function and its bounded typed
request/result flow.

AI Slot Phase E1 can generate and evaluate an advisory plan, bind a local
confirmation to a visible canonical payload and hash, and build a validated
`HelloPeerRequest` outbound preview and `CapabilityRequestPreviewEnvelope`.
CL-3B provides preview-only room-control transport delivery and a bounded
received inbox. CL-3C integrates it with the current-session queue, CL-4
reserves sender-side runtime capacity, CL-5 adds explicit receiver
allow-once/deny review, and CL-6 consumes an exact allow-once record before one
fixed in-process execution. Transport delivery and trusted-room membership are
not execution authorization.

## 1. What Does a Trusted Room Actually Trust?

A trusted room means the devices have a current joined room relationship
sufficient for existing Pastey communication. Current source exchanges
ephemeral room-server transport public keys during join and encrypts item/file
payloads, but the room server uses plain HTTP and does not provide a generic
authenticated connection or durable device identity. A future room-control
transport therefore needs explicit current-session peer-key/source/target
binding.

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

- event and envelope IDs;
- request ID;
- canonical request payload hash;
- target peer;
- requesting device;
- capability name;
- message exactly `hello peer!`;
- room/current session;
- expiry time.

Consent must not silently become long-lived authorization. Local user approval
on the requesting device does not replace peer approval. The peer may deny
without fallback or retry escalation.

CL-5 implements this binding in current-session memory. The receiver PolicyGate
accepts only the exact fixed `runtime.execute_hello_template` / `hello peer!`
preview and exposes only **Allow once** or **Deny**. Allow once queues
`capability_preview_ack`; deny queues `capability_preview_deny`. The ack records
that the receiver allowed this exact preview once, but it is not proof of
execution, does not launch anything, and creates no long-lived grant. CL-6
requires a separate explicit sender action, revalidates the exact request and
unexpired local consent record, and consumes that consent once before the fixed
function begins. Failure after execution begins still consumes consent.

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

Phase E1 adds frontend current-session duplicate detection for envelope IDs and
embedded request IDs. CL-3B adds a bounded current-session Rust replay cache.
CL-5 records decided event/envelope/request/consent IDs. CL-6 separately records
consumed consent/request/execution IDs and rejects replayed execution requests
and results. Session change clears unusable current-session state. None of
these use a long-term database.

**Conclusion:** replay defense = request ID + expiry + current-session binding +
peer replay cache.

## 4. What Is the Minimum Executor Isolation?

The Hello Peer v0 executor:

- is not raw shell;
- is not arbitrary code execution;
- runs a fixed internal hello template only;
- permits only the exact output message `hello peer!`;
- accepts no command string from the model or requesting device;
- is one fixed in-process host function;
- has no filesystem access;
- has no network access;
- enforces a one-second timeout check;
- caps the fixed output at 64 bytes;
- has no stdout/stderr stream, exit code, log, stack trace, or attachment.

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
- bounded fixed result status.

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

The implemented lifecycle audit reuses the existing bounded rotating
`pastey.log` destination. Entries are structured and redacted, use shortened
room/session/peer/event/request/execution references, and never contain API
keys, room codes/keys, transport keys, ciphertext, raw provider input/output,
raw control payloads, canonical grants, or arbitrary remote output. The log is
never read to reconstruct workflow state and is not consent, authority, or
trust evidence.

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
