// Headless test harness (Phase 4).
//
// Tests live in the target place (compiled by rbxtsc, synced by Rojo, published).
// This module sends a tiny Luau entry shim through the Open Cloud client, which
// requires the in-place runner and returns its summary. The summary rides the
// task's return value (cloud.ts result.results[0]); logs are diagnostic only.
//
// Running live needs the place published and the Open Cloud env set, so the
// caller (a human or CI) publishes first. The agent cannot publish with the key.

import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { LuauResult, runLuau } from "./cloud.js";

const DEFAULT_TEST_DIR = "sample-game/src/shared";
const TEST_NAME = /^[\w.\-]+$/;

export interface TestFailure {
  name: string;
  message: string;
  file?: string;
}

// The frozen result contract. A different runner (for example Jest-Lua later) can
// map onto this without changing the parser below.
export interface TestSummary {
  passed: number;
  failed: number;
  failures: TestFailure[];
}

export interface TestCaseList {
  file: string;
  cases: string[];
}

export type HarnessResult =
  | { kind: "harness_error"; error: string; logs: string[] }
  | { kind: "tests"; summary: TestSummary; logs: string[] };

// The one place the harness hand-writes Luau, quarantined like runner/runner.luau.
// Open Cloud accepts only a source string; the real logic is the compiled runner.
const RUNNER = 'require(game:GetService("ReplicatedStorage").TS.harness.runTests)';
const SPEC_NAME = /^[\w.\-]+$/;

export async function runTests(): Promise<HarnessResult> {
  return parseSummary(await runLuau(`return ${RUNNER}.runTests(nil)`));
}

export async function runTestFile(specName: string): Promise<HarnessResult> {
  if (!SPEC_NAME.test(specName)) {
    return { kind: "harness_error", error: `invalid spec name: ${specName}`, logs: [] };
  }
  return parseSummary(await runLuau(`return ${RUNNER}.runTests("${specName}")`));
}

export async function listTests(): Promise<{ ok: boolean; specs: TestCaseList[]; error?: string }> {
  const result = await runLuau(`return ${RUNNER}.listTests()`);
  if (!result.ok) return { ok: false, specs: [], error: result.error ?? "Open Cloud run failed" };
  const raw = result.results?.[0];
  if (!Array.isArray(raw)) return { ok: false, specs: [], error: "the runner returned no test list" };
  const specs: TestCaseList[] = [];
  for (const entry of raw) {
    if (typeof entry !== "object" || entry === null) continue;
    const e = entry as Record<string, unknown>;
    const cases = Array.isArray(e.cases) ? e.cases.filter((c): c is string => typeof c === "string") : [];
    specs.push({ file: typeof e.file === "string" ? e.file : "(unknown)", cases });
  }
  return { ok: true, specs };
}

// Three outcomes that must never be conflated: the run itself failed (no creds,
// unpublished place, timeout, runner crash), the tests ran and some failed, or
// the tests all passed.
function parseSummary(result: LuauResult): HarnessResult {
  if (!result.ok) {
    return { kind: "harness_error", error: result.error ?? "Open Cloud run failed", logs: result.logs };
  }
  const summary = toSummary(result.results?.[0]);
  if (summary === undefined) {
    return {
      kind: "harness_error",
      error: "the test runner returned no summary (it may have crashed before returning)",
      logs: result.logs,
    };
  }
  return { kind: "tests", summary, logs: result.logs };
}

function toSummary(raw: unknown): TestSummary | undefined {
  if (typeof raw !== "object" || raw === null) return undefined;
  const obj = raw as Record<string, unknown>;
  if (typeof obj.passed !== "number" || typeof obj.failed !== "number") return undefined;
  const failures: TestFailure[] = [];
  if (Array.isArray(obj.failures)) {
    for (const f of obj.failures) {
      if (typeof f !== "object" || f === null) continue;
      const ff = f as Record<string, unknown>;
      failures.push({
        name: typeof ff.name === "string" ? ff.name : "(unnamed)",
        message: typeof ff.message === "string" ? ff.message : "",
        file: typeof ff.file === "string" ? ff.file : undefined,
      });
    }
  }
  return { passed: obj.passed, failed: obj.failed, failures };
}

export function formatHarness(result: HarnessResult): string {
  if (result.kind === "harness_error") {
    const tail = result.logs.length > 0 ? `\nlogs:\n${result.logs.join("\n")}` : "";
    return `Harness error: ${result.error}${tail}`;
  }
  const s = result.summary;
  const lines = [`Tests: ${s.passed} passed, ${s.failed} failed.`];
  for (const f of s.failures) lines.push(`  FAIL ${f.name}: ${f.message}`);
  return lines.join("\n");
}

// Writes a spec source file to disk (tests are roblox-ts source, git-native and
// compiled into the place). It does not run anything; the place must be rebuilt
// and published before run_tests picks the spec up.
export function writeTest(input: { name: string; source: string; dir?: string }): {
  ok: boolean;
  path?: string;
  error?: string;
} {
  if (!TEST_NAME.test(input.name)) return { ok: false, error: `invalid test name: ${input.name}` };
  const dir = input.dir ?? DEFAULT_TEST_DIR;
  const fileName = input.name.endsWith(".spec.ts") ? input.name : `${input.name}.spec.ts`;
  const path = join(dir, fileName);
  try {
    mkdirSync(dir, { recursive: true });
    writeFileSync(path, input.source.endsWith("\n") ? input.source : `${input.source}\n`);
    return { ok: true, path };
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}

export function formatTestList(list: { ok: boolean; specs: TestCaseList[]; error?: string }): string {
  if (!list.ok) return `Error: ${list.error}`;
  if (list.specs.length === 0) return "No specs found. Build and publish the place first.";
  const lines: string[] = [];
  for (const spec of list.specs) {
    lines.push(spec.file);
    for (const c of spec.cases) lines.push(`  - ${c}`);
  }
  return lines.join("\n");
}
