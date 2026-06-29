# tripwire-roblox

The [Tripwire](https://github.com/aliboIly/Tripwire) MCP server for Roblox Studio and Open Cloud, packaged for npm. Installing this package downloads the prebuilt Rust binary for your platform from the matching GitHub Release.

## Use it

Point an MCP client at it with `npx`:

```json
{
  "mcpServers": {
    "tripwire": {
      "command": "npx",
      "args": ["-y", "tripwire-roblox"]
    }
  }
}
```

Or run it directly: `npx -y tripwire-roblox`.

The Studio tools also need the Tripwire Studio plugin (`Tripwire.rbxmx` on the [Releases](https://github.com/aliboIly/Tripwire/releases) page). See the [main README](https://github.com/aliboIly/Tripwire#readme) for the full setup, the tool list, and the Open Cloud key walkthrough.

## Supported platforms

macOS (arm64, x64), Linux (x64), and Windows (x64). On anything else, build from source with `cargo` (see the main README).
