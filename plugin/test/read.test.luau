--!strict
-- Headless test for the read handlers. Lune (@lune/roblox) gives us a real DataModel,
-- so we run the ACTUAL compiled handler (out/read.luau) against it rather than a mock.
-- read.luau imports only erased types, so it is self-contained and needs just the
-- Roblox globals it references injected here. Run with: lune run test/read.test.luau
-- (after `npm run build`). Exits non-zero on a failed check.

local roblox = require("@lune/roblox")

local dm = roblox.Instance.new("DataModel")
local ws = roblox.Instance.new("Workspace")
ws.Parent = dm
local floor = roblox.Instance.new("Part")
floor.Name = "Floor"
floor.Anchored = true
floor.Size = roblox.Vector3.new(10, 1, 10)
floor.Parent = ws
local ramp = roblox.Instance.new("WedgePart")
ramp.Name = "Ramp"
ramp.Parent = ws
local gui = roblox.Instance.new("ScreenGui")
gui.Name = "HUD"
gui.Parent = dm

-- The compiled roblox-ts handler reads these as globals. Plain (non-local) assignment
-- is visible across Lune's require boundary.
game = dm
typeIs = function(value: any, kind: string): boolean
	return typeof(value) == kind
end

local read = require("../out/read")

local failures = 0
local function check(label: string, ok: boolean)
	print((ok and "PASS" or "FAIL") .. ": " .. label)
	if not ok then
		failures += 1
	end
end

-- isA matches a class and its subclasses.
local isaMatches = read.handleRead({ type = "search_objects", payload = { query = "", isA = "BasePart" } }).data.matches
local byName: { [string]: boolean } = {}
for _, match in isaMatches do
	byName[match.name] = true
end
check("isA BasePart matches a Part", byName["Floor"] == true)
check("isA BasePart matches a WedgePart subclass", byName["Ramp"] == true)
check("isA BasePart excludes a ScreenGui", byName["HUD"] == nil)

-- Exact className keeps matching only that class, not subclasses.
local exact = read.handleRead({ type = "search_objects", payload = { query = "", className = "Part" } }).data.matches
local exactByName: { [string]: boolean } = {}
for _, match in exact do
	exactByName[match.name] = true
end
check("exact className Part matches the Part", exactByName["Floor"] == true)
check("exact className Part excludes the WedgePart", exactByName["Ramp"] == nil)

-- get_instance_properties reads back real engine property values.
local props = read.handleRead({ type = "get_instance_properties", payload = { path = "Workspace.Floor" } }).data.properties
check("reads Anchored we set to true", props.Anchored == true)
check("reads Size as a serialized string", typeof(props.Size) == "string")
check("includes a curated property the class has (Material)", props.Material ~= nil)
check("omits a property the class lacks (Text)", props.Text == nil)

if failures > 0 then
	error(`read.test: {failures} check(s) failed`)
end
print("read.test: all checks passed")
