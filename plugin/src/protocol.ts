// Wire contract between the Tripwire server and this plugin.
//
// The server holds the authoritative copy of this contract. These declarations
// must match it. The version check in the connect handshake fails loudly if the
// two ever drift, so a stale plugin reports a clear error instead of silently
// dropping fields. Bump PROTOCOL_VERSION on both sides whenever the shapes change.

export const PROTOCOL_VERSION = 1;

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
