-- Tripwire in-play runner (Phase 3).
--
-- This is the one intentionally hand-written Luau file in the project (see
-- Docs/ARCHITECTURE.md). The plugin injects it into the running game at playtest
-- start, because the plugin (edit DataModel) and the playtest (separate server
-- and client DataModels) are memory-isolated and the plugin cannot reach into the
-- running game. The runner self-detects its peer (server or client), registers
-- with the bridge under that role, long-polls for in-play commands, runs them
-- inside the live game, and posts results back.
--
-- This file is a TEMPLATE: the server fills in the {{PORT}} and
-- {{PROTOCOL_VERSION}} tokens before injection, so it is not valid Luau on disk.

local HttpService = game:GetService("HttpService")
local RunService = game:GetService("RunService")
local Players = game:GetService("Players")

local BRIDGE = "http://127.0.0.1:{{PORT}}"
local PROTOCOL_VERSION = {{PROTOCOL_VERSION}}
local ROLE = RunService:IsServer() and "server" or "client"

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
		post("/result", { id = cmd.id, ok = true, data = { role = ROLE, players = #Players:GetPlayers() } })
	elseif cmd.type == "stop" then
		-- EndTest must be called from the server DataModel and ends the session,
		-- which is how an F5/F8 test is stopped from inside the running game. Post
		-- the result first, because the DataModel may tear down right after.
		post("/result", { id = cmd.id, ok = true, data = { stopping = true, role = ROLE } })
		if ROLE == "server" then
			local studioTest = game:FindService("StudioTestService")
			if studioTest then
				pcall(function()
					studioTest:EndTest(true)
				end)
			end
		end
	else
		post("/result", { id = cmd.id, ok = false, error = "unknown in-play command: " .. tostring(cmd.type) })
	end
end

local function run()
	local okHello, hello = pcall(function()
		return HttpService:RequestAsync({
			Url = BRIDGE .. "/hello",
			Method = "POST",
			Headers = { ["Content-Type"] = "application/json" },
			Body = HttpService:JSONEncode({ protocolVersion = PROTOCOL_VERSION, role = ROLE }),
		})
	end)
	if not okHello then
		warn("[Tripwire runner] cannot reach the bridge. Is Allow HTTP Requests enabled in Game Settings?")
		return
	end
	if not hello.Success then
		warn("[Tripwire runner] bridge refused the handshake: " .. tostring(hello.Body))
		return
	end

	while true do
		local ok, res = pcall(function()
			return HttpService:RequestAsync({ Url = BRIDGE .. "/poll?role=" .. ROLE, Method = "GET" })
		end)
		if ok and res.Success then
			-- 200 with a body is a command; 204 means the long-poll window elapsed,
			-- so poll again right away.
			if res.StatusCode == 200 and res.Body ~= "" then
				handle(HttpService:JSONDecode(res.Body))
			end
		elseif ok then
			-- Reachable but an unexpected status; back off so a broken endpoint
			-- cannot busy-spin.
			task.wait(2)
		else
			warn("[Tripwire runner] bridge unreachable; retrying.")
			task.wait(2)
		end
	end
end

task.spawn(run)
