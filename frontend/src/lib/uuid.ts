/**
 * crypto.randomUUID() is only exposed on **secure contexts** — HTTPS or
 * localhost. Production deployments served over plain HTTP (e.g. a Vultr IP
 * like http://149.28.x.x:4590) get `undefined` and throw "randomUUID is not
 * a function" when called.
 *
 * This helper:
 *   1. Uses the native crypto.randomUUID() when available (secure contexts).
 *   2. Falls back to crypto.getRandomValues() with manual v4 formatting if
 *      we have any crypto API but not randomUUID.
 *   3. Falls back to Math.random() as a last resort — not cryptographically
 *      strong, but fine for client-side chat IDs which are server-validated.
 */
export function safeUuid(): string {
  const g: { crypto?: Crypto } = globalThis as never;
  const crypto = g.crypto;

  if (crypto && typeof crypto.randomUUID === "function") {
    try {
      return crypto.randomUUID();
    } catch {
      // fall through
    }
  }

  if (crypto && typeof crypto.getRandomValues === "function") {
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 10
    const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0"));
    return (
      hex.slice(0, 4).join("") +
      "-" +
      hex.slice(4, 6).join("") +
      "-" +
      hex.slice(6, 8).join("") +
      "-" +
      hex.slice(8, 10).join("") +
      "-" +
      hex.slice(10, 16).join("")
    );
  }

  // Last-resort Math.random fallback. Not crypto-strong, but every chat
  // ID is verified server-side anyway so collisions/predictability here
  // don't grant access to anything.
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}
