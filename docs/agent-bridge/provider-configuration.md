# Provider Configuration

Provider configuration owns the current Agent Bridge model-provider behavior, context controls, Settings ownership, and natural-v1 provider instruction guidance. For execution safety, see [architecture-and-safety.md](architecture-and-safety.md). For validation and smoke invariants, see [../transfer/validation.md](../transfer/validation.md).

## Supported Provider Paths

The production provider abstraction lives in `src/lib/ai/types.ts`.

Current implemented provider paths:

- `MockProvider` in `src/lib/ai/mockProvider.ts`, deterministic and local.
- `CloudOpenAICompatibleProvider` in `src/lib/ai/cloudOpenAICompatibleProvider.ts`, using an OpenAI-compatible chat-completions shape against a configured base URL and model.

Provider output is always untrusted advisory data. In the Ask Bridge product surface, model/provider output is first reduced to the natural-v1 product primitives `Search`, `Transform`, and `Return`. It must pass host-side validation before it can become a local pending action or Bridge control preview. The high-level natural-v1 safety model is canonical in [architecture-and-safety.md](architecture-and-safety.md).

The current low-level provider allowlist is backed by the static capability registry in `src/lib/ai/capabilityRegistry.ts`. Those capability actions are implementation details behind natural-v1 and are not provider-selectable capability semantics:

- `request_peer_hello_demo` for `runtime.execute_hello_template`;
- `request_peer_hello_stdout_demo` for `runtime.hello_stdout`;
- `request_peer_file_candidates` for `filesystem.find_file_candidates`;
- `request_peer_candidate_payload` for `transfer.request_candidate_payload`.

Ask Bridge is now the single natural-language entry for Layer 5. Request file is folded into Ask Bridge as a `Search` / `Return` plan, not a separate primary product model. Provider output remains advisory; the product UI owns natural-language input, Search / Transform / Return preview, local confirmation, active-operation refresh, terminal Deny display, redacted candidate rendering, and manual candidate selection.

## Natural-V1 Provider Instructions

The provider instruction pack is a supporting piece under natural-v1, not a separate versioned module. The canonical code source is `src/lib/ai/providerInstructionPack.ts`; provider adapters import it rather than carrying their own natural-v1 instructions. It must not be loaded from arbitrary Markdown, user files, remote file contents, workspace docs, or provider-returned text.

Provider instructions guide model behavior only. They describe the shared Search / Transform / Return JSON contract and the sole supported Transform intent `selected_artifact_output`; every other Transform kind is `unsupported_future`. Provider output is advisory only and must not contain capability IDs, peer/session fields, candidate IDs/source request IDs, result contracts, shell, command, code, script, arguments, stdin, cwd, env, runtime, compiler, interpreter, network, URL/proxy, paths, file contents, selected-peers/broadcast intent, auto-transfer, queue/handoff ids, consent claims, execution claims, result fields, chain-of-thought, scratchpads, or reasoning traces. Rust derives a pending Transform prompt only from an authenticated received preview, owns its Allow-once ledger and journal, and constructs/sends the sanitized Transform result; TypeScript validation and consent state are defense-in-depth UI mirrors. Providers never receive receiver-local sanitation inputs or raw rejected executor output.

## Provider Adapter Guidance

Future provider adapters should share the same natural-v1 Search / Transform / Return JSON contract. Provider differences must stay mechanical:

- request/message envelope;
- system instruction placement;
- JSON mode, structured output, or response MIME support;
- refusal and error shape;
- streaming versus non-streaming response handling;
- token and context limits.

Do not add provider-specific capability semantics. Do not implement Anthropic-compatible, Gemini-compatible, DeepSeek-compatible, or additional provider adapters until adapter-specific request envelopes and real-provider smoke coverage are designed around the shared provider instruction pack, static risk scanner, and natural-v1 validator.

## Provider Health Check

When cloud API configuration exists, Settings can run a safe natural-v1 provider health check. It sends a minimal advisory-only prompt, expects `Search` / `Transform` / `Return` JSON, runs the same host validator, and reports only validation status. It does not send room-control events, execute capabilities, select candidates, log secrets, grant consent, claim execution, or grant transfer authority. If API configuration is missing or unavailable, deterministic/mock natural-v1 planning remains available locally.

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

Pastey does not currently provide multi-provider adapters beyond the current OpenAI-compatible shape, local LLM scheduling, model routing, MCP tools, persistent provider credentials, cloud relay, provider-managed execution, payload reading, broad capability coverage, global Activity detail surfaces, or full Agent/Jarvis orchestration. Those require new design and validation before they can be claimed as broader Layer 5 completion. The current candidate-payload path can queue a selected file candidate only after second consent; `handoff_queued` remains queue acceptance, not transfer completion.
