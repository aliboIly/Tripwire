// Standard base64 encoder for a Luau buffer. Roblox Luau has no built-in base64, and
// the screenshot path needs to ship raw pixels over the JSON bridge. This stays free of
// imports and array helpers (it works buffer-in, buffer-out) so it compiles to a
// self-contained module that a headless runtime can unit-test on its own.

const PAD = 61; // "="

// Map a 6-bit value to the ASCII code of its base64 character.
function sextetToByte(value: number): number {
	if (value < 26) return 65 + value; // A-Z
	if (value < 52) return 97 + (value - 26); // a-z
	if (value < 62) return 48 + (value - 52); // 0-9
	return value === 62 ? 43 : 47; // "+" or "/"
}

export function encode(data: buffer): string {
	const length = buffer.len(data);
	const out = buffer.create(math.ceil(length / 3) * 4);
	let outPos = 0;
	for (let i = 0; i < length; i += 3) {
		const b0 = buffer.readu8(data, i);
		const b1 = i + 1 < length ? buffer.readu8(data, i + 1) : 0;
		const b2 = i + 2 < length ? buffer.readu8(data, i + 2) : 0;
		const triple = (b0 << 16) | (b1 << 8) | b2;
		const remaining = length - i;
		buffer.writeu8(out, outPos, sextetToByte((triple >> 18) & 63));
		buffer.writeu8(out, outPos + 1, sextetToByte((triple >> 12) & 63));
		buffer.writeu8(out, outPos + 2, remaining >= 2 ? sextetToByte((triple >> 6) & 63) : PAD);
		buffer.writeu8(out, outPos + 3, remaining >= 3 ? sextetToByte(triple & 63) : PAD);
		outPos += 4;
	}
	return buffer.tostring(out);
}
