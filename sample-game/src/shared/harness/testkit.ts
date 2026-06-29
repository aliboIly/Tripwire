// Minimal assertion kit for headless tests. Asserters error() on failure; that
// message becomes the failure's message in the summary. Kept deliberately small
// and free of Roblox APIs so it runs in the Open Cloud RCC context.

export interface Suite {
	case: (name: string, fn: () => void) => void;
}

export function equal<T>(actual: T, expected: T): void {
	if (actual !== expected) error(`expected ${tostring(expected)}, got ${tostring(actual)}`, 2);
}

export function isTrue(value: boolean): void {
	if (!value) error("expected true, got false", 2);
}

export function isFalse(value: boolean): void {
	if (value) error("expected false, got true", 2);
}

export function throws(fn: () => void): void {
	const [ok] = pcall(fn);
	if (ok) error("expected the function to throw", 2);
}
