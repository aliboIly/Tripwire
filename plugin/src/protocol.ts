// Wire contract between the Tripwire server and this plugin.
//
// The server holds the authoritative copy of this contract. These declarations
// must match it. The version check in the connect handshake fails loudly if the
// two ever drift, so a stale plugin reports a clear error instead of silently
// dropping fields. Bump PROTOCOL_VERSION on both sides whenever the shapes change.

export const PROTOCOL_VERSION = 2;

// Human-facing build version, shown in the Studio Output log prefix so you can tell
// at a glance which plugin build is loaded. Separate from PROTOCOL_VERSION (the wire
// contract). The runner gets this value templated in by the plugin.
export const TRIPWIRE_VERSION = "0.2.5";

export interface BridgeCommand {
	id: string;
	type: string;
	payload: unknown;
}

export interface CommandResult {
	ok: boolean;
	data?: unknown;
	error?: string;
}
