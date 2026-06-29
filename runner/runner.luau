-- Tripwire in-play runner (Phase 0b / Phase 3).
--
-- This is the one intentionally hand-written Luau file in the project. The
-- plugin injects it into the running game at playtest start, because the plugin
-- lives in a separate DataModel and cannot reach the running game directly.
--
-- It polls the Tripwire bridge for in-play commands, runs them inside the live
-- game, and posts results back.
--
-- PHASE 3 TODO: the bridge must route commands by target so the plugin and this
-- runner don't steal each other's commands. Until then this is only safe to run
-- when the plugin's own poll loop is paused.

local HttpService = game:GetService("HttpService")
local BRIDGE = "http://127.0.0.1:44331"

local function post(path, body)
	pcall(function()
		HttpService:RequestAsync({
			Url = BRIDGE .. path,
			Method = "POST",
			Headers = { ["Content-Type"] = "application/json" },
			Body = HttpService:JSONEncode(body),
		})
	end)
end

local function handle(cmd)
	if cmd.type == "in_play_ping" then
		post("/result", {
			id = cmd.id,
			ok = true,
			data = { runtime = true, players = #game:GetService("Players"):GetPlayers() },
		})
		-- PHASE 3: add user_mouse_input / user_keyboard_input via VirtualInput here,
		-- and StudioTestService:EndTest(...) to stop F8 simulation cleanly.
	else
		post("/result", { id = cmd.id, ok = false, error = "unknown in-play command: " .. tostring(cmd.type) })
	end
end

task.spawn(function()
	while true do
		local ok, res = pcall(function()
			return HttpService:RequestAsync({ Url = BRIDGE .. "/poll", Method = "GET" })
		end)
		if ok and res.Success and res.StatusCode == 200 and res.Body ~= "" then
			handle(HttpService:JSONDecode(res.Body))
		elseif not ok then
			task.wait(2)
		end
	end
end)
