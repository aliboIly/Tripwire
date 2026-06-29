#!/usr/bin/env node
// Thin launcher: run the native tripwire-server that postinstall placed next to this
// file, forwarding stdio (the MCP stream) and any arguments, and mirroring its exit.

const path = require("path");
const { spawn } = require("child_process");

const binName = process.platform === "win32" ? "tripwire-server.exe" : "tripwire-server";
const binPath = path.join(__dirname, binName);

const child = spawn(binPath, process.argv.slice(2), { stdio: "inherit" });

child.on("error", (err) => {
  console.error(`[tripwire-roblox] failed to start the server: ${err.message}`);
  console.error("[tripwire-roblox] Try reinstalling, or build from source: https://github.com/aliboIly/Tripwire");
  process.exit(1);
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exit(code ?? 0);
  }
});
