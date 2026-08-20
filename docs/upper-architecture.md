# Pastey 上层产品与运行时架构

本文是 Pastey 1.9.2 在 Layer 1–5 语义与权限合同结构冻结之后的上层架构规范。它以当前本地代码为依据，定义未来 Managed Workspace、Agent、Host Runtime、Developer Mode、Multi-Host 与通用托管对象接入之间的边界。

除“当前实现”小节明确说明的能力外，本文描述的是已经达成一致、但尚未实现的未来架构。特别是：当前只有 Search 与 Transfer 可执行；Transform 与 Execute 仍只有 Plan 框架，任何包含二者的不可变 Plan 都会在执行准入前整体 fail closed。Developer Mode v0 已实现 desktop-to-desktop 的人工 PTY/ConPTY 路径与独立权限域，但本文不宣称 Agent Harness、Headless Host、generic Host admission policy 或 Multi-Host protocol 已经存在。

## 1. 架构结论

当前 Layer 1–5 可以直接承载一致的上层架构，不需要改变已经冻结的四原语语义、显式拓扑、不可变 Plan、逻辑修订或权限边界。

未来工作的性质分为三类：

- **接口抽取**：把当前由 Tauri `AppState`、invoke command 与窗口事件承载的 Host 服务抽成 UI 无关的 Host Runtime；为未来 Worker 提供一个受 Core 控制的调用缝隙。
- **表示迁移**：把 `requesting_device` / `selected_device` 升级为多参与 Host 表示，把 `selected_file` / Search-first 根升级为通用托管对象绑定。
- **新权限域**：Developer Terminal v0 已增加与 Managed Workspace 分离的人工 authority；generic/headless Host admission 与受约束的 Agent effect/tool enforcement 仍待实现。

这些工作都不能倒置现有依赖方向，也不能把 Agent、renderer 或 Layer 4 session 提升为 Pastey 权限来源。

## 2. 单一规范架构

```text
                                      USER
                                        │
                  ┌─────────────────────┴─────────────────────┐
                  │                                           │
          MANAGED WORKSPACE                             DEVELOPER MODE
   GUI / Inbox / drag-drop / local assistant          explicit human action
                  │                                           │
      ┌───────────┴───────────┐                               │
      │                       │                               │
 Local Intent Interpreter   PM / Planner Agent                │
 constrained proposal       WHAT / WHERE / ORDER              │
      │                       │                               │
      └──────── Candidate Semantic Plan ────────┐              │
                                                ▼              ▼
                               ┌────────────────────────────────────┐
                               │            PASTEY CORE             │
                               │ proposal validation and lowering   │
                               │ object binding / logical revisions │
                               │ immutable Plan / topology / hash   │
                               │ Review / requester approval        │
                               │ Host admission orchestration       │
                               │ attempt / step grants / lineage    │
                               └──────────────┬─────────────┬───────┘
                                              │             │
                                 approved semantic step     │ separate human-only
                                              │             │ terminal grant
                                              ▼             ▼
                                  WORKER AGENT HARNESS   TERMINAL SERVICE
                                  HOW / reason / tools    PTY / native console
                                              │             │
                                      bounded tool requests │
                                              ▼             │
                               HOST EFFECT / TOOL ENFORCEMENT│
                               exact step + local policy     │
                                              └──────┬──────┘
                                                     ▼
                                      PASTEY HOST RUNTIME
                         shared Core services / identity / Bridge / Burn
                                ┌───────────────────┴──────────────────┐
                                │                                      │
                         Desktop Adapter                         Headless Adapter
                          Tauri shell/UI                         daemon/service
                                └───────────────────┬──────────────────┘
                                                    ▼
                 Layer 2 facts → Layer 5 eligibility → Layer 3 capacity
                                                    ↓
                         Layer 4 session/control → Layer 1 transport
                                                    ↓
                                                  HOST(S)
```

图中的 Worker Harness 与 Terminal Service 是两个不相交的权限域。Host Runtime 是共享运行容器，不是新的语义层；它承载现有 Layer 1–5 Host 侧服务及未来 Host 侧能力。Desktop/Headless 只是适配器。

## 3. 组件责任

| 组件 | 输入 | 输出 | 生命周期所有者 | 权限级别 | 禁止职责 | 当前连接点 |
| --- | --- | --- | --- | --- | --- | --- |
| Managed Workspace UI | 用户交互、可展示对象与设备 | 编辑中的候选流程、Review 动作 | Desktop adapter | 无执行权限 | 生成 ObjectRef、approval、grant；隐藏移动 | `BridgeProductPages.tsx`, `bridgePlanComposer.ts`, `tauri.ts` |
| Local Intent Interpreter | 用户语言、受限事实词汇表 | `CandidateSemanticPlan` | requester 客户端 | 提案权限为零 | 文件/网络/进程工具；自动云升级；批准或运行 | `naturalV1Plan.ts` 的确定性 schema/validator seam |
| PM / Planner Agent | 用户目标、可公开的 Host/对象事实 | WHAT/WHERE/ORDER 候选 Plan | requester 显式选择的 planner run | 提案权限为零 | 创建 grant；运行工具；重写已批准 Plan | natural-v1/provider advisory pipeline；未来 adapter |
| Pastey Core | 候选 Plan、对象绑定、当前 session、用户决定 | 不可变 revision、approval、attempt、step authority、lineage | Host Runtime | 唯一 managed authority owner | 把 route/capability/provider 结果当批准 | `bridge_plan.rs`, `commands.rs`, `bridge_plan/protocol.rs` |
| Managed Object Binder | Host-local physical artifact 与安全身份 | logical object、revision、location、session binding | Pastey Core / Host Runtime | 只绑定对象，不授予行为 | 把 import 当 Transform；暴露私有路径；隐式 Transfer | `object_refs.rs`, `file_candidates.rs`, `safe_file_identity.rs`; future generic import seam |
| Host Admission | exact approved Plan/Host fragment、Host identity/session、local policy | admit/deny/约束，锚定 Plan hash | 每个执行 Host | Host-local admission authority | 修改 Plan、增加步骤、以 route 代替 policy | 当前 receiver `accept_start` 之前的未来接口 |
| Layer 5 Host Coordinator | immutable attempt state、完成事件 | 原子领取并分派下一个已创作 eligible step | Host Runtime | attempt/step correlation authority | 通用调度；隐藏步骤；绕过全 Plan fail-closed | `continue_bridge_plan_attempt_inner`, `authorize_next_eligible_transfer` |
| Worker Agent Harness | 单个已批准 Transform/Execute 描述、受限观察 | tool requests、observations、result/failure proposal | Worker run | 无 Host 权限；只有请求能力 | 选择 Host；改 topology；获得 Terminal grant；登记修订 | 未来接在 Host coordinator 的 primitive dispatch seam |
| Host Effect / Tool Enforcement | exact step grant、semantic/effect envelope、tool request | 受控副作用、验证后的结果证据 | Host Runtime | 实际 effect authority | 让 Harness 自证权限；扩展语义边界 | 新权限域；复用 identity/grant/Burn 基础 |
| HostRuntime | 配置/路径/session/runtime state | Layer 1–5 Host 服务 | desktop app 或 daemon | 承载 Core 权限，不由 UI 决定 | 依赖窗口存在；把事件展示当状态真相 | 当前 `AppState` 与 setup 中可复用的 Rust 服务 |
| Desktop Adapter | OS 桌面生命周期、Tauri invoke/events | UI 命令适配、通知、tray/window | Tauri shell | 无新增 Core 权限 | 在 renderer 重建 authority/state machine | `main.rs`, Tauri command wrappers, plugins |
| Headless Adapter | service config、daemon lifecycle、RPC/log sink | 同一 HostRuntime 的非 GUI 容器 | service manager | 无新增 Core 权限 | 复制 Layer 1–5；跳过 Host admission | 未来 adapter；共享 HostRuntime |
| Developer Terminal | 人工请求、Developer v0 Host admission、独立 session grant | PTY/console stream 与退出状态 | `DeveloperTerminalService` | 广泛但短期、人工授予 | 声称 managed lineage；由 Agent 进入；成为第五原语 | 已实现 v0；平行于 Layer 5，复用 Layer 4/`HostRuntimeState` |
| Layer 2 facts | Host 观测 | 有界事实 | HostRuntime | 无权限 | 路由、批准、拓扑改写 | `peer_capabilities.rs`, `capability_probe.rs`, `device_profile.rs` |

## 4. 权限模型

### 4.1 权限链

```text
PM / local interpreter / renderer
             │ proposal only
             ▼
Pastey Core deterministic validation + lowering
             │
             ▼
Requester semantic workflow approval
             │ exact immutable Plan revision/hash
             ▼
Host-local admission on every affected Host
             │ admitted exact Host-bound work
             ▼
Layer 5 attempt / one-use step authority
             │
             ├─ Search / Transfer → existing implementation boundary
             │
             └─ future Transform / Execute
                    │ semantic/effect envelope
                    ▼
                 Worker Harness ── tool request ──► Host Tool Enforcement
                    ▲                                  │
                    └──────── bounded observation ─────┘
                                                       │
                                                       ▼
                                            verified result / lineage
```

Requester workflow approval与 Host admission 都必须成功。前者回答“用户批准了什么整体语义和拓扑”；后者回答“此 Host 是否根据本地策略接纳被批准且明确绑定给它的工作”。Layer 4 当前 route/session 只提供身份、传送与活性上下文，不能替代任何一个决定。

Harness tool permission 只描述 Worker 可请求哪些工具。真正的 filesystem/process/network 等 effect authority 必须由 Host Runtime 在每次请求时用 exact Plan/revision/attempt/step、Host、object revision、expiry 与本地 admission 重新约束。未来 semantic/effect envelope 属于 Pastey Core 与 Host enforcement 的边界，而不是 PM prompt 或 Harness 内部状态。

Managed approval 是语义授权，不必等同于 exact patch、command、argv、cwd 或 runtime。例如用户批准“修复这个项目直到测试通过”，并不自动允许修改系统配置、访问任意网络目标、安装任意系统软件包或修改无关文件。未来 Host Effect Enforcement 必须把已批准语义编译/约束为具体 effect envelope 并逐次执行；不能为了回避该问题而把 exact patch/command 强塞进冻结的 Layer 5 Plan 语义。

### 4.2 Developer Terminal 权限链

```text
human explicit Developer Mode request
        │
        ▼
current-session Host identity + Host admission
        │
        ▼
DeveloperTerminalGrant (separate type, expiry, session binding)
        │
        ▼
terminal channel / PTY / native console
        │
        └─ cancel / disconnect / expiry / Burn → terminal authority ends
```

Terminal grant 不派生自 Layer 5 Plan，也不能转换为 Agent step grant。Terminal 内任意副作用不得自动登记为受管理对象修订。

### 4.3 禁止的升级路径

```text
PM Agent             X→ execution/step grant
Local model          X→ approval or cloud escalation
Worker Harness       X→ topology rewrite / new Transfer / Host selection
Worker Harness       X→ DeveloperTerminalGrant
Harness tool policy  X→ Host effect authority
Capability fact      X→ routing / authority / movement
Layer 4 route        X→ requester approval / Host admission
Renderer/provider    X→ immutable revision / ObjectRef / grant
Developer Terminal  X→ managed revision lineage
```

## 5. 与当前仓库的接口映射

| 上层组件 | 当前代码/接口 | 是否可直接使用 | 最小抽取或变化 |
| --- | --- | --- | --- |
| PM Agent | `src/lib/ai/naturalV1Plan.ts`, provider instruction/risk scanner, provider adapter | 部分；提案/验证边界正确 | 把 planner provider 明确适配为统一 candidate-plan producer；不进入 Rust authority |
| Worker Agent | 当前无实现 | 否，但 Core seam 已存在 | 在 Host coordinator 的 primitive dispatch 后增加单步 `WorkerRun` 接口；输入锚定 exact step，输出只为请求/结果提案 |
| Local Intent Interpreter | natural-v1 schema、deterministic builder、strict validator | 适合作为 v1 起点 | 将模型后端做成显式本地 adapter；逐步解除 Search-first/two-role 表示，不授予工具 |
| Agent Harness | 当前无 reasoning/tool loop | 否 | 新增 Pastey-facing harness adapter；Host tool broker 保留真实权限 |
| HostRuntime | `AppState`, startup/recovery/cleanup/Burn, stores and runtimes | 核心服务可复用，容器不可直接无 UI 使用 | 抽取 UI 无关 runtime state/service；注入 path、event sink 与 task spawning |
| Desktop adapter | `main.rs` setup/invoke registration、tray/window/plugins | 是，作为 adapter | Tauri commands 变为薄调用；保留桌面专属生命周期 |
| Headless adapter | 无 | 否 | 新 service binary/adapter，复用同一 HostRuntime；不复制 Core |
| Developer Terminal | `developer_terminal.rs`, `host_runtime.rs`, Room Control typed branch, Bridge Developer UI | v0 可用 | durable HostRef、headless admission、持久 session 与完整 terminal emulator 仍需后续孤立扩展；不得穿过 Agent |
| Host admission | receiver review/start 校验具有部分准入位置，但没有通用 policy | 位置可复用，接口需新增 | 在本地 grant/副作用之前增加 exact Plan/Host-bound admission decision |
| Multi-Host identity | `requesting_device_ref`, `selected_device_ref`, step device refs, current Bridge refs | 语义可复用，v1 表示不够 | Plan schema v2 + protocol v2 的 participant/HostRef 与 session binding |
| Managed object import | candidate/requester/pipeline stores、ObjectRef、safe identity、Inbox persistence | 安全基础可复用，入口不通用 | generic acquisition/binding service；不再把 Search 或 `selected_file` 当唯一根 |
| Semantic/effect policy | semantic intents、attempt/step grants、safe object identity | 基础正确，effect envelope 未实现 | 新 Core-owned envelope/compiler + Host enforcement；不在 Provider/Harness 中定义 authority |

### 5.1 当前可保留的具体边界

- `BridgePlanRevision`、`BridgePlanStep`、`LogicalObjectRevision`、canonical semantic hash：继续作为 managed semantic IR 和授权锚点。
- `BridgePlanStore::create_attempt_from_approval`：继续作为 Core attempt admission 的深层防线。
- receiver `accept_start`：继续作为 Host 侧 current-session protocol admission；未来 Host policy 放在创建本地 attempt/grant 之前。
- `BridgePlanStore::authorize_next_eligible_transfer` 与 `continue_bridge_plan_attempt_inner`：证明当前可以从 immutable attempt state 原子领取下一步骤；未来只抽取 primitive-neutral dispatch，不扩展成通用 scheduler。
- `TransferCapacityCoordinator`：继续作为 Layer 3 资源边界；semantic eligibility 不下沉。
- `ObjectRefStore`、candidate stores 与 `safe_file_identity`：继续作为 Host 私有对象解析和物理身份基础；不向 renderer/Harness 暴露路径。
- Room Control：继续作为 Layer 4 typed encrypted control transport，不理解 PM/Worker 语义。

### 5.2 当前需要隔离的 Tauri 交叉点

- `AppState` 同时持有 `AppHandle` 与 Core stores/runtime。
- `main.rs` setup 同时负责 path bootstrap、DB/recovery、Burn cleanup、discovery、tray/window/plugin。
- `commands.rs` 以 `tauri::State<Arc<AppState>>` 暴露业务入口。
- `discovery.rs` 与 `transfer.rs` 直接通过 `AppHandle.emit` 发 UI 事件。
- cleanup、commands、room control 使用 `tauri::async_runtime::spawn`。

这些是实现容器耦合，不是 Layer 1–5 语义冲突。最小解法是注入 `HostEventSink`、显式 `AppPaths`、runtime task interface，并把 command wrapper 与 service function 分开。

## 6. Managed Workspace 数据与控制流

Managed Workspace 的所有入口最终只产生候选 Plan 或对象绑定请求：

```text
GUI block edits ──────────────┐
Inbox / drag / local object ──┼─► object binding + candidate semantic Plan
Local Interpreter ────────────┤
Explicit PM Agent ────────────┘
                                      │
                                      ▼
                           deterministic Core validation
                                      │
                                      ▼
                            immutable Review & Run Plan
                                      │
                                      ▼
                       requester approval + Host admission
                                      │
                                      ▼
                      Host coordinator / per-step execution
```

GUI、local model 与 PM 共享相同的 proposal contract，但可以有不同体验。任何来源都不能直接构造 process-local ObjectRef、consume approval 或 create grant。Rust/Core 负责把用户可读 Host/object 选择解析到 current-session 身份，把语义 lower 到不可变 revision，并重新验证。

PM 负责 WHAT / WHERE / ORDER。Worker 只在 Core 原子认领一个已批准 Transform/Execute step 后负责 HOW。Worker 完成 Transform 后，只有 Core 在验证真实 effect/result 后才能登记同一 logical object 的 N+1；Execute 读取 exact current revision，结果默认是执行结果而不是另一个 filesystem object。

## 7. Developer Mode

Developer Mode v0 已实现为**平行于 Layer 5、位于 Layer 4 与 HostRuntime 之上的独立 Host capability domain**。其当前协议、权限和生命周期以 [Developer Mode](developer-mode.md) 为准；以下仍是长期边界。

它复用：

- Layer 4 current-session Host identity、route 生命周期、encrypted control foundation、disconnect/replay/Burn 边界；
- HostRuntime 的配置、任务、审计与本地 admission；
- 必要时由 Layer 1/4 演进出的适合交互流量的加密 channel。

它不复用：

- 四原语来分类每条 shell 指令；
- managed Plan approval 作为 terminal grant；
- Agent Harness 作为 terminal transport；
- logical revision 追踪任意 shell 副作用。

当前 v0 复用 Room Control 的会话身份、加密 envelope、route、expiry 与 replay 基础，并用独立 typed delivery 分支绕过普通 control inbox/history。它没有建立第二套 peer/session/crypto 系统。未来若引入更高吞吐量的专用流式 channel，仍必须保留同样的 identity、admission、grant 与 Burn 合同。

## 8. Multi-Host 模型

当前 v1 的两方结构不是未来 Host ontology：顶层 `requesting_device_ref` / `selected_device_ref`、前端 `requesting` / `selected` role、协议中的 requester/receiver correlation、Bridge peer persistence 都以一次会话中的一对角色组织。

未来概念模型应为：

```text
PlanRevision
  requester: HostRef
  participants: HostRef[]
  steps:
    Search    { host: HostRef, ... }
    Transform { host: HostRef, ... }
    Transfer  { source: HostRef, destination: HostRef, ... }
    Execute   { host: HostRef, ... }

HostSessionBinding
  HostRef + current Bridge/session/peer identity + expiry
```

`HostRef` 是 Core-owned Plan participant identity，不是 route，不是 capability fact，也不应仅等于当前 display-only durable pairing。批准时每个 HostRef 必须绑定到被审查的身份；执行时用 current-session binding 重新关联并验证。

### 8.1 保持不变

- immutable revision 与 semantic hash；
- 每步显式 Host 与依赖；
- 只有 Transfer 改 location；
- exact Plan/revision/attempt/step authority；
- Layer 4 session binding 与 Layer 5 consent 分离；
- capability facts 只作为观察。

### 8.2 迁移建议

采用 **Plan schema v2 + Bridge Plan protocol v2**，而不是在 v1 内就地扩大。理由：v1 的 exact hash、deny-unknown serialization、顶层 two-party refs、逐消息 requester/selected correlation、receiver persistence 与 replay key 都是权限合同的一部分。v2 应与 v1 明确共存或显式拒绝，不能把旧 revision 静默重解释。

每个 Host 可以收到完整不可变 revision，或收到锚定 full-plan hash 的 Host projection；两种选择都必须保留对完整拓扑与 exact step 的可验证关联。Layer 4 仍只负责把协议消息送到对应 current-session peer。

## 9. Managed Object 接入模型

必须区分以下概念：

| 概念 | 定义 | 权限含义 |
| --- | --- | --- |
| Physical Artifact | 某 Host 上的真实文件/目录/字节 | 本身不是行为授权 |
| Managed Logical Object | Core-owned 稳定逻辑身份 | 用于 Plan 引用与 lineage |
| Logical Revision | 对逻辑对象状态的有序语义版本 | 必须由验证过的 acquisition/effect 建立 |
| Host Location | exact revision 当前存在的 Host | 只有显式 Transfer 可改变 |
| Session Binding | 当前 Host 进程内把 opaque reference 解析到安全物理身份的短期绑定 | 不可作为 approval/grant |

可能的 acquisition 路径：

```text
Search result ──────────────┐
Inbox item ─────────────────┤
drag/drop or local choice ──┼─► Host-local safe validation/import
future generated artifact ──┘              │
                                           ▼
                         ManagedLogicalObject revision N @ Host
```

Acquisition/binding 是进入 managed workspace 的前置 Core 操作，不是第五原语，也不等于 Transform。Search 仍是“find”原语；它只是能产生对象绑定的一种行为。普通 Bridge transfer 先按现有机制把 physical artifact 落到 Inbox；以后只有用户或受验证的 workflow 明确 import/bind 后，它才成为 managed logical object。

当前 `selected_file`、`ObjectKind::FilesystemCandidate`、Search-first Composer 与 direct transfer source binding 都是 MVP 表示。未来应抽成 generic root/input slot 与 object-binding service，同时复用 safe identity、ObjectRef privacy 与 location rules。

## 10. HostRuntime 模型

### 10.1 PasteyHostRuntime 应拥有

- Layer 1 transfer engine 与 Layer 3 capacity coordinator；
- Layer 2 factual probes 与 capability store；
- Layer 4 Room Control、peer/session/runtime、replay 与 Burn；
- Layer 5 Plan/approval/attempt/protocol authority stores 与 Host coordinator；
- ObjectRef、candidate、safe identity 与 managed object binding；
- storage paths/config、startup reconciliation、restart invalidation、cleanup/TTL；
- 当前 Developer Terminal service，以及未来 generic Host admission 与 Worker tool enforcement；
- UI-independent event/result stream。

### 10.2 DesktopAdapter / TauriShell 应拥有

- `tauri::Builder`、state injection 与 invoke registration；
- window/tray/global shortcut；
- dialog/opener/clipboard/update/autostart plugins；
- OS desktop path discovery；
- 把 runtime events 映射为 frontend events；
- 把 invoke DTO 映射到 HostRuntime service calls。

### 10.3 HeadlessAdapter 应拥有

- daemon/service startup 与 shutdown；
- service config 与 path provider；
- RPC/CLI/admin adapter；
- structured log/event sink；
- 与 Desktop 相同的 HostRuntime initialization/reconciliation。

当前抽取成本是中等但局部：安全与语义模块多数已经是普通 Rust；主要耦合集中于 `AppState`、setup、Tauri command wrapper、event emission、path bootstrap 与 async spawning。正确抽取不会改变冻结语义。

## 11. Agent Harness 模型

### 11.1 PM 与 Worker

```text
PM Agent
  input: user goal + bounded product facts
  output: CandidateSemanticPlan
  authority: none

Worker Harness
  input: one approved StepWorkDescriptor
  output: ToolRequest / Observation / StepResultProposal / Failure
  authority: none by itself

Pastey Core + Host Enforcement
  input: exact plan/step/grant + request/result evidence
  output: allowed effect, authoritative state transition, lineage
  authority: authoritative
```

`StepWorkDescriptor` 至少应锚定 Plan ID、revision hash、attempt、step、Host、semantic intent、input logical revision 与 effect-envelope reference。它不是可转授权 bearer token；Host tool broker 对每个请求仍校验 process-local grant、expiry/session/Burn。

### 11.2 Harness 可拥有

- model/provider lifecycle；
- reasoning/context/observation loop；
- tool selection 与 request construction；
- retry/self-correction；
- Worker run 内部状态。

### 11.3 Harness 不可拥有

- Plan topology、Host selection 或 hidden Transfer；
- semantic approval、Host admission 或 step grant creation；
- object identity/revision registration；
- Developer Mode escalation；
- 绕过 Host tool enforcement 的 raw filesystem/process/network authority。

当前 Host coordinator 的“读取 immutable attempt → 原子认领下一 eligible step → 按 primitive dispatch”是未来调用 Harness 的正确位置。只需把 command/Tauri 依赖从 coordinator service 抽离，并在 Transform/Execute 真正实现时增加受控 dispatch。Harness 不得复制 `BridgePlanStore` 或成为第二个 Layer 5 Core。

## 12. 本地 2–4B 模型角色

本地小模型只走简化提案链：

```text
user language
   ↓
local constrained interpreter
   ↓
CandidateSemanticPlan
   ↓
deterministic schema + topology validation
   ↓
Rust/Core lowering
   ↓
Review & Run
```

当前 natural-v1 已提供重要基础：受限 schema、严格 validator、risk scanner、实际原语序列标题、Transform/Execute `unsupported_future`、provider 输出非权限。它仍有 Search-first、two-role 与 TypeScript-only proposal shape 等 MVP 假设，需要随 object-root/Multi-Host 表示迁移。

本地 interpreter 默认不获得 Worker tool loop、filesystem、shell、network 或云 provider。强 Agent 必须由用户显式选择；禁止 local-to-cloud 自动升级。云/本地模型的差异只影响 proposal producer 或 Harness adapter，不影响 Core authority。

## 13. 具体场景

### A. 本地助手：在 PC 找昨天的报告并发到 laptop

1. UI 把语言与用户可选 Host 事实交给本地 interpreter。
2. interpreter 提出 `Search @ PC → Transfer PC → laptop`；没有权限。
3. Core 解析 Host/session、验证 scope/dependency/location，生成不可变 revision。
4. 用户在 Review & Run 批准 exact Plan。
5. PC 与 laptop 的 Host admission 分别接纳其相关工作；route 本身不构成接纳。
6. Layer 5 建立 attempt/Search grant；candidate selection 只选择对象。
7. authored Transfer 变为 eligible；Layer 3 给出 capacity，Layer 4 提供 current route，Layer 1 加密传输。
8. Core 记录完成与目标 location。任何层都没有插入移动。

### B. 强 Agent：在 Linux 修项目，再到 Mac 运行测试

1. 用户显式选择强 PM；PM 提出 `Transform @ Linux → Transfer Linux→Mac → Execute @ Mac`，并引用已绑定项目对象（也可以显式先 Search）。
2. Core 验证 exact input revision、Host locality 与显式 Transfer，生成 Review。
3. 用户批准；每个 Host 独立 admission。
4. Linux Core 为 exact Transform step 建立 authority，Worker Harness 决定 HOW，并通过 Host tool enforcement 请求受限操作。
5. 只有 effect 验证成功后，Core 登记同一 logical object 的 N+1 @ Linux。
6. authored Transfer 经 Layer 3/4/1 把 N+1 移到 Mac。
7. Mac Worker 接收 exact Execute step 与 N+1，Harness 决定 HOW；Core 记录受验证 execution result，不默认创建文件对象。
8. Worker 不能改成另一 Host、跳过 Transfer 或新增步骤。

当前 Transform/Execute 未实现，因此这类 Plan 现在仍在 attempt admission 前整体拒绝；以上是未来合同，不是当前功能声明。

### C. drag/drop 后再让 Agent 修改

1. 用户通过普通 Bridge drag/drop 发送文件；现有 Transfer 将 physical artifact 落入 receiver Inbox。
2. 后续用户选择“刚发送的文件”。未来 object binder 在 receiver Host 上安全重验 physical identity，并建立 logical object revision/location/session binding。
3. UI/PM 从该 bound object 开始提出 Transform Plan；不需要虚构 Search step。
4. import/binding 不授予修改；仍需 immutable Review、requester approval、Host admission 与 exact step authority。

### D. Headless Linux Developer Mode

1. Headless daemon 已通过现有 Bridge enrollment/session 基础连接。
2. Mac 上的用户显式进入 Developer Mode 并选择 Host。
3. Host admission 根据 exact current identity/session 与本地 terminal policy 决定是否建立独立 `DeveloperTerminalGrant`。
4. Terminal service 打开 native PTY/console，流量走专用加密 channel。
5. 用户退出、session 断开、expiry 或 Burn 时 grant 与 PTY/process tree 按未来 terminal policy 终止。
6. Agent 和 Layer 5 Plan 均不参与；terminal 副作用不伪装成 managed lineage。

### E. Headless Linux Managed Agent Mode

1. PM 提案、Core validation、Review、approval 与 Host admission 与 Desktop Host 相同。
2. Layer 2/Host probes 把 OS、工具、liveness 等有界观察传给 Worker Harness；这些只是事实。
3. Worker 在 exact semantic step 内根据观察选择 HOW，所有真实 tool request 都经过 Host enforcement。
4. 未知 Linux 环境导致 observation/unsupported/denied，而不是 capability 驱动换 Host 或 hidden Transfer。
5. Core 独立记录 authoritative state 与 result lineage；Headless adapter 只提供 service lifecycle 与 remote presentation channel。

## 14. 未来接口变化分类

| 变化 | 分类 | 当前触点 | 是否改变冻结语义 |
| --- | --- | --- | --- |
| `HostRuntimeState` / service 从 `AppState` 分离 | 孤立接口抽取 | `main.rs`, `commands.rs` | 否 |
| `HostEventSink`、path provider、runtime spawner | 孤立接口抽取 | `main.rs`, `discovery.rs`, `transfer.rs`, cleanup | 否 |
| Tauri command wrapper 与业务 service 分离 | 孤立接口抽取 | `commands.rs`, invoke registration | 否 |
| primitive-neutral coordinator dispatch seam | 孤立接口抽取 | `continue_bridge_plan_attempt_inner`, `BridgePlanStore` | 否；必须保留 whole-plan fail-closed 直到实现存在 |
| Worker Harness adapter + tool request/result contract | 孤立接口抽取 | 未来接 coordinator；复用 attempt/step correlation | 否 |
| two-party → HostRef/participants | 表示迁移（schema v2/protocol v2） | `bridge_plan.rs`, protocol, storage, composer/UI | 否 |
| Search-first `selected_file` → generic bound input | 表示迁移 | composer, revision builder, ObjectRef/candidate/Inbox | 否 |
| Host identity / HostRef contract | 表示合同先行 | current device refs、Bridge identity/session | 否；必须先于 Host admission，不能把 admission 固化在 temporary two-party roles 上 |
| Host admission | 新权限域 | receiver admission、HostRuntime | 否；在 HostRef/HostSessionBinding 合同之后增加额外 fail-closed 条件 |
| semantic/effect envelope 与 Host tool enforcement | 新权限域 | exact step grants、safe identity、Burn | 否；实现 Transform/Execute 所需但不在本阶段设计 policy |
| Developer Terminal authority/channel | 新权限域（v0 已实现） | `host_runtime.rs`, `developer_terminal.rs`, Layer 4 identity/session/Burn | 否；平行于 Layer 5；后续只扩展 headless/persistence 表示 |
| Headless adapter/service binary | 孤立接口抽取后的新 adapter | runtime bootstrap | 否 |

没有发现必须改变四原语、显式 Transfer 或 immutable authority 模型的 fundamental conflict。

## 15. 编码代理必须遵守的架构不变量

1. Search=find，Transform=modify，Transfer=move，Execute=run；它们是内部 managed IR，不是用户命令语言。
2. 只有不可变 Plan 中显式创作的 Transfer 才能改变 object location；不得 capability-driven、Agent-driven 或 convenience-driven 自动移动。
3. Transform 消耗 exact N，在同一 Host 概念性地产生 N+1；Execute 消耗 exact current revision 且不默认产生 filesystem object。
4. provider、PM、local model、Worker、renderer、capability fact、ObjectRef 与 Layer 4 route 都不是 authority。
5. Core 独占 Host/object identity、logical revision、topology、semantic hash、approval、attempt、step grant 与 result lineage。
6. requester approval、Host admission、Layer 5 step authority、Harness tool permission、effect enforcement、DeveloperTerminalGrant 是不同权限域。
7. Worker 只能为一个 exact approved semantic step 决定 HOW；不得改 Host、拓扑、语义范围或添加 Transfer。
8. Harness 不得持有或生成持久 Host authority；每个 effect request 由 Host enforcement fail closed。
9. Developer Mode 平行于 Layer 5，只有人类显式进入；Agent 不能获得或升级为 terminal authority。
10. Layer 5 决定 semantic eligibility；Layer 3 决定 transport capacity；Layer 4 提供 current-session route/control；Layer 1 执行加密 transfer；Layer 2 只提供事实。
11. Tauri/Headless adapters 不得拥有或重建 Core authority。
12. physical path、safe identity 与 ObjectRef resolution 保留在 owning Host；跨边界只传 opaque、bounded、correlated references。
13. acquisition/binding 不是第五原语，也不是 modification authority；Search 不是唯一对象来源。
14. Multi-Host migration 必须保持 explicit per-step Host、full semantic hash、session correlation 与 per-step authority，不能静默重解释 v1。
15. Transform/Execute 在真实 Host implementation 与 enforcement 存在前仍是 whole-plan non-executable；不得部分执行前置 Search/Transfer。
16. restart、disconnect、expiry 与 Burn 必须使 process-local execution material/terminal authority fail closed。

## 16. Freeze 边界

### 16.1 结构冻结

- 四 primitive 的语义与 location rules；
- immutable semantic Plan、reviewed topology 与 logical revision dependency；
- provider/model/renderer non-authority；
- capability observation non-authority；
- Layer 4 route/session 与 Layer 5 approval 分离；
- requester approval、Host-local execution authority 与 per-step grants 分离；
- Layer 5 eligibility → Layer 3 capacity → Layer 4 session/control → Layer 1 transport 的依赖方向；
- safe object identity、one-use authority、restart/Burn fail-closed foundations。
- Agent authority 与 Developer Terminal authority 永久分离。

### 16.2 有意不冻结

- `requesting_device` / `selected_device` 的 two-party schema；
- `selected_file`、Search-first Composer 与 filesystem-candidate-only root；
- Bridge Plan protocol v1 的 requester/receiver wire representation；
- `AppState` / Tauri-only Host container；
- Host admission policy 的规则语言；
- future semantic/effect envelope、tool set、Harness/provider 实现；
- Developer Terminal channel 与 containment；
- Headless deployment/management 机制。

表示迁移不得借机改变结构冻结的语义合同。

## 17. 实现依赖顺序

### Phase 1 — HostRuntime seam

从当前 Tauri `AppState` 抽取/定义 UI-independent HostRuntime boundary，保持现有 Desktop 行为。本阶段不实现 Headless deployment。

### Phase 2 — Host identity / HostRef contract

先定义 `HostRef`、Plan participants、`HostSessionBinding`，以及 durable/logical Host identity 与当前 Layer 4 session binding 的区别。此阶段可以先完成合同设计，不要求立即迁移全部 wire/storage 表示。

**Host admission 不得直接围绕临时 `requesting_device` / `selected_device` 表示实现。** 否则本地 policy 和 grant 会错误固化 two-party role，而不是绑定稳定的 Plan participant/Host identity。

### Phase 3 — Host admission + generic managed-object binding

在 Host identity 边界确定后，定义 Host-local admission 与通用 managed object acquisition/binding。Binding 必须能扩展到 Search、Inbox、drag/drop、本地选择和 future generated artifact；它不是第五 primitive。

### Phase 4 — Multi-Host representation migration

把 two-party Plan、schema、protocol、persistence/correlation 迁移到 HostRef/participants、Plan schema v2 与 Bridge Plan protocol v2，同时保持 immutable Plan、exact Host、explicit Transfer、route-not-consent 和 exact step authority。

### Phase 5 — Effect / control capability domains

Developer Terminal v0 authority/channel 已作为独立人工权限域实现。此阶段剩余工作是定义并实现 Worker Host effect enforcement 与 semantic/effect envelope；Managed Agent effect authority 与人工 Terminal authority 不得合并或相互升级。

### Phase 6 — Concrete upper implementations

只有完成上述基础后，才实现 Headless Host daemon/service、本地 2–4B interpreter、Codex-style Worker Harness 或具体 Transform/Execute capability。Developer Terminal v0 已完成最小 desktop vertical slice；headless admission、持久 session 与更完整 terminal emulator 仍依赖后续 HostRuntime/HostRef 工作。

此顺序是依赖关系，不是功能承诺或完整实施计划。

## 18. 代码证据与当前状态

本设计核对了当前本地工作树中的：

- runtime/container：`src-tauri/src/main.rs`、`AppState`、Tauri setup/commands/events/paths；
- Layer 5：`bridge_plan.rs`、`bridge_plan/protocol.rs`、`commands.rs` 的 revision、approval、attempt、receiver admission、continuation；
- lower layers：`transfer.rs`、`transfer_orchestration.rs`、`room_control.rs`、`peer_capabilities.rs`、storage/session/Burn；
- object/security：`object_refs.rs`、`file_candidates.rs`、`safe_file_identity.rs`；
- frontend/planning：`bridgePlanComposer.ts`、`BridgeProductPages.tsx`、natural-v1、provider instruction/risk scanner、ordinary transfer/Inbox paths；
- canonical layer/reference/development documentation。

代码证据确认当前：

- Search/Transfer 可执行；Transform/Execute framework-only 且 whole-plan fail closed；
- requester command、store-level attempt admission 与 receiver protocol 均保留深层校验；
- next-step continuation 依据 immutable attempt state，managed/ordinary Transfer 共用 Layer 3 capacity boundary；
- capability projection 可为空且始终是 observation；
- 没有 Agent Harness、Worker runtime、managed shell/process runtime 或 patch/mutation engine。Developer Mode v0 的人工 PTY/ConPTY runtime 是独立权限域，不是 Execute/Agent implementation。

Multi-Host、generic object import、generic/headless Host admission policy、Agent effect envelope 与 Headless Host 仍是概念合同。Developer Mode v0 有本机 Unix PTY 自动化和 Windows cross-compile 证据，但自动化与 cross-compilation 不能证明物理 Mac↔Windows/Linux E2E。
