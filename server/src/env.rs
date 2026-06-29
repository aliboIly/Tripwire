// Credentials and configuration from the environment only. The Open Cloud key is
// read here and passed in the x-api-key header; it is never logged.

use std::env;

/// A set environment variable, treating empty as unset.
pub fn var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

pub struct CloudCreds {
    pub key: String,
    pub universe: String,
    pub place: String,
}

/// The credentials the Luau-execution and place-scoped Open Cloud tools need.
pub fn cloud_creds() -> Result<CloudCreds, String> {
    match (
        var("ROBLOX_OPEN_CLOUD_KEY"),
        var("ROBLOX_UNIVERSE_ID"),
        var("ROBLOX_PLACE_ID"),
    ) {
        (Some(key), Some(universe), Some(place)) => Ok(CloudCreds {
            key,
            universe,
            place,
        }),
        _ => Err(
            "Missing env: set ROBLOX_OPEN_CLOUD_KEY, ROBLOX_UNIVERSE_ID, ROBLOX_PLACE_ID.".into(),
        ),
    }
}

/// The bridge port. Fixed to 44331 unless overridden, matching the plugin default.
pub fn bridge_port() -> u16 {
    var("TRIPWIRE_BRIDGE_PORT")
        .and_then(|p| p.parse().ok())
        .unwrap_or(44331)
}
