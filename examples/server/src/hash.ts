import { createHash } from "crypto";

/** BLAKE3 isn't in Node/Bun stdlib yet, so we use SHA-256 as a stand-in for this example. */
export function hash(data: Uint8Array | string): string {
  const h = createHash("sha256");
  h.update(typeof data === "string" ? data : Buffer.from(data));
  return h.digest("hex");
}

/** Hash a JSON-serializable value deterministically. */
export function hashJson(value: unknown): string {
  return hash(JSON.stringify(value));
}

/** Generate a short workspace id: ws-XXXX */
export function generateWorkspaceId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(2));
  const hex = Buffer.from(bytes).toString("hex");
  return `ws-${hex}`;
}
