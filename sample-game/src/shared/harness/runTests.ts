// In-place test runner. It discovers spec ModuleScripts (names ending in
// ".spec"), runs their cases, and returns a JSON-safe summary that the engine
// serializes into the Open Cloud task output. Everything is pcall'd so one bad
// spec becomes one failure entry rather than a task crash, and the summary is
// always returned (a propagated error would discard the output entirely).

import { ReplicatedStorage } from "@rbxts/services";
import { Suite } from "shared/harness/testkit";

export interface TestFailure {
	name: string;
	message: string;
	file: string;
}

export interface TestSummary {
	passed: number;
	failed: number;
	failures: TestFailure[];
}

type Registrar = (suite: Suite) => void;

function isSpec(instance: Instance): instance is ModuleScript {
	if (!instance.IsA("ModuleScript")) return false;
	const [matched] = instance.Name.find("%.spec$");
	return matched !== undefined;
}

function specRoot(): Instance | undefined {
	return ReplicatedStorage.FindFirstChild("TS");
}

function collectCases(register: Registrar): Array<{ name: string; fn: () => void }> {
	const cases: Array<{ name: string; fn: () => void }> = [];
	register({ case: (name, fn) => cases.push({ name, fn }) });
	return cases;
}

export function runTests(filter?: string): TestSummary {
	const summary: TestSummary = { passed: 0, failed: 0, failures: [] };
	const root = specRoot();
	if (root === undefined) return summary;

	for (const instance of root.GetDescendants()) {
		if (!isSpec(instance)) continue;
		if (filter !== undefined && instance.Name !== filter) continue;
		const file = instance.Name;

		const [okRequire, moduleOrErr] = pcall(require, instance);
		if (!okRequire) {
			summary.failed += 1;
			summary.failures.push({ name: file, message: tostring(moduleOrErr), file });
			continue;
		}

		const [okRegister, casesOrErr] = pcall(collectCases, moduleOrErr as Registrar);
		if (!okRegister) {
			summary.failed += 1;
			summary.failures.push({ name: file, message: `failed to register cases: ${casesOrErr}`, file });
			continue;
		}

		for (const testCase of casesOrErr as Array<{ name: string; fn: () => void }>) {
			const [okRun, err] = pcall(testCase.fn);
			if (okRun) {
				summary.passed += 1;
			} else {
				summary.failed += 1;
				summary.failures.push({ name: `${file} > ${testCase.name}`, message: tostring(err), file });
			}
		}
	}

	return summary;
}

// Discovery without running anything, for list_tests.
export function listTests(): Array<{ file: string; cases: string[] }> {
	const out: Array<{ file: string; cases: string[] }> = [];
	const root = specRoot();
	if (root === undefined) return out;

	for (const instance of root.GetDescendants()) {
		if (!isSpec(instance)) continue;
		const file = instance.Name;
		const [okRequire, moduleOrErr] = pcall(require, instance);
		if (!okRequire) {
			out.push({ file, cases: [] });
			continue;
		}
		const names: string[] = [];
		pcall(() => {
			(moduleOrErr as Registrar)({ case: (name) => names.push(name) });
		});
		out.push({ file, cases: names });
	}

	return out;
}
