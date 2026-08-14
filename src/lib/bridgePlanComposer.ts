/**
 * Renderer-only editing state for bounded Bridge Plan object-flow primitives.
 * It is deliberately not a revision format: Rust binds sessions, slots,
 * object ownership and immutable revision identity before a plan is stored.
 */

export type SafeSearchScope = "downloads" | "desktop" | "documents" | "pastey_shared";
export type ComposerPrimitive = "Search" | "Transform" | "Transfer";
export type ComposerDevice = "requesting_device" | "selected_device";
export type ComposerTransferDestination = ComposerDevice | "pastey_shared";
export type TransferLandingMode = "pipeline_handoff" | "final_delivery";

export const SAFE_SEARCH_SCOPES: ReadonlyArray<{ value: SafeSearchScope; label: string }> = [
  { value: "downloads", label: "Downloads" },
  { value: "desktop", label: "Desktop" },
  { value: "documents", label: "Documents" },
  { value: "pastey_shared", label: "Pastey Shared" },
];

export type SearchBlock = {
  primitive: "Search";
  executionDevice: "selected_device";
  filenameHint: string;
  extension: string;
  safeScopes: SafeSearchScope[];
};
export type TransformBlock = {
  primitive: "Transform";
  intent: "extract readable text";
  executionDevice: ComposerDevice;
};
export type TransferBlock = {
  primitive: "Transfer";
  destination: ComposerTransferDestination;
  landingMode: "final_delivery";
};
export type DerivedPipelineTransferBlock = {
  primitive: "Transfer";
  source: ComposerDevice;
  destination: ComposerDevice;
  landingMode: "pipeline_handoff";
  derived: true;
  reason: string;
};
export type ComposerBlock = SearchBlock | TransformBlock | TransferBlock;
export type VisibleComposerBlock = ComposerBlock | DerivedPipelineTransferBlock;

export type TransformAvailability = {
  intent: "extract readable text";
  status: "unknown" | "available" | "unavailable";
  available: boolean;
  reason: string;
  hostLabel: string;
  acceptedInputMediaTypes?: string[];
  outputMediaType?: string | null;
};

export type TransformExecutorCapabilities = Record<ComposerDevice, TransformAvailability>;

export type ManualBridgePlanInput = {
  blocks: ComposerBlock[];
  visibleBlocks: VisibleComposerBlock[];
  filenameHint: string;
  extensions: string[];
  safeScopes: SafeSearchScope[];
  transformIntent?: "extract readable text";
  transformExecutionDevice?: ComposerDevice;
  transferDestination?: ComposerTransferDestination;
  originalUserGoal: string;
};

export function newSearchBlock(): SearchBlock {
  return { primitive: "Search", executionDevice: "selected_device", filenameHint: "", extension: "", safeScopes: ["downloads"] };
}

export function newTransformBlock(executionDevice: ComposerDevice = "requesting_device"): TransformBlock {
  return { primitive: "Transform", intent: "extract readable text", executionDevice };
}

/** Selects the sole available executor, otherwise keeps the deterministic local-first default. */
export function initialTransformExecutionDevice(capabilities: TransformExecutorCapabilities): ComposerDevice {
  const available = (["requesting_device", "selected_device"] as const)
    .filter((device) => capabilities[device].status === "available" && capabilities[device].available);
  return available.length === 1 ? available[0]! : "requesting_device";
}

export function newTransferBlock(): TransferBlock {
  return { primitive: "Transfer", destination: "requesting_device", landingMode: "final_delivery" };
}

export function primitives(blocks: readonly ComposerBlock[]): ComposerPrimitive[] {
  return blocks.map((block) => block.primitive);
}

/** Resolves the linear product editor to explicit object locations. */
export function objectFlow(blocks: readonly ComposerBlock[]): { visibleBlocks: VisibleComposerBlock[]; error: string | null } {
  let location: ComposerDevice | null = null;
  const visibleBlocks: VisibleComposerBlock[] = [];
  for (const block of blocks) {
    if (block.primitive === "Search") {
      if (location) return { visibleBlocks, error: "Search starts a new object flow; remove the earlier flow before adding another Search." };
      location = block.executionDevice;
      visibleBlocks.push(block);
      continue;
    }
    if (!location) {
      return { visibleBlocks, error: block.primitive === "Transform" ? "Transform needs a selected input before it can run." : "Transfer needs an available source before it can run." };
    }
    if (block.primitive === "Transform") {
      if (location !== block.executionDevice) {
        visibleBlocks.push({
          primitive: "Transfer",
          source: location,
          destination: block.executionDevice,
          landingMode: "pipeline_handoff",
          derived: true,
          reason: `Required to process this file on ${block.executionDevice === "requesting_device" ? "this device" : "the selected device"}.`,
        });
        location = block.executionDevice;
      }
      visibleBlocks.push(block);
      continue;
    }
    // A final transfer changes where the flowing object is available. Pastey
    // Shared remains on the selected device and cannot feed a later Transform.
    if (block.destination === "pastey_shared") {
      visibleBlocks.push(block);
      location = null;
    } else {
      visibleBlocks.push(block);
      location = block.destination;
    }
  }
  return { visibleBlocks, error: null };
}

export function dependencyError(blocks: readonly ComposerBlock[]): string | null {
  return objectFlow(blocks).error;
}

export function canAddPrimitive(blocks: readonly ComposerBlock[], primitive: ComposerPrimitive): boolean {
  return dependencyError([...blocks, blockForPrimitive(primitive)]) === null;
}

export function addPrimitive(
  blocks: readonly ComposerBlock[],
  primitive: ComposerPrimitive,
  transformExecutionDevice: ComposerDevice = "requesting_device",
): { blocks: ComposerBlock[]; error: string | null } {
  const next = [...blocks, blockForPrimitive(primitive, transformExecutionDevice)];
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
  return { ...block, ...patch, filenameHint: (patch.filenameHint ?? block.filenameHint).slice(0, 128), extension: extension.replace(/[^a-z0-9]/g, "").slice(0, 16) };
}

export function manualBridgePlanInput(
  blocks: readonly ComposerBlock[],
  transformCapabilities: TransformExecutorCapabilities,
): { value?: ManualBridgePlanInput; error?: string } {
  const flow = objectFlow(blocks);
  if (flow.error) return { error: flow.error };
  const search = blocks.find((block): block is SearchBlock => block.primitive === "Search");
  if (!search) return { error: "Search is required for this composed plan." };
  const filenameHint = search.filenameHint.trim();
  if (!filenameHint) return { error: "Enter the filename to search for." };
  if (!search.safeScopes.length || search.safeScopes.some((scope) => !SAFE_SEARCH_SCOPES.some((entry) => entry.value === scope))) return { error: "Choose one or more reviewed Search locations." };
  const transform = blocks.find((block): block is TransformBlock => block.primitive === "Transform");
  const extensions = search.extension ? [search.extension] : [];
  if (transform && extensions.includes("pdf")) return { error: "Extract readable text does not accept PDF input." };
  if (transform) {
    const chosenCapability = transformCapabilities[transform.executionDevice];
    if (chosenCapability.status !== "available" || !chosenCapability.available) return { error: chosenCapability.reason };
  }
  const transfer = [...blocks].reverse().find((block): block is TransferBlock => block.primitive === "Transfer");
  return { value: {
    blocks: blocks.map((block) => ({ ...block, ...(block.primitive === "Search" ? { safeScopes: [...block.safeScopes] } : {}) })) as ComposerBlock[],
    visibleBlocks: flow.visibleBlocks,
    filenameHint,
    extensions,
    safeScopes: [...search.safeScopes],
    transformIntent: transform?.intent,
    transformExecutionDevice: transform?.executionDevice,
    transferDestination: transfer?.destination,
    originalUserGoal: manualGoal(flow.visibleBlocks, filenameHint),
  } };
}

export function manualGoal(blocks: readonly Pick<VisibleComposerBlock, "primitive">[], filenameHint: string): string {
  return `${blocks.map((block) => block.primitive).join(" → ")}: ${filenameHint}`;
}

function blockForPrimitive(primitive: ComposerPrimitive, transformExecutionDevice: ComposerDevice = "requesting_device"): ComposerBlock {
  if (primitive === "Search") return newSearchBlock();
  if (primitive === "Transform") return newTransformBlock(transformExecutionDevice);
  return newTransferBlock();
}
