export type BridgeDurableIdentityId = string & { readonly __bridgeDurableIdentityId: unique symbol };
export type BridgeDurableIdentityDisplayName = string & { readonly __bridgeDurableIdentityDisplayName: unique symbol };

export type BridgeDurableIdentityPairingState = "paired" | "revoked";
export type BridgeDurableIdentityPairingMethod = "manual_identity_code" | "verified_public_key";
export type BridgeDurableIdentityRotationState =
  | "current"
  | "rotation_required"
  | "rotation_deferred"
  | "rotation_unsupported";

export interface BridgeDurablePeerIdentity {
  readonly identityId: BridgeDurableIdentityId;
  readonly displayName: BridgeDurableIdentityDisplayName;
  readonly pairingState: BridgeDurableIdentityPairingState;
  readonly pairingMethod: BridgeDurableIdentityPairingMethod;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly lastSeenAt?: string;
  readonly revokedAt?: string;
  readonly rotationState: BridgeDurableIdentityRotationState;
  readonly publicKeyFingerprint?: string;
  readonly durableIdentityOnly: true;
  readonly grantsConsent: false;
  readonly grantsExecutionAuthority: false;
  readonly grantsReusableTrust: false;
  readonly autoJoinBridges: false;
  readonly currentSessionMember: false;
}

export type BridgeDurableIdentityNormalizationResult =
  | { ok: true; identity: BridgeDurablePeerIdentity; errors: [] }
  | { ok: false; errors: string[] };

const REQUIRED_FIELDS = [
  "identityId",
  "displayName",
  "pairingState",
  "pairingMethod",
  "createdAt",
  "updatedAt",
  "rotationState",
  "durableIdentityOnly",
  "grantsConsent",
  "grantsExecutionAuthority",
  "grantsReusableTrust",
  "autoJoinBridges",
  "currentSessionMember",
];
const OPTIONAL_FIELDS = ["lastSeenAt", "publicKeyFingerprint", "revokedAt"];
const UNSUPPORTED_AUTHORITY_FIELDS = [
  "authority",
  "authorized",
  "automaticApproval",
  "consent",
  "consentId",
  "executionAuthority",
  "history",
  "historyId",
  "reusableTrust",
  "trust",
  "trustedDevice",
  "trustedDeviceId",
];

export function bridgeDurableIdentityId(value: string): BridgeDurableIdentityId {
  const normalized = normalizeIdentifier(value);
  if (normalized === null) {
    throw new Error("Bridge durable identity id must be a non-empty string.");
  }
  return normalized as BridgeDurableIdentityId;
}

export function normalizeBridgeDurablePeerIdentity(value: unknown): BridgeDurableIdentityNormalizationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { ok: false, errors: ["Bridge durable identity must be an object."] };
  }

  rejectUnsupportedAuthorityFields(value, errors);
  requireExactFields(value, REQUIRED_FIELDS, OPTIONAL_FIELDS, errors);

  const identityId = normalizeIdentifier(value.identityId);
  const displayName = normalizeIdentifier(value.displayName);
  const createdAt = normalizeIdentifier(value.createdAt);
  const updatedAt = normalizeIdentifier(value.updatedAt);
  const lastSeenAt = normalizeIdentifier(value.lastSeenAt);
  const revokedAt = normalizeIdentifier(value.revokedAt);
  const publicKeyFingerprint = normalizeIdentifier(value.publicKeyFingerprint);
  if (identityId === null) errors.push("Bridge durable identity requires a non-empty identityId.");
  if (displayName === null) errors.push("Bridge durable identity requires a non-empty displayName.");
  if (createdAt === null) errors.push("Bridge durable identity requires a non-empty createdAt.");
  if (updatedAt === null) errors.push("Bridge durable identity requires a non-empty updatedAt.");
  if (value.pairingState !== "paired" && value.pairingState !== "revoked") {
    errors.push("Bridge durable identity pairingState must be paired or revoked.");
  }
  if (value.pairingMethod !== "manual_identity_code" && value.pairingMethod !== "verified_public_key") {
    errors.push("Bridge durable identity pairingMethod is unsupported.");
  }
  if (!isBridgeDurableIdentityRotationState(value.rotationState)) {
    errors.push("Bridge durable identity rotationState is unsupported.");
  }
  if (value.pairingState === "revoked" && revokedAt === null) {
    errors.push("Bridge durable identity revoked state requires revokedAt.");
  }
  if (value.durableIdentityOnly !== true) errors.push("Bridge durable identity requires durableIdentityOnly true.");
  if (value.grantsConsent !== false) errors.push("Bridge durable identity must not grant consent.");
  if (value.grantsExecutionAuthority !== false) {
    errors.push("Bridge durable identity must not grant execution authority.");
  }
  if (value.grantsReusableTrust !== false) errors.push("Bridge durable identity must not grant reusable trust.");
  if (value.autoJoinBridges !== false) errors.push("Bridge durable identity must not auto-join Bridges.");
  if (value.currentSessionMember !== false) {
    errors.push("Bridge durable identity foundation must stay separate from current-session membership.");
  }

  return errors.length === 0 &&
    identityId !== null &&
    displayName !== null &&
    createdAt !== null &&
    updatedAt !== null &&
    (value.pairingState === "paired" || value.pairingState === "revoked") &&
    (value.pairingMethod === "manual_identity_code" || value.pairingMethod === "verified_public_key") &&
    isBridgeDurableIdentityRotationState(value.rotationState)
    ? {
        ok: true,
        identity: {
          identityId: identityId as BridgeDurableIdentityId,
          displayName: displayName as BridgeDurableIdentityDisplayName,
          pairingState: value.pairingState,
          pairingMethod: value.pairingMethod,
          createdAt,
          updatedAt,
          rotationState: value.rotationState,
          ...(lastSeenAt === null ? {} : { lastSeenAt }),
          ...(revokedAt === null ? {} : { revokedAt }),
          ...(publicKeyFingerprint === null ? {} : { publicKeyFingerprint }),
          durableIdentityOnly: true,
          grantsConsent: false,
          grantsExecutionAuthority: false,
          grantsReusableTrust: false,
          autoJoinBridges: false,
          currentSessionMember: false,
        },
        errors: [],
      }
    : { ok: false, errors: unique(errors) };
}

export function describeBridgeDurableIdentityBoundary(identity: BridgeDurablePeerIdentity): string {
  return [
    `Durable identity ${identity.identityId} is pairing metadata only.`,
    "It is not Bridge membership, consent, reusable trust, execution authority, or auto-join state.",
  ].join(" ");
}

function normalizeIdentifier(value: unknown): string | null {
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isBridgeDurableIdentityRotationState(value: unknown): value is BridgeDurableIdentityRotationState {
  return (
    value === "current" ||
    value === "rotation_required" ||
    value === "rotation_deferred" ||
    value === "rotation_unsupported"
  );
}

function requireExactFields(
  value: Record<string, unknown>,
  requiredFields: readonly string[],
  optionalFields: readonly string[],
  errors: string[],
): void {
  for (const field of requiredFields) {
    if (!(field in value)) {
      errors.push(`Bridge durable identity is missing ${field}.`);
    }
  }
  for (const field of Object.keys(value)) {
    if (!requiredFields.includes(field) && !optionalFields.includes(field)) {
      errors.push(`Bridge durable identity contains unsupported field ${field}.`);
    }
  }
}

function rejectUnsupportedAuthorityFields(value: Record<string, unknown>, errors: string[]): void {
  for (const field of UNSUPPORTED_AUTHORITY_FIELDS) {
    if (field in value) {
      errors.push(`Bridge durable identity must not include ${field}; identity is not consent, trust, or authority.`);
    }
  }
}

function unique(values: readonly string[]): string[] {
  return Array.from(new Set(values));
}
