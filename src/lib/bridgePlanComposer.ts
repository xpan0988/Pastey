/**
 * Renderer-only editing state for the bounded Bridge Plan primitives.
 * This is deliberately not a revision format: the Host still binds devices,
 * slots, grants, and immutable revision identity when a plan is submitted.
 */

export type SafeSearchScope = "downloads" | "desktop" | "documents" | "pastey_shared";
export type ComposerPrimitive = "Search" | "Transform" | "Transfer";
export type ComposerTransferDestination = "requesting_device" | "pastey_shared";

export const SAFE_SEARCH_SCOPES: ReadonlyArray<{ value: SafeSearchScope; label: string }> = [
  { value: "downloads", label: "Downloads" },
  { value: "desktop", label: "Desktop" },
  { value: "documents", label: "Documents" },
  { value: "pastey_shared", label: "Pastey Shared" },
];

export type SearchBlock = {
  primitive: "Search";
  filenameHint: string;
  extension: string;
  safeScopes: SafeSearchScope[];
};
export type TransformBlock = {
  primitive: "Transform";
  intent: "extract readable text";
};
export type TransferBlock = {
  primitive: "Transfer";
  destination: ComposerTransferDestination;
};
export type ComposerBlock = SearchBlock | TransformBlock | TransferBlock;

export type TransformAvailability = {
  intent: "extract readable text";
  status?: "unknown" | "available" | "unavailable";
  available: boolean;
  reason: string;
  hostLabel: string;
  acceptedInputMediaTypes?: string[];
  outputMediaType?: string | null;
};

export type ManualBridgePlanInput = {
  blocks: ComposerBlock[];
  filenameHint: string;
  extensions: string[];
  safeScopes: SafeSearchScope[];
  transformIntent?: "extract readable text";
  transferDestination?: ComposerTransferDestination;
  originalUserGoal: string;
};

const VALID_ORDERS = new Set([
  "Search",
  "Search>Transfer",
  "Search>Transform",
  "Search>Transform>Transfer",
]);

export function newSearchBlock(): SearchBlock {
  return { primitive: "Search", filenameHint: "", extension: "", safeScopes: ["downloads"] };
}

export function newTransformBlock(): TransformBlock {
  return { primitive: "Transform", intent: "extract readable text" };
}

export function newTransferBlock(): TransferBlock {
  return { primitive: "Transfer", destination: "requesting_device" };
}

export function primitives(blocks: readonly ComposerBlock[]): ComposerPrimitive[] {
  return blocks.map((block) => block.primitive);
}

export function dependencyError(blocks: readonly ComposerBlock[]): string | null {
  const order = primitives(blocks);
  if (VALID_ORDERS.has(order.join(">"))) return null;
  if (order.includes("Transform") && order.indexOf("Search") > order.indexOf("Transform")) {
    return "Transform needs a selected input before it can run.";
  }
  if (order.includes("Transfer") && order.indexOf("Search") > order.indexOf("Transfer")) {
    return "Transfer needs an available source before it can run.";
  }
  if (order.includes("Transform") && !order.includes("Search")) {
    return "Transform needs a selected input before it can run.";
  }
  if (order.includes("Transfer") && !order.includes("Search")) {
    return "Transfer needs an available source before it can run.";
  }
  return "Pastey supports Search, Search → Transfer, Search → Transform, and Search → Transform → Transfer.";
}

export function canAddPrimitive(blocks: readonly ComposerBlock[], primitive: ComposerPrimitive): boolean {
  return dependencyError([...blocks, blockForPrimitive(primitive)]) === null;
}

export function addPrimitive(blocks: readonly ComposerBlock[], primitive: ComposerPrimitive): { blocks: ComposerBlock[]; error: string | null } {
  const next = [...blocks, blockForPrimitive(primitive)];
  return { blocks: dependencyError(next) ? [...blocks] : next, error: dependencyError(next) };
}

export function removeBlock(blocks: readonly ComposerBlock[], index: number): { blocks: ComposerBlock[]; error: string | null } {
  const next = blocks.filter((_, current) => current !== index);
  return { blocks: dependencyError(next) ? [...blocks] : next, error: dependencyError(next) };
}

export function moveBlock(blocks: readonly ComposerBlock[], from: number, to: number): { blocks: ComposerBlock[]; error: string | null } {
  if (from === to || from < 0 || to < 0 || from >= blocks.length || to >= blocks.length) return { blocks: [...blocks], error: null };
  const next = [...blocks];
  const [block] = next.splice(from, 1);
  next.splice(to, 0, block!);
  return { blocks: dependencyError(next) ? [...blocks] : next, error: dependencyError(next) };
}

export function updateSearchBlock(block: SearchBlock, patch: Partial<Omit<SearchBlock, "primitive">>): SearchBlock {
  const extension = (patch.extension ?? block.extension).trim().replace(/^\./, "").toLowerCase();
  return { ...block, ...patch, filenameHint: (patch.filenameHint ?? block.filenameHint).slice(0, 128), extension: extension.replace(/[^a-z0-9]/g, "").slice(0, 16) };
}

export function manualBridgePlanInput(
  blocks: readonly ComposerBlock[],
  transformAvailability: TransformAvailability,
): { value?: ManualBridgePlanInput; error?: string } {
  const orderError = dependencyError(blocks);
  if (orderError) return { error: orderError };
  const search = blocks.find((block): block is SearchBlock => block.primitive === "Search");
  if (!search) return { error: "Search is required for this composed plan." };
  const filenameHint = search.filenameHint.trim();
  if (!filenameHint) return { error: "Enter the filename to search for." };
  if (!search.safeScopes.length || search.safeScopes.some((scope) => !SAFE_SEARCH_SCOPES.some((entry) => entry.value === scope))) {
    return { error: "Choose one or more reviewed Search locations." };
  }
  const transform = blocks.find((block): block is TransformBlock => block.primitive === "Transform");
  if (transform && !transformAvailability.available) return { error: transformAvailability.reason };
  const transfer = blocks.find((block): block is TransferBlock => block.primitive === "Transfer");
  const extensions = search.extension ? [search.extension] : [];
  return {
    value: {
      blocks: blocks.map((block) => ({ ...block, ...(block.primitive === "Search" ? { safeScopes: [...block.safeScopes] } : {}) })) as ComposerBlock[],
      filenameHint,
      extensions,
      safeScopes: [...search.safeScopes],
      transformIntent: transform?.intent,
      transferDestination: transfer?.destination,
      originalUserGoal: manualGoal(blocks, filenameHint),
    },
  };
}

export function manualGoal(blocks: readonly ComposerBlock[], filenameHint: string): string {
  return `${primitives(blocks).join(" → ")}: ${filenameHint}`;
}

function blockForPrimitive(primitive: ComposerPrimitive): ComposerBlock {
  if (primitive === "Search") return newSearchBlock();
  if (primitive === "Transform") return newTransformBlock();
  return newTransferBlock();
}
