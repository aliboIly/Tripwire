--!strict
-- Headless test for the base64 encoder. base64.luau imports nothing and works
-- buffer-in/string-out, so Lune runs the actual compiled module directly. Checks the
-- canonical RFC 4648 vectors, including both padding cases. Run with:
-- lune run test/base64.test.luau (after `npm run build`). Exits non-zero on failure.

local base64 = require("../out/base64")

local failures = 0
local function check(input: string, expected: string)
	local got = base64.encode(buffer.fromstring(input))
	local ok = got == expected
	print((ok and "PASS" or "FAIL") .. `: encode("{input}") = "{got}" (want "{expected}")`)
	if not ok then
		failures += 1
	end
end

-- RFC 4648 test vectors plus the two padding lengths.
check("", "")
check("f", "Zg==")
check("fo", "Zm8=")
check("foo", "Zm9v")
check("foob", "Zm9vYg==")
check("fooba", "Zm9vYmE=")
check("foobar", "Zm9vYmFy")
check("Man", "TWFu")

-- Non-ASCII bytes round-trip through the byte math.
local raw = buffer.create(3)
buffer.writeu8(raw, 0, 255)
buffer.writeu8(raw, 1, 0)
buffer.writeu8(raw, 2, 128)
local got = base64.encode(raw)
local ok = got == "/wCA"
print((ok and "PASS" or "FAIL") .. `: encode(<0xFF,0x00,0x80>) = "{got}" (want "/wCA")`)
if not ok then
	failures += 1
end

if failures > 0 then
	error(`base64.test: {failures} check(s) failed`)
end
print("base64.test: all checks passed")
