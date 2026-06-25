import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  bridgeDurableIdentityId,
  describeBridgeDurableIdentityBoundary,
  normalizeBridgeDurablePeerIdentity,
} from "../src/lib/bridgeIdentity";

function durableIdentity(overrides: Record<string, unknown> = {}) {
  return {
    identityId: "durable-peer:one",
    displayName: "Peer One",
    pairingState: "paired",
    pairingMethod: "manual_identity_code",
    createdAt: "2026-06-24T00:00:00.000Z",
    updatedAt: "2026-06-24T00:00:00.000Z",
    lastSeenAt: "2026-06-24T00:00:00.000Z",
    rotationState: "current",
    publicKeyFingerprint: "sha256:abcd",
    durableIdentityOnly: true,
    grantsConsent: false,
    grantsExecutionAuthority: false,
    grantsReusableTrust: false,
    autoJoinBridges: false,
    currentSessionMember: false,
    ...overrides,
  };
}

test("durable identity foundation normalizes pairing metadata only", () => {
  const result = normalizeBridgeDurablePeerIdentity(durableIdentity());

  assert.equal(result.ok, true, result.ok ? "" : result.errors.join(" "));
  if (!result.ok) return;
  assert.equal(result.identity.identityId, bridgeDurableIdentityId("durable-peer:one"));
  assert.equal(result.identity.durableIdentityOnly, true);
  assert.equal(result.identity.rotationState, "current");
  assert.equal(result.identity.lastSeenAt, "2026-06-24T00:00:00.000Z");
  assert.equal(result.identity.grantsConsent, false);
  assert.equal(result.identity.grantsExecutionAuthority, false);
  assert.equal(result.identity.grantsReusableTrust, false);
  assert.equal(result.identity.autoJoinBridges, false);
  assert.equal(result.identity.currentSessionMember, false);
});

test("durable identity represents revocation and rotation without authority", () => {
  const revoked = normalizeBridgeDurablePeerIdentity(durableIdentity({
    pairingState: "revoked",
    revokedAt: "2026-06-24T01:00:00.000Z",
    rotationState: "rotation_required",
  }));

  assert.equal(revoked.ok, true, revoked.ok ? "" : revoked.errors.join(" "));
  if (!revoked.ok) return;
  assert.equal(revoked.identity.pairingState, "revoked");
  assert.equal(revoked.identity.revokedAt, "2026-06-24T01:00:00.000Z");
  assert.equal(revoked.identity.rotationState, "rotation_required");
  assert.equal(revoked.identity.grantsConsent, false);
  assert.equal(revoked.identity.grantsExecutionAuthority, false);
});

test("durable identity rejects revoked state without revoked timestamp and unsupported rotation", () => {
  assert.equal(normalizeBridgeDurablePeerIdentity(durableIdentity({
    pairingState: "revoked",
    revokedAt: undefined,
  })).ok, false);
  assert.equal(normalizeBridgeDurablePeerIdentity(durableIdentity({
    rotationState: "silent_authority_preserved",
  })).ok, false);
});

test("durable identity rejects consent trust authority history and auto-join", () => {
  const cases = [
    durableIdentity({ grantsConsent: true }),
    durableIdentity({ grantsExecutionAuthority: true }),
    durableIdentity({ grantsReusableTrust: true }),
    durableIdentity({ autoJoinBridges: true }),
    durableIdentity({ currentSessionMember: true }),
    durableIdentity({ consentId: "consent" }),
    durableIdentity({ trustedDeviceId: "trusted" }),
    durableIdentity({ historyId: "history" }),
  ];

  for (const candidate of cases) {
    const result = normalizeBridgeDurablePeerIdentity(candidate);
    assert.equal(result.ok, false);
  }
});

test("durable identity does not create Bridge membership or execution authority wording", () => {
  const result = normalizeBridgeDurablePeerIdentity(durableIdentity());
  assert.equal(result.ok, true, result.ok ? "" : result.errors.join(" "));
  if (!result.ok) return;

  const description = describeBridgeDurableIdentityBoundary(result.identity);
  assert.match(description, /pairing metadata only/);
  assert.match(description, /not Bridge membership/);
  assert.match(description, /not .*consent/);
  assert.match(description, /execution authority/);
  assert.equal(JSON.stringify(result.identity).includes("\"trust\":true"), false);
});

test("Room pairing UI uses display-only labels and avoids execution or trust wording", () => {
  const source = readFileSync("src/pages/RoomPage.tsx", "utf8");
  assert.match(source, /paired/);
  assert.match(source, /rotation required/);
  assert.match(source, /fingerprint/);
  assert.doesNotMatch(source, /trusted device|auto-approved|safe executor|can execute/i);
});
