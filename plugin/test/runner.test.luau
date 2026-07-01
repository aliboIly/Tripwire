--!strict
-- Headless test for the runner's live-eval logic (run_luau_live). The runner is an
-- injected template, so we fill its tokens, set the test flag so the poll loop does not
-- start, and load it under Lune with a synthetic DataModel as `game`. Lune has a real
-- loadstring and DataModel, so this exercises the ACTUAL runner code (evalCode,
-- serialize), not a mock. Run via: lune run test/runner.test.luau (after `npm run build`,
-- though this test does not depend on the compiled plugin). Exits non-zero on failure.

local fs = require("@lune/fs")
local roblox = require("@lune/roblox")

local source = fs.readFile("../runner/runner.luau")
source = source:gsub("{{PORT}}", "44331")
source = source:gsub("{{PROTOCOL_VERSION}}", "2")
source = source:gsub("{{INSTANCE_ID}}", "test")
source = source:gsub("{{VERSION}}", "test")
fs.writeFile("test/.runner_under_test.luau", source)

-- A small live-ish tree the eval can walk.
local dm = roblox.Instance.new("DataModel")
local ws = dm:GetService("Workspace")
local npc = roblox.Instance.new("Model")
npc.Name = "NPC1"
npc.Parent = ws
local root = roblox.Instance.new("Part")
root.Name = "HumanoidRootPart"
root.Position = roblox.Vector3.new(3, 5, 7)
root.Parent = npc

game = dm
_G.TRIPWIRE_RUNNER_TEST = true
local runner = require("./.runner_under_test")

local failures = 0
local function check(label: string, cond: boolean)
	print((cond and "PASS" or "FAIL") .. ": " .. label)
	if not cond then
		failures += 1
	end
end

do
	local ok, data = runner.evalCode("return 1 + 41")
	check("eval returns a value", ok and data.result == 42)
end

do
	local ok, data =
		runner.evalCode("local n = 0 for _, d in game:GetService('Workspace'):GetDescendants() do n = n + 1 end return n")
	check("eval walks the live tree (NPC1 + HumanoidRootPart)", ok and data.result == 2)
end

do
	local ok, data = runner.evalCode("return game:GetService('Workspace').NPC1.HumanoidRootPart")
	check("eval serializes an Instance to {className, path}", ok and type(data.result) == "table" and data.result.className == "Part")
end

do
	local ok, data = runner.evalCode("return game:GetService('Workspace').NPC1.HumanoidRootPart.Position")
	check("eval serializes a Vector3", ok and data.result.x == 3 and data.result.y == 5 and data.result.z == 7)
end

do
	local ok, err = runner.evalCode("this is not valid ))")
	check("compile error is caught, not thrown", (not ok) and type(err) == "string")
end

do
	local ok, err = runner.evalCode("error('boom')")
	check("runtime error is caught, not thrown", (not ok) and string.find(err, "boom") ~= nil)
end

do
	local ok = runner.evalCode("")
	check("empty code is rejected", not ok)
end

do
	local cyclic = {}
	cyclic.self = cyclic
	local ok = pcall(function()
		return runner.serialize(cyclic, 0, {})
	end)
	check("serialize breaks cycles without erroring", ok)
end

fs.removeFile("test/.runner_under_test.luau")

if failures > 0 then
	error(`runner.test: {failures} check(s) failed`)
end
print("runner.test: all checks passed")
