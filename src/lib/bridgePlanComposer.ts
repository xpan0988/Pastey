/** Renderer editing state. Rust remains the immutable revision authority. */
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

export type SearchBlock = { primitive: "Search"; executionDevice: "selected_device"; filenameHint: string; extension: string; safeScopes: SafeSearchScope[] };
export type TransformBlock = { primitive: "Transform"; intent: "extract readable text"; executionDevice: ComposerDevice };
export type TransferBlock = { primitive: "Transfer"; source: ComposerDevice; destination: ComposerTransferDestination; landingMode: TransferLandingMode };
export type ComposerBlock = SearchBlock | TransformBlock | TransferBlock;

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
export type ManualBridgePlanInput = { blocks: ComposerBlock[]; originalUserGoal: string };
export type ObjectFlowResult = { location: ComposerDevice | null; error: string | null };

export function newSearchBlock(): SearchBlock {
  return { primitive: "Search", executionDevice: "selected_device", filenameHint: "", extension: "", safeScopes: ["downloads"] };
}

/** A standalone default is selected-device; addPrimitive uses actual locality. */
export function newTransformBlock(executionDevice: ComposerDevice = "selected_device"): TransformBlock {
  return { primitive: "Transform", intent: "extract readable text", executionDevice };
}

export function newTransferBlock(source: ComposerDevice = "selected_device"): TransferBlock {
  return { primitive: "Transfer", source, destination: source === "selected_device" ? "requesting_device" : "selected_device", landingMode: "final_delivery" };
}

export function requiredTransferForTransform(blocks: readonly ComposerBlock[], index: number): TransferBlock | null {
  const transform = blocks[index];
  if (transform?.primitive !== "Transform") return null;
  const source = currentObjectLocation(blocks.slice(0, index));
  if (!source || source === transform.executionDevice) return null;
  return { primitive: "Transfer", source, destination: transform.executionDevice, landingMode: "pipeline_handoff" };
}

/** Explicit convenience edit: nothing is inserted until the user invokes it. */
export function insertRequiredTransfer(blocks: readonly ComposerBlock[], transformIndex: number): { blocks: ComposerBlock[]; error: string | null } {
  const transfer = requiredTransferForTransform(blocks, transformIndex);
  if (!transfer) return { blocks: [...blocks], error: null };
  const next = [...blocks];
  next.splice(transformIndex, 0, transfer);
  return { blocks: next, error: dependencyError(next) };
}

export function primitives(blocks: readonly ComposerBlock[]): ComposerPrimitive[] {
  return blocks.map((block) => block.primitive);
}

/** Resolves authored object locality without inserting or rewriting steps. */
export function objectFlow(blocks: readonly ComposerBlock[]): ObjectFlowResult {
  let location: ComposerDevice | null = null;
  for (const block of blocks) {
    if (block.primitive === "Search") {
      if (location) return { location, error: "Search starts a new object flow; remove the earlier flow before adding another Search." };
      location = block.executionDevice;
      continue;
    }
    if (!location) {
      return { location, error: block.primitive === "Transform" ? "Transform needs a selected input before it can run." : "Transfer needs an available source before it can run." };
    }
    if (block.primitive === "Transform") {
      if (location !== block.executionDevice) {
        return { location, error: `This file is currently on ${deviceDescription(location)}. Add a Transfer to ${deviceDescription(block.executionDevice)} before processing it there.` };
      }
      continue;
    }
    if (block.source !== location) {
      return { location, error: `Transfer source must be ${deviceDescription(location)}, where the current object is located.` };
    }
    if (block.destination === "pastey_shared" && block.source !== "selected_device") {
      return { location, error: "Pastey Shared final delivery is available only while the object is on the selected device." };
    }
    const destination = block.destination === "pastey_shared" ? "selected_device" : block.destination;
    if (destination === block.source && block.landingMode === "pipeline_handoff") return { location, error: "A private pipeline Transfer must move the object to another device." };
    location = block.landingMode === "pipeline_handoff" ? destination : null;
  }
  const lastBlock = blocks.length > 0 ? blocks[blocks.length - 1] : undefined;
  if (lastBlock?.primitive === "Transfer" && lastBlock.landingMode === "pipeline_handoff") {
    return { location, error: "A private pipeline Transfer needs a following step that consumes its object." };
  }
  return { location, error: null };
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
  return { ...block, ...patch, filenameHint: (patch.filenameHint ?? block.filenameHint).slice(0, 128), extension: extension.replace(/[^a-z0-9]/g, "").slice(0, 16) };
}

export function manualBridgePlanInput(blocks: readonly ComposerBlock[], transformCapabilities: TransformExecutorCapabilities): { value?: ManualBridgePlanInput; error?: string } {
  const flow = objectFlow(blocks);
  if (flow.error) return { error: flow.error };
  const search = blocks.find((block): block is SearchBlock => block.primitive === "Search");
  if (!search) return { error: "Search is required for this composed plan." };
  const filenameHint = search.filenameHint.trim();
  if (!filenameHint) return { error: "Enter the filename to search for." };
  if (!search.safeScopes.length || search.safeScopes.some((scope) => !SAFE_SEARCH_SCOPES.some((entry) => entry.value === scope))) return { error: "Choose one or more reviewed Search locations." };
  const transforms = blocks.filter((block): block is TransformBlock => block.primitive === "Transform");
  if (transforms.length > 1) return { error: "The current readable-text capability supports one Transform per plan." };
  if (transforms.length && search.extension === "pdf") return { error: "Extract readable text does not accept PDF input." };
  for (const transform of transforms) {
    const capability = transformCapabilities[transform.executionDevice];
    if (capability.status !== "available" || !capability.available) return { error: capability.reason };
  }
  return { value: {
    blocks: blocks.map((block) => ({ ...block, ...(block.primitive === "Search" ? { filenameHint, safeScopes: [...block.safeScopes] } : {}) })) as ComposerBlock[],
    originalUserGoal: manualGoal(blocks, filenameHint),
  } };
}

export function manualGoal(blocks: readonly Pick<ComposerBlock, "primitive">[], filenameHint: string): string {
  return `${blocks.map((block) => block.primitive).join(" → ")}: ${filenameHint}`;
}

export function reviewedObjectFlow(blocks: readonly ComposerBlock[]): string[] {
  return blocks.map((block) => {
    if (block.primitive === "Search") return `Search @ ${block.executionDevice}`;
    if (block.primitive === "Transform") return `Transform @ ${block.executionDevice}: ${block.intent}`;
    return `Transfer ${block.source} → ${block.destination} (${block.landingMode})`;
  });
}

function blockForPrimitive(blocks: readonly ComposerBlock[], primitive: ComposerPrimitive): ComposerBlock {
  if (primitive === "Search") return newSearchBlock();
  const location = currentObjectLocation(blocks) ?? "selected_device";
  return primitive === "Transform" ? newTransformBlock(location) : newTransferBlock(location);
}

function deviceDescription(device: ComposerDevice): string {
  return device === "requesting_device" ? "the requesting device" : "the selected device";
}
