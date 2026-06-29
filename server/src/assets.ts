// Open Cloud Assets upload client (Phase 2).
//
// Uploads a local file as a Roblox asset over HTTPS. Same shape as cloud.ts
// (env-only key, create then poll an operation, transient retry), but a SEPARATE
// base path and, crucially, a multipart body whose boundary fetch must set itself,
// so this does not reuse cloud.ts's JSON fetch wrapper.
//
// Env: ROBLOX_OPEN_CLOUD_KEY (assets read + write scope) and a creator id,
// ROBLOX_CREATOR_USER_ID or ROBLOX_CREATOR_GROUP_ID. The key is sent in x-api-key
// and never logged.

import { readFileSync, statSync } from "node:fs";
import { basename, extname } from "node:path";

const ASSETS_BASE = "https://apis.roblox.com/assets/v1";
const POLL_INTERVAL_MS = 2000;
const POLL_DEADLINE_MS = 120_000;
const TRANSIENT_ATTEMPTS = 3;
const MAX_FILE_BYTES = 20 * 1024 * 1024;

export type AssetType = "Decal" | "Audio" | "Model" | "Animation" | "Video";

// The fileContent part's content type must match the asset type, so infer it from
// the extension when the caller does not pass one.
const CONTENT_TYPES: Record<string, string> = {
  ".png": "image/png",
  ".jpeg": "image/jpeg",
  ".jpg": "image/jpeg",
  ".bmp": "image/bmp",
  ".tga": "image/tga",
  ".mp3": "audio/mpeg",
  ".ogg": "audio/ogg",
  ".wav": "audio/wav",
  ".flac": "audio/flac",
  ".fbx": "model/fbx",
  ".gltf": "model/gltf+json",
  ".glb": "model/gltf-binary",
  ".rbxm": "model/x-rbxm",
  ".rbxmx": "model/x-rbxmx",
  ".mp4": "video/mp4",
  ".mov": "video/mov",
};

export interface UploadResult {
  ok: boolean;
  assetId?: string | number;
  revisionId?: string | number;
  error?: string;
}

type Credentials =
  | { ok: true; key: string; creator: { userId: string } | { groupId: string } }
  | { ok: false; error: string };

function readCredentials(): Credentials {
  const key = process.env.ROBLOX_OPEN_CLOUD_KEY;
  if (!key) return { ok: false, error: "Missing env: set ROBLOX_OPEN_CLOUD_KEY." };
  const userId = process.env.ROBLOX_CREATOR_USER_ID;
  const groupId = process.env.ROBLOX_CREATOR_GROUP_ID;
  if (userId) return { ok: true, key, creator: { userId } };
  if (groupId) return { ok: true, key, creator: { groupId } };
  return { ok: false, error: "Missing env: set ROBLOX_CREATOR_USER_ID or ROBLOX_CREATOR_GROUP_ID." };
}

export interface UploadInput {
  filePath: string;
  assetType: AssetType;
  displayName: string;
  description?: string;
  contentType?: string;
}

export async function uploadAsset(input: UploadInput): Promise<UploadResult> {
  const creds = readCredentials();
  if (!creds.ok) return { ok: false, error: creds.error };

  let fileBuffer: Buffer;
  try {
    const size = statSync(input.filePath).size;
    if (size > MAX_FILE_BYTES) {
      return { ok: false, error: `file is ${size} bytes; the per-upload limit is ${MAX_FILE_BYTES}.` };
    }
    fileBuffer = readFileSync(input.filePath);
  } catch (err) {
    return { ok: false, error: `cannot read ${input.filePath}: ${messageOf(err)}` };
  }

  const contentType = input.contentType ?? CONTENT_TYPES[extname(input.filePath).toLowerCase()];
  if (contentType === undefined) {
    return { ok: false, error: `cannot infer a content type for ${input.filePath}; pass contentType explicitly.` };
  }

  const form = new FormData();
  form.set(
    "request",
    JSON.stringify({
      assetType: input.assetType,
      displayName: input.displayName,
      description: input.description ?? "",
      creationContext: { creator: creds.creator },
    }),
  );
  // Copy into a fresh Uint8Array so the Blob part is backed by a plain ArrayBuffer.
  form.set("fileContent", new Blob([new Uint8Array(fileBuffer)], { type: contentType }), basename(input.filePath));

  let operationPath: string;
  try {
    const created = (await (
      await assetsFetch(creds.key, `${ASSETS_BASE}/assets`, { method: "POST", body: form })
    ).json()) as { path?: string };
    if (!created.path) return { ok: false, error: "create asset returned no operation path" };
    operationPath = created.path;
  } catch (err) {
    return { ok: false, error: `create asset failed: ${messageOf(err)}` };
  }

  try {
    const done = await pollOperation(creds.key, operationPath);
    // A finished operation can carry an error (for example moderation) instead of a response.
    if (done.error !== undefined) return { ok: false, error: `asset rejected: ${JSON.stringify(done.error)}` };
    return { ok: true, assetId: done.response?.assetId, revisionId: done.response?.revisionId };
  } catch (err) {
    return { ok: false, error: `polling failed: ${messageOf(err)}` };
  }
}

interface AssetOperation {
  done?: boolean;
  error?: unknown;
  response?: { assetId?: string | number; revisionId?: string | number };
}

async function pollOperation(key: string, path: string): Promise<AssetOperation> {
  const url = `${ASSETS_BASE}/${path}`;
  const deadline = Date.now() + POLL_DEADLINE_MS;
  for (;;) {
    const op = (await (await assetsFetch(key, url, { method: "GET" })).json()) as AssetOperation;
    if (op.done) return op;
    if (Date.now() > deadline) {
      throw new Error(`asset operation did not finish within ${Math.round(POLL_DEADLINE_MS / 1000)}s`);
    }
    await sleep(POLL_INTERVAL_MS);
  }
}

interface AssetsFetchInit {
  method: string;
  body?: FormData;
}

// Only adds x-api-key. It must NOT set content-type: multipart needs fetch to set
// its own boundary, and the GET poll needs no content type.
async function assetsFetch(key: string, url: string, init: AssetsFetchInit): Promise<Response> {
  let lastError = "";
  for (let attempt = 1; attempt <= TRANSIENT_ATTEMPTS; attempt++) {
    let res: Response | undefined;
    try {
      res = await fetch(url, { method: init.method, headers: { "x-api-key": key }, body: init.body });
    } catch (err) {
      lastError = messageOf(err);
    }
    if (res) {
      if (res.ok) return res;
      const detail = `${res.status} ${await safeText(res)}`.trim();
      if (!isTransient(res.status)) throw new Error(`${init.method} ${url} responded ${detail}`);
      lastError = detail;
    }
    if (attempt < TRANSIENT_ATTEMPTS) await sleep(attempt * POLL_INTERVAL_MS);
  }
  throw new Error(`${init.method} ${url} failed after ${TRANSIENT_ATTEMPTS} attempts: ${lastError}`);
}

function isTransient(status: number): boolean {
  return status === 429 || status >= 500;
}

function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

async function safeText(res: Response): Promise<string> {
  try {
    return await res.text();
  } catch {
    return "";
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
