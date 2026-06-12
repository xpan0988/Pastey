# AI Agent Provider and Tooling Survey

## Executive Summary

Major AI APIs and mature agent runtimes consistently separate model inference from
tool execution. A model receives descriptions and schemas for available tools,
then returns a structured request to use one. The application or agent host
validates that request, decides whether it is permitted, optionally asks the user,
executes the approved capability, and returns a bounded result to the model or
user.

The recurring safety boundary is therefore outside the model provider. Provider
APIs can help produce structured output, but they do not establish whether an
action is authorized or safe. Mature local agents add host-side controls such as
allow, ask, and deny policies; workspace trust; command previews; sandboxing;
least-privilege tools; and audit or review surfaces.

Provider portability is useful but incomplete. OpenAI-compatible APIs reduce the
cost of connecting multiple models, while differences remain in model IDs,
authentication, supported schema subsets, tool-call/result formats, streaming,
server-side tools, and error behavior. A provider adapter must normalize those
differences without treating provider output as trusted authority.

MCP standardizes discovery and invocation of external tools, resources, and
prompts. It does not authorize their use, prove that a server is trustworthy, or
protect an application from prompt injection and excessive permissions. An MCP
host still needs its own policy, consent, isolation, context, and audit controls.

The safest minimal design for a new product is an advisory-first host that treats
all model output as an untrusted proposal. It should expose narrow capabilities,
validate every structured action, apply a local policy gate, require clear human
confirmation for consequential operations, isolate execution, redact context,
and record what was proposed, approved, executed, and returned.

## Research Scope and Evidence

This survey prioritizes official API, product, security, and protocol
documentation. Credible security research is used where official documentation
does not fully cover emerging risks such as MCP tool poisoning.

- **Documented fact** means the behavior is described by a linked source.
- **Inference** means a general design conclusion drawn across several sources.
- Public documentation does not expose every product's internal architecture.
  Where details are unavailable, this survey says so rather than assuming them.
- Product behavior and documentation can change. Links were reviewed on
  June 11, 2026.

## Common Architecture Pattern

The most consistent cross-system pattern is:

```text
user request
  -> host or agent runtime
  -> context selection and redaction
  -> provider adapter and selected model
  -> model response or structured tool request
  -> parser and schema validator
  -> permission and policy gate
  -> optional human approval
  -> least-privilege executor or sandbox
  -> bounded and sanitized result
  -> result returned to model and/or user
  -> audit or review event
```

This pattern creates several distinct trust boundaries:

1. The **provider adapter** handles credentials, request translation, model
   selection, streaming, and provider-specific response normalization.
2. The **model** proposes text or structured actions. Its output remains
   untrusted, even when it conforms to a schema.
3. The **parser and schema validator** establish shape and basic validity, not
   authorization.
4. The **policy gate** decides whether the requested capability, arguments,
   context, and destination are allowed.
5. The **human approval surface** makes consequential action visible and
   intentional.
6. The **executor** performs only the approved capability within a constrained
   environment.
7. The **result reporter** limits what execution output is returned to the model,
   user, logs, or external services.

**Inference:** A model provider should never be the final authority for a local or
remote side effect. The host has the relevant user intent, product policy,
environment, and capability context, so authorization belongs there.

## System-by-System Findings

### OpenAI / Codex

#### Model and provider invocation

The OpenAI API exposes model invocation through APIs including Responses and Chat
Completions. Applications choose a model and submit input, tools, and generation
settings. OpenAI's Agents SDK documentation also describes explicit model
selection and provider/client overrides, allowing a host to vary model and
transport configuration.

Codex uses a host configuration model for selecting models and controlling local
capabilities. Its public documentation describes sandbox and approval settings,
network access controls, and MCP server configuration.

#### Tool and structured-output model

OpenAI function calling describes tools using names, descriptions, and JSON
Schema parameters. The model returns a structured function call. The application
executes the function and sends a matching function-call output back to the
model. Structured Outputs and strict function schemas improve conformance to a
declared shape, but schema conformance does not establish safety or permission.

The execution loop is explicitly host-driven:

```text
application supplies tool schema
  -> model returns function call
  -> application validates and executes
  -> application sends function_call_output
  -> model continues
```

#### Permission, sandbox, and security boundary

Codex documents separate sandbox and approval controls. Permission profiles
restrict filesystem and network access, while approval policies determine when
Codex must ask before attempting an operation. Codex recommends the narrowest
permission profile that still supports the task.

Codex guardrail and approval guidance treats sensitive tool use as interruptible:
the runtime records a pending action, the application or user approves or rejects
it, and execution resumes only after that decision. Codex MCP configuration can
also limit enabled tools and specify approval behavior at server or tool level.

The security boundary is the Codex host, policy, and sandbox, not the generated
tool call. Platform-specific sandbox strength and supported policies vary, so a
host must fail conservatively when a requested isolation mode is unavailable.

#### Reusable general pattern

- Keep tool execution in the host.
- Use strict schemas to reduce malformed actions, then separately authorize them.
- Place approval checks immediately before side effects.
- Separate sandbox permissions from approval policy.
- Apply per-tool allowlists and approval rules to externally supplied tools.

#### Sources

- [OpenAI function calling guide](https://developers.openai.com/api/docs/guides/function-calling)
- [OpenAI Structured Outputs guide](https://developers.openai.com/api/docs/guides/structured-outputs)
- [OpenAI Agents SDK models and providers](https://developers.openai.com/api/docs/guides/agents/models)
- [OpenAI guardrails and human approvals](https://developers.openai.com/api/docs/guides/agents/guardrails-approvals)
- [Codex permissions](https://developers.openai.com/codex/permissions)
- [Codex security](https://developers.openai.com/codex/security)
- [Codex MCP support](https://developers.openai.com/codex/mcp)

### Anthropic / Claude Code

#### Model and provider invocation

Anthropic's Messages API accepts tools alongside messages. Claude Code can use
Anthropic directly and documents enterprise provider routes through AWS Bedrock,
Google Vertex AI, Microsoft Foundry, and organization-managed LLM gateways or
proxies. Its settings support model selection, fallback behavior, provider
configuration, and managed policy at several configuration scopes.

Provider-specific model identifiers and versions remain significant. Anthropic
recommends pinning appropriate provider-specific versions rather than assuming a
single model name is portable across every route.

#### Tool and structured-output model

Client tools are described with names, descriptions, and input schemas. Claude
returns a `tool_use` block with an ID, tool name, and input. The application
executes the tool and returns a corresponding `tool_result`. Anthropic's Tool
Runner can automate this loop, but the manual loop remains appropriate when an
application needs human approval, custom logging, or conditional execution.

Anthropic also distinguishes client tools from server tools. Server tools are
executed within Anthropic's service, while client tool execution remains the
responsibility of the application.

Anthropic warns that tool results can contain untrusted content and indirect
prompt injection. Keeping results in their structured tool-result role helps
preserve provenance, but the host must still constrain and assess them.

#### Permission, sandbox, and security boundary

Claude Code documents allow, ask, and deny permission rules, with deny taking
precedence over ask and allow. It states that permissions are enforced by Claude
Code rather than by the model. Read-only activity generally has a different
approval posture from edits and shell commands.

Claude Code combines permission rules with sandboxing as defense in depth.
`PreToolUse` hooks can deny an action or force an approval prompt. Public
documentation also notes an important boundary: MCP servers and hooks are
separate processes and are not automatically constrained by the same built-in
tool sandbox unless the whole process or environment is isolated.

Claude Code supports MCP and advises users to verify MCP server trust because
external servers and content can introduce prompt-injection risk. When Claude
Code is exposed as an MCP server, the connecting client remains responsible for
confirmation and authorization.

#### Reusable general pattern

- Make provider routing configurable without weakening the local permission
  model.
- Let deny rules override convenience settings.
- Preserve provenance for untrusted tool results.
- Treat hooks and MCP servers as separate processes with separate trust.
- Use a manual tool loop when approval or custom policy must interrupt execution.

#### Sources

- [Anthropic: Define tools](https://platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools)
- [Anthropic: Handle tool calls](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls)
- [Anthropic: Tool Runner](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-runner)
- [Claude Code permissions](https://code.claude.com/docs/en/permissions)
- [Claude Code settings](https://code.claude.com/docs/en/settings)
- [Claude Code enterprise provider integrations](https://code.claude.com/docs/en/third-party-integrations)
- [Claude Code MCP](https://code.claude.com/docs/en/mcp)
- [Claude Code sandbox environments](https://code.claude.com/docs/en/sandbox-environments)

### Google Gemini / Vertex AI / Gemini CLI

#### Model and provider invocation

The Gemini API and Vertex AI expose Gemini model invocation through distinct
authentication, hosting, and enterprise-control routes. Gemini CLI documents
authentication through a Google account, Gemini API key, or Vertex credentials.
It also documents model selection and routing behavior.

#### Tool and structured-output model

Gemini function calling uses function declarations with names, descriptions, and
parameter schemas. The model returns a structured function call containing the
function name and arguments. The application executes the function and returns a
function response, associated with the call when an ID is present. Google
explicitly states that the model does not execute the function.

Gemini also supports structured output using JSON Schema. As with other
providers, structured output constrains response shape but does not authorize an
action.

#### Permission, sandbox, and security boundary

Gemini CLI documents a host-side policy engine with `allow`, `deny`, and
`ask_user` decisions. Policies cover built-in tools, shell commands, MCP tools,
and subagents. Higher-priority rules win, and an approval requirement becomes a
denial in noninteractive contexts where no user can answer.

The CLI validates tool parameters and security settings before execution,
previews sensitive operations such as commands or diffs, and prompts the user.
It also documents sandbox options and trusted-folder behavior. Denied tools can
be excluded from model context, reducing both accidental selection and exposure
of unavailable capabilities.

#### Reusable general pattern

- Make noninteractive approval fail closed.
- Remove denied capabilities from the model's advertised tool set.
- Show exact commands or diffs before approval.
- Apply one policy engine consistently across built-in, shell, MCP, and delegated
  tools.
- Keep authentication choice separate from local authorization.

#### Sources

- [Gemini API function calling](https://ai.google.dev/gemini-api/docs/function-calling)
- [Gemini API structured output](https://ai.google.dev/gemini-api/docs/structured-output)
- [Vertex AI function calling](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/tools/function-calling)
- [Gemini CLI documentation](https://geminicli.com/docs/)
- [Gemini CLI tools](https://geminicli.com/docs/tools/)
- [Gemini CLI policy engine](https://geminicli.com/docs/reference/policy-engine/)
- [Gemini CLI sandbox](https://geminicli.com/docs/cli/sandbox/)
- [Gemini CLI trusted folders](https://geminicli.com/docs/cli/trusted-folders/)
- [Gemini CLI authentication](https://github.com/google-gemini/gemini-cli/blob/main/docs/get-started/authentication.md)
- [Gemini CLI model routing](https://geminicli.com/docs/cli/model-routing/)

### Microsoft / GitHub Copilot / VS Code

#### Model and provider invocation

GitHub Copilot exposes several agent surfaces rather than one publicly documented
provider-adapter architecture. Public documentation describes model choice and,
for some organizational configurations, bring-your-own-key controls. Exact
internal provider routing and normalization details are not fully public.

GitHub's cloud coding agent works in an ephemeral GitHub Actions development
environment, creates changes on a branch, and presents them for review through
the pull-request workflow. VS Code's local agent and chat surfaces operate inside
the editor's extension, workspace, and terminal security model.

#### Tool and action model

Copilot and VS Code surface suggested changes, tool activity, terminal commands,
and reviewable diffs through the host UI. The cloud coding agent's branch and pull
request are a durable review boundary between generated work and adoption.

Public documentation does not describe every internal tool-call schema or
execution loop. The visible product boundary is nevertheless clear: actions are
performed by the agent host or its execution environment, not directly by model
text.

#### Permission, sandbox, and security boundary

VS Code Workspace Trust places untrusted workspaces into Restricted Mode and
limits features that could execute code. Terminal and agent actions are surfaced
through the editor and its approval controls. GitHub Copilot CLI documents
allowing tools and commands, while the cloud coding agent isolates work in an
ephemeral Actions environment and relies on branch and pull-request review.

Permission behavior differs across Copilot surfaces and can evolve. A
provider-agnostic application should learn from the visible review and workspace
trust boundaries without assuming that Copilot has one universal sandbox or
approval implementation.

#### Reusable general pattern

- Treat workspace trust as an input to capability availability.
- Put generated changes in a reviewable artifact before adoption.
- Surface commands, edits, and tool activity in the host UI.
- Isolate autonomous work in a disposable environment where practical.
- Do not infer a universal security policy from a shared product name.

#### Sources

- [GitHub Copilot cloud coding agent](https://docs.github.com/en/copilot/concepts/agents/cloud-agent/about-cloud-agent)
- [GitHub Copilot CLI: Allowing tools](https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli/allowing-tools)
- [VS Code chat and agent overview](https://code.visualstudio.com/docs/chat/chat-overview)
- [VS Code Workspace Trust](https://code.visualstudio.com/docs/editing/workspaces/workspace-trust)

### Cursor / Windsurf / Other Coding Agents

#### Publicly documented patterns

Cursor and Windsurf publicly document agent workflows, terminal use, MCP
integration, model selection, privacy controls, and security-related product
settings. These products illustrate the user demand for switching models,
bringing provider credentials, controlling codebase context, reviewing edits,
and approving consequential terminal activity.

However, their public documentation does not provide enough stable detail to
make strong claims about every internal provider adapter, policy decision, or OS
sandbox boundary. Marketing descriptions of privacy or autonomy are not a
substitute for a documented execution and authorization model.

#### Reusable general pattern

- Make external context and model selection visible to the user.
- Separate codebase indexing and context controls from action authorization.
- Preview edits and terminal actions before consequential execution.
- Treat privacy mode as one control among many, not proof of safe tool use.
- Document exact execution boundaries rather than relying on broad agent claims.

#### Sources and public gaps

- [Cursor documentation](https://cursor.com/docs)
- [Cursor security](https://cursor.com/security)
- [Windsurf terminal documentation](https://docs.windsurf.com/windsurf/cascade/terminal)
- [Windsurf security](https://windsurf.com/security)

The sources above are useful product and security references, but the exact
internal provider normalization and sandbox architecture were not sufficiently
documented to compare at the same depth as Codex, Claude Code, or Gemini CLI.

### MCP Ecosystem

#### Architecture and protocol model

MCP defines a host, clients, and servers. A host coordinates one or more MCP
clients, and each client maintains a connection to a server. Servers can expose
tools, resources, and prompts. For tools, clients discover capabilities through
`tools/list` and invoke them through `tools/call`.

MCP solves an interoperability problem: it gives hosts and servers a common way
to describe and invoke capabilities and exchange contextual data. It can reduce
one-off integrations and allow a host to connect to many external services.

#### What MCP does not solve

MCP discovery is not authorization. A schema says how to call a tool, not whether
the current user, model, task, or context should be allowed to call it. MCP also
does not by itself establish server trust, prevent prompt injection, constrain
server-side effects, limit returned data, or guarantee that a tool's description
remains honest.

Security research has demonstrated tool-poisoning and prompt-injection risks,
including malicious instructions placed in tool descriptions or results.
Because tools can influence model behavior and cause external side effects, a
host must treat both MCP metadata and results as untrusted.

#### Required host-side boundary

An MCP-capable application should combine the protocol with its own:

- server trust and installation policy;
- tool allowlist and least-privilege capability design;
- schema and argument validation;
- per-call permission and human-confirmation policy;
- sandbox or constrained executor;
- result-size, provenance, and context controls;
- audit events and revocation controls.

Exposing a tool to a model means the model may propose using it. It must not mean
the model has unrestricted authority to execute it.

#### Sources

- [MCP architecture](https://modelcontextprotocol.io/docs/learn/architecture)
- [MCP tools specification](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)
- [Invariant Labs: MCP tool-poisoning attacks](https://invariantlabs.ai/blog/mcp-security-notification-tool-poisoning-attacks)
- [OWASP LLM01: Prompt Injection](https://genai.owasp.org/llmrisk/llm01-prompt-injection/)
- [Systematic analysis of MCP security](https://arxiv.org/abs/2603.22489)
- [Architectural risks in MCP ecosystems](https://arxiv.org/abs/2601.17549)

### Provider Gateways / OpenAI-Compatible APIs

#### Invocation and normalization model

Provider gateways such as OpenRouter expose a common API surface across multiple
models and providers. Several model providers also expose OpenAI-compatible
endpoints. This can make basic invocation, streaming, and tool integration easier
to adopt because applications can begin with a familiar request and response
shape.

Compatibility is not complete portability. Differences commonly remain in:

- provider and gateway authentication;
- model ID naming and version pinning;
- supported message roles and request fields;
- JSON Schema subsets and strict-output behavior;
- tool-call IDs, argument encoding, and tool-result conventions;
- parallel and server-side tool support;
- streaming event types and ordering;
- token accounting, rate limits, retries, and error shapes;
- data retention, routing, logging, and geographic controls.

A provider adapter should normalize these differences into a stable host-facing
result, preserve provider-specific diagnostics where useful, and reject
unsupported semantics explicitly. Silent fallback can weaken safety or produce
incorrect actions.

#### Gateway risks

A gateway adds another trusted service between an application and the model. It
may receive prompts, tool definitions, tool results, credentials, and metadata.
Applications must evaluate the gateway's retention, routing, security, privacy,
and failure behavior. A gateway's normalized output must still be treated as
untrusted model output.

No provider or gateway response should be trusted as proof of authorization,
user intent, safe arguments, truthful tool metadata, or safe execution output.

#### Reusable general pattern

- Normalize provider transport, not product policy.
- Maintain explicit capability flags for provider differences.
- Validate tool calls after normalization.
- Pin and record actual provider/model identity.
- Make gateway routing and data exposure visible.
- Fail closed when a requested safety-relevant feature is unsupported.

#### Sources

- [OpenRouter API overview](https://openrouter.ai/docs/api/reference/overview)
- [Gemini API OpenAI compatibility](https://ai.google.dev/gemini-api/docs/openai)
- [Anthropic enterprise provider integrations](https://code.claude.com/docs/en/third-party-integrations)

## Cross-System Comparison Table

| System | Provider abstraction | Tool schema mechanism | Who executes tools | Permission/approval model | Sandbox model | External tool protocol | Cloud/local context controls | Key lesson |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| OpenAI API / Codex | API model selection; Agents SDK and Codex provider/config controls | JSON Schema function tools and Structured Outputs | Application for client tools; provider for documented server tools | Host guardrails and interruptible approvals; Codex approval policy | Codex filesystem/network sandbox with platform-specific enforcement | MCP | Host chooses context; Codex controls filesystem/network access | Strict schemas improve shape, while host policy authorizes execution |
| Anthropic API / Claude Code | Anthropic direct, Bedrock, Vertex, Foundry, gateways/proxies | Tool input schemas with `tool_use` and `tool_result` blocks | Application for client tools; Anthropic for server tools | Claude Code allow/ask/deny rules and hooks | Built-in tool sandbox; external MCP servers/hooks require separate isolation | MCP | Settings, managed policy, and provider route affect context exposure | Preserve tool-result provenance and treat external processes separately |
| Gemini / Vertex / Gemini CLI | Gemini API, Vertex, account/API-key/enterprise routes | Function declarations and JSON Schema structured output | Application or Gemini CLI host | CLI policy engine with allow/deny/ask_user | CLI sandbox plus trusted-folder controls | MCP | Host/CLI selects context and trust level | Apply one fail-closed policy engine across all tool sources |
| GitHub Copilot / VS Code | Multiple Copilot surfaces and model choices; internals partly public | Public details vary by surface | Editor, CLI, or cloud agent environment | Visible tool/terminal approvals and review workflows | Workspace Trust; cloud agent uses ephemeral Actions environment | MCP support documented in Copilot surfaces | Workspace trust and review artifacts constrain exposure/adoption | Use workspace trust and reviewable diffs as first-class boundaries |
| Cursor / Windsurf | User-facing model/provider options; internal normalization not fully public | Public details incomplete | Product host | User-facing terminal/edit controls are documented, exact internals vary | Exact sandbox boundary not sufficiently public | MCP | Privacy and context/indexing controls are user-facing | Do not infer hard security guarantees from product-level claims |
| MCP | Protocol is provider-neutral | Server advertises tool input schemas through `tools/list` | MCP server after host/client invokes `tools/call` | Not supplied by protocol; host responsibility | Not supplied by protocol | MCP itself | Resources/results cross server boundary under host control | Discovery and invocation are not authorization |
| OpenAI-compatible gateways | Common request/response envelope across routed models | Usually OpenAI-style tools, with provider-specific gaps | Calling application or provider-specific server tool | Host responsibility | Host responsibility | Varies | Gateway becomes an additional context recipient | Normalize transport differences, never delegate authority |

## Design Patterns We Should Learn

### Provider adapters

Use a stable host-facing provider contract for credentials, model identity,
requests, streaming events, structured responses, errors, and capability flags.
Keep authorization and product policy outside the provider adapter.

### Model output as an untrusted plan

Treat generated text, structured output, tool calls, arguments, and follow-up
requests as proposals. They may be malformed, deceptive, based on injected
content, or inconsistent with user intent.

### Schema validation

Validate action shape, types, enumerations, bounds, and required fields before
policy evaluation. Use strict provider schemas when supported, but validate again
inside the host because provider behavior and compatibility vary.

### Policy gate

Evaluate the requested capability, arguments, target, current trust state,
provider, context source, user intent, and interaction mode. Deny rules should
take precedence. Unsupported or ambiguous safety conditions should fail closed.

### Human confirmation

Require clear, timely confirmation for consequential or externally visible
effects. Show exact commands, targets, file changes, data disclosures, or other
meaningful parameters. Approval should apply to the concrete pending action, not
to a vague future category.

### Executor isolation

Run approved actions through narrow tools and constrained environments. Separate
filesystem, network, process, credential, and peer-side capabilities. Sandboxing
is defense in depth and does not replace policy or consent.

### Context redaction and provenance

Select the minimum context needed for the task. Redact secrets and sensitive
metadata before cloud transmission. Preserve whether content came from the user,
a file, a webpage, a tool, an MCP server, or another model so that untrusted
instructions do not silently become authoritative.

### Least privilege

Expose narrow, task-specific capabilities instead of broad shell, filesystem, or
administrative access. Advertise only tools that are currently available and
allowed. Limit arguments, scope, duration, and output.

### Action and result loop

Keep a typed relationship between proposed action, policy decision, approval,
execution result, and any subsequent model turn. Bound and sanitize results
before returning them to a model or external service.

### Auditability and revocation

Record provider/model identity, context classes disclosed, proposed actions,
policy decisions, approvals, execution outcomes, and errors. Let users and
administrators revoke tools, providers, credentials, and remembered approvals.

### No direct model authority

The model should not hold credentials, bypass the host, or directly invoke local
or peer capabilities. Authorization belongs to the application that understands
the user's policy and environment.

## Security and Safety Findings

### Common failure modes

- Indirect prompt injection in files, webpages, logs, tool descriptions, or tool
  results changes the agent's intended behavior.
- Over-broad tools turn a small model error into a large side effect.
- Vague or repeated approval prompts train users to approve without inspection.
- Tool schemas are mistaken for permission checks.
- A model or gateway silently falls back to unsupported structured-output or
  tool behavior.
- Sensitive context is sent to a cloud provider or gateway without a clear need.
- External tools, hooks, MCP servers, or plugins run outside the expected
  sandbox.
- Persistent memory retains secrets, inferred behavior, or stale instructions.
- Result data is returned to the model without size, provenance, or privacy
  controls.
- Logs omit the exact proposed action, approval decision, or actual execution.

### Consistently recommended protections

- Least-privilege tools and narrow capability scopes.
- Host-side schema validation, policy checks, and execution.
- Human approval for consequential operations.
- Sandboxing and disposable execution environments.
- Clear previews of commands, diffs, targets, and disclosures.
- Context minimization and secret handling.
- Tool and server trust controls.
- Audit and review surfaces.
- Conservative behavior when policy or isolation is unavailable.

### Protections often missing or under-documented

- Formal provenance and trust propagation across multi-tool or multi-agent
  workflows.
- Attestation that discovered tool descriptions and implementations have not
  changed.
- Clear isolation guarantees for third-party plugins, hooks, and MCP servers.
- Per-action data-disclosure previews for cloud providers and gateways.
- Consistent revocation and expiration of remembered approvals.
- Public detail about provider normalization and unsupported-feature fallback.

### Safest minimal design for a new product

A new product should begin with read-only or advisory behavior. It should expose
only a small set of narrow capabilities, treat every model action as untrusted,
validate it against a local schema, apply a deny-first policy gate, and require
specific user confirmation before any side effect. Execution should occur in a
constrained host-owned executor. Context sent to cloud services should be
minimal and redacted, and every proposal, decision, disclosure, and result should
be reviewable.

## Anti-Patterns

- Letting model output directly execute commands or side effects.
- Letting a provider or gateway own local permission decisions.
- Exposing raw shell as a default general-purpose tool.
- Sending full logs, histories, workspaces, or file contents to a cloud model by
  default.
- Sending file contents without explicit user intent.
- Enabling broad persistent memory or behavioral profiling by default.
- Hiding tool calls, targets, arguments, data disclosure, or execution results.
- Using vague confirmation that does not show the concrete pending action.
- Treating schema-valid output as safe or authorized.
- Treating MCP tool exposure as authorization.
- Assuming MCP servers, hooks, plugins, or delegated agents share the host
  sandbox.
- Silently falling back when a provider lacks a required safety or schema
  feature.
- Combining provider credentials, policy decisions, and executor privileges in
  one component.

## Possible Implications for Pastey

These are tentative considerations, not a final architecture or roadmap:

- Pastey may later need a provider-agnostic model adapter that does not own
  permissions or transfer behavior.
- Any AI context may need a strict local/cloud boundary with explicit redaction.
- Model output may need an action-plan schema that remains advisory until a
  separate Pastey policy gate and human confirmation approve an existing action.
- Peer-side capabilities would need their own capability and trust gate.
- Raw shell should not be a default capability.
- An external-agent integration path and a built-in AI path may need separate
  trust, context, and execution boundaries.

## Open Questions

- Which provider routes, credential models, and deployment environments would be
  acceptable to users and administrators?
- What context classes may be sent to local models, cloud providers, or gateways,
  and how should each disclosure be shown?
- Which provider capabilities are mandatory, and when must unsupported features
  fail closed rather than degrade?
- What actions, if any, could be approved by durable policy rather than
  per-action confirmation?
- How should action approvals expire, be revoked, and be audited?
- What is the smallest useful set of narrow tools that avoids raw shell and broad
  filesystem authority?
- Which executor isolation guarantees are required on each supported platform?
- How should untrusted tool descriptions, tool results, MCP resources, and
  external-agent output retain provenance?
- How should MCP servers be installed, trusted, updated, pinned, and revoked?
- What limits should apply to result size, model-visible logs, retries, and
  multi-step loops?
- How should provider and gateway retention, routing, and privacy differences be
  communicated?
- When should autonomous work use a disposable environment or reviewable branch
  instead of the user's active environment?
- What peer-side capabilities could ever be appropriate, and what separate
  threat model would be required before exposing them?
