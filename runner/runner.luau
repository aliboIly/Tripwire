-- Tripwire in-play runner (Phase 3).
--
-- This is the one intentionally hand-written Luau file in the project. The plugin
-- injects it into the running game at playtest start, because the plugin (edit
-- DataModel) and the playtest (separate server and
-- client DataModels) are memory-isolated and the plugin cannot reach the running
-- game. The runner self-detects its peer (server or client).
--
-- HTTP is server-only in Roblox (HttpService:RequestAsync errors on the client), so
-- ONLY the server peer talks to the bridge. Client input (VirtualInput is
-- client-only) is relayed: the server forwards a client command over a RemoteEvent,
-- the client runs it and replies, and the server posts the result back. So the
-- server peer is the only bridge client; the client peer is reached through the relay.
--
-- This file is a TEMPLATE: the server fills the {{PORT}}, {{PROTOCOL_VERSION}},
-- {{INSTANCE_ID}}, and {{VERSION}} tokens before injection, so it is not valid Luau
-- on disk.

local HttpService = game:GetService("HttpService")
local RunService = game:GetService("RunService")
local Players = game:GetService("Players")
local LogService = game:GetService("LogService")
local UserInputService = game:GetService("UserInputService")
local ReplicatedStorage = game:GetService("ReplicatedStorage")

local BRIDGE = "http://127.0.0.1:{{PORT}}"
local PROTOCOL_VERSION = {{PROTOCOL_VERSION}}
local INSTANCE_ID = "{{INSTANCE_ID}}"
local ROLE = RunService:IsServer() and "server" or "client"
local LOG = "[Tripwire runner v{{VERSION}}]"

local RELAY_NAME = "TripwireRelay"
local RELAY_READY_TIMEOUT = 8 -- seconds to wait for the player + client peer to come up
local RELAY_REPLY_TIMEOUT = 25 -- seconds to wait for the client's reply (covers WaitForChild Humanoid 10s + MoveTo ~8s)

-- ===== command execution (the input ones run on the client; each returns ok, dataOrError) =====

-- VirtualInput drives mouse/keyboard exactly like real hardware. It is client-only
-- (Security None, so an injected client script can call it) and may be nil on older
-- Studio. Create it once, lazily.
local virtualInput = nil
local function getVirtualInput()
	if virtualInput == nil and ROLE == "client" then
		virtualInput = UserInputService:CreateVirtualInput()
	end
	return virtualInput
end

local function doMouseInput(p)
	local vi = getVirtualInput()
	if not vi then
		return false, "VirtualInput is unavailable in this Studio version"
	end
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
	return true, { x = pos.X, y = pos.Y, action = p.action or "click" }
end

local function doKeyboardInput(p)
	local vi = getVirtualInput()
	if not vi then
		return false, "VirtualInput is unavailable in this Studio version"
	end
	if p.text ~= nil then
		vi:SendTextInput(p.text)
	elseif p.key ~= nil then
		local okKey, keyCode = pcall(function()
			return Enum.KeyCode[p.key]
		end)
		if not okKey or keyCode == nil then
			return false, "unknown key: " .. tostring(p.key)
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
		return false, "keyboard_input needs a 'text' or 'key' field"
	end
	task.wait()
	return true, { ok = true }
end

local function doCharacterNavigation(p)
	local player = Players.LocalPlayer
	if not player then
		return false, "no LocalPlayer"
	end
	local character = player.Character or player.CharacterAdded:Wait()
	local humanoid = character:WaitForChild("Humanoid", 10)
	if not humanoid then
		return false, "character has no Humanoid"
	end
	humanoid:MoveTo(Vector3.new(p.x or 0, p.y or 0, p.z or 0))
	-- MoveToFinished self-times-out after ~8s and fires reached=false even if still
	-- walking, so this Wait never hangs indefinitely.
	local reached = humanoid.MoveToFinished:Wait()
	local position = character:GetPivot().Position
	return true, { reached = reached, position = { position.X, position.Y, position.Z } }
end

local function collectLogs()
	local entries = {}
	for _, entry in ipairs(LogService:GetLogHistory()) do
		table.insert(entries, { message = entry.message, type = entry.messageType.Name, timestamp = entry.timestamp })
	end
	return entries
end

-- ===== client peer: no HTTP; listens on the relay, runs commands, replies =====

local CLIENT_HANDLERS = {
	mouse_input = doMouseInput,
	keyboard_input = doKeyboardInput,
	character_navigation = doCharacterNavigation,
	get_logs = function()
		return true, { entries = collectLogs() }
	end,
}

local function runClient()
	local relay = ReplicatedStorage:WaitForChild(RELAY_NAME, 30)
	if not relay then
		warn(LOG .. " relay event never appeared; client input is unavailable")
		return
	end
	relay.OnClientEvent:Connect(function(cmd)
		local fn = CLIENT_HANDLERS[cmd.type]
		if not fn then
			relay:FireServer({ id = cmd.id, ok = false, error = "unknown client command: " .. tostring(cmd.type) })
			return
		end
		local ran, okOrErr, dataOrErr = pcall(fn, cmd.payload)
		if not ran then
			relay:FireServer({ id = cmd.id, ok = false, error = "client error: " .. tostring(okOrErr) })
		elseif okOrErr then
			relay:FireServer({ id = cmd.id, ok = true, data = dataOrErr })
		else
			relay:FireServer({ id = cmd.id, ok = false, error = dataOrErr })
		end
	end)
	relay:FireServer({ ready = true }) -- tell the server the client peer is listening
end

-- ===== server peer: talks to the bridge over HTTP; relays client commands =====

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
		-- Surface in-play failures in the Studio Output too, not just over the bridge.
		warn(LOG .. " " .. tostring(cmd.type) .. ": " .. tostring(dataOrError))
		post("/result", { id = cmd.id, ok = false, error = dataOrError })
	end
end

local relayEvent = nil
local pendingReplies = {} -- relay command id -> reply table (only ids still awaited)
local awaiting = {} -- relay command id -> true while relayToClient is waiting on it
local clientReady = false

local function setupRelay()
	relayEvent = Instance.new("RemoteEvent")
	relayEvent.Name = RELAY_NAME
	relayEvent.Parent = ReplicatedStorage
	relayEvent.OnServerEvent:Connect(function(_player, msg)
		if type(msg) ~= "table" then
			return
		end
		if msg.ready then
			clientReady = true
		elseif msg.id ~= nil and awaiting[msg.id] then
			-- Only store replies still being awaited, so a late reply arriving after a
			-- timeout cannot accumulate as an orphan entry.
			pendingReplies[msg.id] = msg
		end
	end)
end

-- Forwards a client command to the player and waits for its reply. Returns ok,
-- dataOrError. Runs on the server poll thread; the OnServerEvent connection fills
-- pendingReplies during the task.wait yields.
local function relayToClient(cmd)
	-- Wait briefly for both a player and the client runner. A player can be absent
	-- (F8) or simply not spawned yet (an early F5 call); both look the same here.
	local waited = 0
	while (Players:GetPlayers()[1] == nil or not clientReady) and waited < RELAY_READY_TIMEOUT do
		task.wait(0.2)
		waited = waited + 0.2
	end
	local player = Players:GetPlayers()[1]
	if not player then
		return false, "no client peer connected (an F5 playtest has a player; an F8 simulation does not)"
	end
	if not clientReady then
		return false, "client runner did not come up within " .. RELAY_READY_TIMEOUT .. "s"
	end

	awaiting[cmd.id] = true
	relayEvent:FireClient(player, { id = cmd.id, type = cmd.type, payload = cmd.payload })
	local elapsed = 0
	while pendingReplies[cmd.id] == nil and elapsed < RELAY_REPLY_TIMEOUT do
		task.wait(0.1)
		elapsed = elapsed + 0.1
	end
	local r = pendingReplies[cmd.id]
	pendingReplies[cmd.id] = nil
	awaiting[cmd.id] = nil
	if r == nil then
		return false, "client did not reply within " .. RELAY_REPLY_TIMEOUT .. "s"
	end
	if r.ok then
		return true, r.data
	end
	return false, r.error
end

local function handle(cmd)
	if cmd.type == "in_play_ping" then
		reply(cmd, true, { role = ROLE, players = #Players:GetPlayers() })
	elseif cmd.type == "stop" then
		-- EndTest runs from the server DataModel and ends the session. Reply first,
		-- because the DataModel may tear down right after.
		reply(cmd, true, { stopping = true, role = ROLE })
		local studioTest = game:FindService("StudioTestService")
		if studioTest then
			pcall(function()
				studioTest:EndTest(true)
			end)
		end
	elseif cmd.type == "mouse_input" or cmd.type == "keyboard_input" or cmd.type == "character_navigation" then
		local ok, dataOrError = relayToClient(cmd)
		reply(cmd, ok, dataOrError)
	elseif cmd.type == "get_logs" then
		-- Server logs plus the client's, fetched over the relay. The two arrays are
		-- merged and deduped on the server side (Node), not here.
		local clientEntries = {}
		local cok, cdata = relayToClient({ id = cmd.id, type = "get_logs", payload = {} })
		if cok and type(cdata) == "table" and cdata.entries then
			clientEntries = cdata.entries
		end
		reply(cmd, true, { serverEntries = collectLogs(), clientEntries = clientEntries })
	else
		reply(cmd, false, "unknown in-play command: " .. tostring(cmd.type))
	end
end

local function announce()
	local ok, res = pcall(function()
		return HttpService:RequestAsync({
			Url = BRIDGE .. "/hello",
			Method = "POST",
			Headers = { ["Content-Type"] = "application/json" },
			Body = HttpService:JSONEncode({ protocolVersion = PROTOCOL_VERSION, role = ROLE, instanceId = INSTANCE_ID }),
		})
	end)
	if not ok then
		warn(LOG .. " cannot reach the bridge. Is Allow HTTP Requests enabled in Game Settings?")
		return false
	end
	if not res.Success then
		warn(LOG .. " bridge refused the handshake: " .. tostring(res.Body))
		return false
	end
	return true
end

local function runServer()
	setupRelay() -- create the relay before announcing, so the client can find it
	if not announce() then
		return
	end
	while true do
		local ok, res = pcall(function()
			return HttpService:RequestAsync({ Url = BRIDGE .. "/poll?studio=" .. INSTANCE_ID .. "&role=" .. ROLE, Method = "GET" })
		end)
		if ok and res.Success then
			if res.StatusCode == 205 then
				-- The server lost our registration (it restarted); re-announce.
				if not announce() then
					task.wait(2)
				end
			elseif res.StatusCode == 200 and res.Body ~= "" then
				-- 204 means the long-poll window elapsed with nothing queued. Dispatch on
				-- its own thread so a relay's wait cannot block the poll loop, which must
				-- keep polling to heartbeat and to accept stop while a relay is open.
				task.spawn(handle, HttpService:JSONDecode(res.Body))
			end
		elseif ok then
			-- Reachable but an unexpected status; back off so a broken endpoint
			-- cannot busy-spin.
			task.wait(2)
		else
			warn(LOG .. " bridge unreachable; retrying.")
			task.wait(2)
		end
	end
end

if ROLE == "client" then
	task.spawn(runClient)
else
	task.spawn(runServer)
end
