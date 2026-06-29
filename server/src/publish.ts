// Open Cloud place publishing (Phase 4 support).
//
// Publishes a local place file (.rbxl or .rbxlx) as a new version of the configured
// experience, so the test harness lands in the place that run_tests reads. Same
// env-only credentials as cloud.ts, plus the universe-places write scope on the key.

import { readFileSync } from "node:fs";
import { extname } from "node:path";

const BASE = "https://apis.roblox.com/universes/v1";

export interface PublishResult {
  ok: boolean;
  versionNumber?: number;
  error?: string;
}

export interface PublishInput {
  filePath: string;
  versionType?: "Published" | "Saved";
}

export async function publishPlace(input: PublishInput): Promise<PublishResult> {
  const key = process.env.ROBLOX_OPEN_CLOUD_KEY;
  const universe = process.env.ROBLOX_UNIVERSE_ID;
  const place = process.env.ROBLOX_PLACE_ID;
  if (!key || !universe || !place) {
    return { ok: false, error: "Missing env: set ROBLOX_OPEN_CLOUD_KEY, ROBLOX_UNIVERSE_ID, ROBLOX_PLACE_ID." };
  }

  let body: Buffer;
  try {
    body = readFileSync(input.filePath);
  } catch (err) {
    return { ok: false, error: `cannot read ${input.filePath}: ${err instanceof Error ? err.message : String(err)}` };
  }

  // Binary .rbxl uses octet-stream; XML .rbxlx uses application/xml.
  const contentType = extname(input.filePath).toLowerCase() === ".rbxlx" ? "application/xml" : "application/octet-stream";
  const versionType = input.versionType ?? "Published";
  const url = `${BASE}/${universe}/places/${place}/versions?versionType=${versionType}`;
  try {
    const res = await fetch(url, {
      method: "POST",
      headers: { "x-api-key": key, "Content-Type": contentType },
      body: new Uint8Array(body),
    });
    const text = await res.text();
    if (!res.ok) return { ok: false, error: `publish failed: ${res.status} ${text}`.trim() };
    const data = JSON.parse(text) as { versionNumber?: number };
    return { ok: true, versionNumber: data.versionNumber };
  } catch (err) {
    return { ok: false, error: `publish failed: ${err instanceof Error ? err.message : String(err)}` };
  }
}
