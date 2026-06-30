# Provider Configuration

Provider configuration owns the current Agent Bridge model-provider behavior, context controls, and Settings ownership. For execution safety, see [architecture-and-safety.md](architecture-and-safety.md).

## Supported Provider Paths

The production provider abstraction lives in `src/lib/ai/types.ts`.

Current implemented provider paths:

- `MockProvider` in `src/lib/ai/mockProvider.ts`, deterministic and local.
- `CloudOpenAICompatibleProvider` in `src/lib/ai/cloudOpenAICompatibleProvider.ts`, using an OpenAI-compatible chat-completions shape against a configured base URL and model.

Provider output is always untrusted advisory data. It must pass host-side validation and PolicyGate before it can become a local pending action or Bridge control preview.

The current provider allowlist is backed by the static capability registry in `src/lib/ai/capabilityRegistry.ts`. It may propose only these bounded advisory actions:

- `request_peer_hello_demo` for `runtime.execute_hello_template`;
- `request_peer_hello_stdout_demo` for `runtime.hello_stdout`;
- `request_peer_file_candidates` for `filesystem.find_file_candidates`.

Provider output may identify the selected peer, registered capability, fixed message or filename hint, and fixed constraints. It must not include command text, script text, runtime arguments, environment variables, file paths, current working directories, network targets, stdout/stderr/exit values, file contents, hidden transfer requests, selected-peers/broadcast intent, durable trust claims, or execution request/result payloads. The host builds those payloads after validation and consent.

For `filesystem.find_file_candidates`, provider output is only an advisory proposal for bounded candidate metadata discovery. It cannot by itself cause local or peer filesystem traversal, cannot return real candidates, cannot provide real paths, and cannot start a transfer. The host may build a selected-peer preview only after validation and local confirmation; the receiver may run the bounded metadata search only after explicit Allow once.

## Context Controls

Context snapshots are built in `src/lib/ai/contextSnapshot.ts`.

The cloud-safe context path redacts sensitive material and avoids sending raw secrets, file contents, encryption keys, API keys, raw Bridge control payloads, or local filesystem paths. The context is meant to help a model propose a bounded action plan, not reconstruct app state.

## Credential Handling

Cloud provider configuration is Settings-owned. API keys are runtime-memory-only and are not a durable trust or identity mechanism.

Provider configuration does not authorize execution. A valid provider response still requires host validation, local confirmation, receiver PolicyGate review, explicit receiver consent, and a bounded executor.

## Settings Versus Bridge Ownership

Settings owns:

- provider kind;
- cloud base URL;
- model name;
- runtime-memory API key;
- enablement;
- redacted lifecycle-log level.

The active Bridge owns current-session workflow state:

- current Bridge/peer context;
- pending preview workflow;
- receiver review state;
- capability consent;
- execution request/result state.

Legacy implementation term: the active room owns this state.

This split keeps model configuration separate from Bridge-scoped authority. Accepted peer status or session verification does not authorize capability execution.

## Non-Current Provider Work

Pastey does not currently launch or manage `llama-cli`, `llama-server`, Ollama, LM Studio, or other local model processes. The practical local-model workaround is to point the existing OpenAI-compatible provider base URL at a user-managed localhost server that speaks the configured chat-completions shape.

Pastey does not currently provide local LLM scheduling, model routing, MCP tools, persistent provider credentials, cloud relay, provider-managed execution, or approved transfer handoff from file candidates. Those require new design and validation before they can be claimed as Layer 5 completion.
