/** Renderer editing state. Rust remains the immutable revision authority. */
export type SafeSearchScope = "downloads" | "desktop" | "documents" | "pastey_shared";
export type ComposerPrimitive = "Search" | "Transform" | "Transfer" | "Execute";
export type ComposerDevice = "requesting_device" | "selected_device";
export type ComposerTransferDestination = ComposerDevice | "pastey_shared";
export type TransferLandingMode = "pipeline_handoff" | "final_delivery";

export const SAFE_SEARCH_SCOPES: ReadonlyArray<{ value: SafeSearchScope; label: string }> = [
  { value: "downloads", label: "Downloads" },
  { value: "desktop", label: "Desktop" },
  { value: "documents", label: "Documents" },
  { value: "pastey_shared", label: "Pastey Shared" },
];

export type LogicalObjectRevision = {
  logicalObjectId: "selected_file";
  revision: number;
};

export type SearchBlock = {
  primitive: "Search";
  executionDevice: "selected_device";
  filenameHint: string;
  extension: string;
  safeScopes: SafeSearchScope[];
};

export type TransformBlock = {
  primitive: "Transform";
  executionDevice: ComposerDevice;
  targetRevision: LogicalObjectRevision;
  modificationIntent: string;
};

export type TransferBlock = {
  primitive: "Transfer";
  source: ComposerDevice;
  destination: ComposerTransferDestination;
  landingMode: TransferLandingMode;
};

export type ExecuteBlock = {
  primitive: "Execute";
  executionDevice: ComposerDevice;
  targetRevision: LogicalObjectRevision;
  executionIntent: string;
};

export type ComposerBlock = SearchBlock | TransformBlock | TransferBlock | ExecuteBlock;
export type ManualBridgePlanInput = { blocks: ComposerBlock[]; originalUserGoal: string };
export type ObjectFlowResult = { location: ComposerDevice | null; revision: number | null; error: string | null };

export function newSearchBlock(): SearchBlock {
  return { primitive: "Search", executionDevice: "selected_device", filenameHint: "", extension: "", safeScopes: ["downloads"] };
}

export function newTransformBlock(executionDevice: ComposerDevice = "selected_device", revision = 1): TransformBlock {
  return {
    primitive: "Transform",
    executionDevice,
    targetRevision: { logicalObjectId: "selected_file", revision },
    modificationIntent: "",
  };
}

export function newTransferBlock(source: ComposerDevice = "selected_device"): TransferBlock {
  return {
    primitive: "Transfer",
    source,
    destination: source === "selected_device" ? "requesting_device" : "selected_device",
    landingMode: "final_delivery",
  };
}

export function newExecuteBlock(executionDevice: ComposerDevice = "selected_device", revision = 1): ExecuteBlock {
  return {
    primitive: "Execute",
    executionDevice,
    targetRevision: { logicalObjectId: "selected_file", revision },
    executionIntent: "",
  };
}

export function requiredTransferForConsumer(blocks: readonly ComposerBlock[], index: number): TransferBlock | null {
  const consumer = blocks[index];
  if (consumer?.primitive !== "Transform" && consumer?.primitive !== "Execute") return null;
  const source = objectFlow(blocks.slice(0, index)).location;
  if (!source || source === consumer.executionDevice) return null;
  return { primitive: "Transfer", source, destination: consumer.executionDevice, landingMode: "pipeline_handoff" };
}

/** Explicit convenience edit. Nothing is inserted until the user invokes it. */
export function insertRequiredTransfer(blocks: readonly ComposerBlock[], consumerIndex: number): { blocks: ComposerBlock[]; error: string | null } {
  const transfer = requiredTransferForConsumer(blocks, consumerIndex);
  if (!transfer) return { blocks: [...blocks], error: null };
  const next = [...blocks];
  next.splice(consumerIndex, 0, transfer);
  return { blocks: next, error: dependencyError(next) };
}

export function primitives(blocks: readonly ComposerBlock[]): ComposerPrimitive[] {
  return blocks.map((block) => block.primitive);
}

/** Resolves only authored locality and semantic revision dependencies. */
export function objectFlow(blocks: readonly ComposerBlock[]): ObjectFlowResult {
  let location: ComposerDevice | null = null;
  let revision: number | null = null;
  for (const block of blocks) {
    if (block.primitive === "Search") {
      if (location) return { location, revision, error: "Search starts a new object flow; remove the earlier flow before adding another Search." };
      location = block.executionDevice;
      revision = 1;
      continue;
    }
    if (!location || revision === null) {
      return { location, revision, error: `${block.primitive} needs an available object before it can run.` };
    }
    if (block.primitive === "Transform") {
      if (location !== block.executionDevice) {
        return { location, revision, error: `The object is on ${deviceDescription(location)}. Add an explicit Transfer before modifying it on ${deviceDescription(block.executionDevice)}.` };
      }
      if (block.targetRevision.revision !== revision) return { location, revision, error: "Transform must consume the current logical object revision." };
      revision += 1;
      continue;
    }
    if (block.primitive === "Execute") {
      if (location !== block.executionDevice) {
        return { location, revision, error: `The object is on ${deviceDescription(location)}. Add an explicit Transfer before executing it on ${deviceDescription(block.executionDevice)}.` };
      }
      if (block.targetRevision.revision !== revision) return { location, revision, error: "Execute must consume the current logical object revision." };
      continue;
    }
    if (block.source !== location) return { location, revision, error: `Transfer source must be ${deviceDescription(location)}, where the object is currently located.` };
    if (block.destination === "pastey_shared" && block.source !== "selected_device") {
      return { location, revision, error: "Pastey Shared final delivery is available only while the object is on the selected device." };
    }
    const destination = block.destination === "pastey_shared" ? "selected_device" : block.destination;
    if (destination === block.source && block.landingMode === "pipeline_handoff") {
      return { location, revision, error: "A private pipeline Transfer must move the object to another device." };
    }
    location = block.landingMode === "pipeline_handoff" ? destination : null;
    if (!location) revision = null;
  }
  const last = blocks.length > 0 ? blocks[blocks.length - 1] : undefined;
  if (last?.primitive === "Transfer" && last.landingMode === "pipeline_handoff") {
    return { location, revision, error: "A private pipeline Transfer needs a following step that consumes its object." };
  }
  return { location, revision, error: null };
}

export function currentObjectLocation(blocks: readonly ComposerBlock[]): ComposerDevice | null {
  return objectFlow(blocks).location;
}

export function dependencyError(blocks: readonly ComposerBlock[]): string | null {
  return objectFlow(blocks).error;
}

export function canAddPrimitive(blocks: readonly ComposerBlock[], primitive: ComposerPrimitive): boolean {
  return dependencyError([...blocks, blockForPrimitive(blocks, primitive)]) === null;
}

export function addPrimitive(blocks: readonly ComposerBlock[], primitive: ComposerPrimitive): { blocks: ComposerBlock[]; error: string | null } {
  const next = [...blocks, blockForPrimitive(blocks, primitive)];
  const error = dependencyError(next);
  return { blocks: error ? [...blocks] : next, error };
}

export function removeBlock(blocks: readonly ComposerBlock[], index: number): { blocks: ComposerBlock[]; error: string | null } {
  const next = blocks.filter((_, current) => current !== index);
  const error = dependencyError(next);
  return { blocks: error ? [...blocks] : next, error };
}

export function moveBlock(blocks: readonly ComposerBlock[], from: number, to: number): { blocks: ComposerBlock[]; error: string | null } {
  if (from === to || from < 0 || to < 0 || from >= blocks.length || to >= blocks.length) return { blocks: [...blocks], error: null };
  const next = [...blocks];
  const [block] = next.splice(from, 1);
  next.splice(to, 0, block!);
  const error = dependencyError(next);
  return { blocks: error ? [...blocks] : next, error };
}

export function updateSearchBlock(block: SearchBlock, patch: Partial<Omit<SearchBlock, "primitive">>): SearchBlock {
  const extension = (patch.extension ?? block.extension).trim().replace(/^\./, "").toLowerCase();
  return {
    ...block,
    ...patch,
    filenameHint: (patch.filenameHint ?? block.filenameHint).slice(0, 128),
    extension: extension.replace(/[^a-z0-9]/g, "").slice(0, 16),
  };
}

export function manualBridgePlanInput(blocks: readonly ComposerBlock[]): { value?: ManualBridgePlanInput; error?: string } {
  const flow = objectFlow(blocks);
  if (flow.error) return { error: flow.error };
  const search = blocks.find((block): block is SearchBlock => block.primitive === "Search");
  if (!search) return { error: "Search is required for this composed plan." };
  const filenameHint = search.filenameHint.trim();
  if (!filenameHint) return { error: "Enter the filename or file description to search for." };
  if (!search.safeScopes.length || search.safeScopes.some((scope) => !SAFE_SEARCH_SCOPES.some((entry) => entry.value === scope))) {
    return { error: "Choose one or more reviewed Search locations." };
  }
  for (const block of blocks) {
    if (block.primitive === "Transform") {
      const intent = boundedIntent(block.modificationIntent, "Describe the modification intent for Transform.");
      if (intent) return { error: intent };
    }
    if (block.primitive === "Execute") {
      const intent = boundedIntent(block.executionIntent, "Describe the execution intent for Execute.");
      if (intent) return { error: intent };
    }
  }
  return {
    value: {
      blocks: blocks.map((block) => block.primitive === "Search"
        ? { ...block, filenameHint, safeScopes: [...block.safeScopes] }
        : { ...block }) as ComposerBlock[],
      originalUserGoal: manualGoal(blocks, filenameHint),
    },
  };
}

export function manualGoal(blocks: readonly Pick<ComposerBlock, "primitive">[], filenameHint: string): string {
  return `${blocks.map((block) => block.primitive).join(" → ")}: ${filenameHint}`;
}

export function reviewedObjectFlow(blocks: readonly ComposerBlock[]): string[] {
  return blocks.map((block) => {
    if (block.primitive === "Search") return `Search @ ${block.executionDevice}`;
    if (block.primitive === "Transform") return `Transform @ ${block.executionDevice}: revision ${block.targetRevision.revision} → ${block.targetRevision.revision + 1}; ${block.modificationIntent}`;
    if (block.primitive === "Execute") return `Execute @ ${block.executionDevice}: revision ${block.targetRevision.revision}; ${block.executionIntent}`;
    return `Transfer ${block.source} → ${block.destination} (${block.landingMode})`;
  });
}

function blockForPrimitive(blocks: readonly ComposerBlock[], primitive: ComposerPrimitive): ComposerBlock {
  if (primitive === "Search") return newSearchBlock();
  const flow = objectFlow(blocks);
  const location = flow.location ?? "selected_device";
  const revision = flow.revision ?? 1;
  if (primitive === "Transform") return newTransformBlock(location, revision);
  if (primitive === "Execute") return newExecuteBlock(location, revision);
  return newTransferBlock(location);
}

function boundedIntent(value: string, missing: string): string | null {
  const trimmed = value.trim();
  if (!trimmed) return missing;
  if (trimmed.length > 1024) return "Intent text must be 1,024 characters or fewer.";
  return null;
}

function deviceDescription(device: ComposerDevice): string {
  return device === "requesting_device" ? "the requesting device" : "the selected device";
}
