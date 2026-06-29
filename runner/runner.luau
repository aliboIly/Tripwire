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
local LogService = game:GetService("LogService")
local UserInputService = game:GetService("UserInputService")

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

local function reply(cmd, ok, dataOrError)
	if ok then
		post("/result", { id = cmd.id, ok = true, data = dataOrError })
	else
		post("/result", { id = cmd.id, ok = false, error = dataOrError })
	end
end

-- VirtualInput drives mouse/keyboard exactly like real hardware. It is client-only
-- (Security None, so a normal injected client script can call it) and may be nil on
-- older Studio. Create it once, lazily.
local virtualInput = nil
local function getVirtualInput()
	if virtualInput == nil and ROLE == "client" then
		virtualInput = UserInputService:CreateVirtualInput()
	end
	return virtualInput
end

local function mouseInput(cmd)
	if ROLE ~= "client" then
		return reply(cmd, false, "mouse input needs a client peer (use start_playtest, not start_simulation)")
	end
	local vi = getVirtualInput()
	if not vi then
		return reply(cmd, false, "VirtualInput is unavailable in this Studio version")
	end
	local p = cmd.payload
	local pos = Vector2.new(p.x or 0, p.y or 0)
	if p.action == "move" then
		vi:SendMousePosition(pos)
	else
		local button = Enum.UserInputType.MouseButton1
		if p.button == "right" then
			button = Enum.UserInputType.MouseButton2
		elseif p.button == "middle" then
			button = Enum.UserInputType.MouseButton3
		end
		vi:SendMouseButton(pos, button, true, 0)
		task.wait()
		vi:SendMouseButton(pos, button, false, 0)
	end
	task.wait() -- let the input pipeline process before reporting
	reply(cmd, true, { x = pos.X, y = pos.Y, action = p.action or "click" })
end

local function keyboardInput(cmd)
	if ROLE ~= "client" then
		return reply(cmd, false, "keyboard input needs a client peer (use start_playtest)")
	end
	local vi = getVirtualInput()
	if not vi then
		return reply(cmd, false, "VirtualInput is unavailable in this Studio version")
	end
	local p = cmd.payload
	if p.text ~= nil then
		vi:SendTextInput(p.text)
	elseif p.key ~= nil then
		local okKey, keyCode = pcall(function()
			return Enum.KeyCode[p.key]
		end)
		if not okKey or keyCode == nil then
			return reply(cmd, false, "unknown key: " .. tostring(p.key))
		end
		local action = p.action or "tap"
		if action == "release" then
			vi:SendKey(false, keyCode, false)
		else
			vi:SendKey(true, keyCode, false) -- isPressed first, then keyCode
			if action == "tap" then
				task.wait()
				vi:SendKey(false, keyCode, false)
			end
		end
	else
		return reply(cmd, false, "keyboard_input needs a 'text' or 'key' field")
	end
	task.wait()
	reply(cmd, true, { ok = true })
end

local function characterNavigation(cmd)
	if ROLE ~= "client" then
		return reply(cmd, false, "navigation needs a client peer (use start_playtest)")
	end
	local p = cmd.payload
	local player = Players.LocalPlayer
	if not player then
		return reply(cmd, false, "no LocalPlayer")
	end
	local character = player.Character or player.CharacterAdded:Wait()
	local humanoid = character:WaitForChild("Humanoid", 10)
	if not humanoid then
		return reply(cmd, false, "character has no Humanoid")
	end
	humanoid:MoveTo(Vector3.new(p.x or 0, p.y or 0, p.z or 0))
	-- MoveToFinished self-times-out after ~8s and fires reached=false even if still
	-- walking, so this Wait never hangs indefinitely.
	local reached = humanoid.MoveToFinished:Wait()
	local position = character:GetPivot().Position
	reply(cmd, true, { reached = reached, position = { position.X, position.Y, position.Z } })
end

local function getLogs(cmd)
	local entries = {}
	for _, entry in ipairs(LogService:GetLogHistory()) do
		table.insert(entries, { message = entry.message, type = entry.messageType.Name, timestamp = entry.timestamp })
	end
	reply(cmd, true, { role = ROLE, entries = entries })
end

local function handle(cmd)
	if cmd.type == "in_play_ping" then
		reply(cmd, true, { role = ROLE, players = #Players:GetPlayers() })
	elseif cmd.type == "stop" then
		-- EndTest must be called from the server DataModel and ends the session.
		-- Reply first, because the DataModel may tear down right after.
		reply(cmd, true, { stopping = true, role = ROLE })
		if ROLE == "server" then
			local studioTest = game:FindService("StudioTestService")
			if studioTest then
				pcall(function()
					studioTest:EndTest(true)
				end)
			end
		end
	elseif cmd.type == "mouse_input" then
		mouseInput(cmd)
	elseif cmd.type == "keyboard_input" then
		keyboardInput(cmd)
	elseif cmd.type == "character_navigation" then
		characterNavigation(cmd)
	elseif cmd.type == "get_logs" then
		getLogs(cmd)
	else
		reply(cmd, false, "unknown in-play command: " .. tostring(cmd.type))
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
