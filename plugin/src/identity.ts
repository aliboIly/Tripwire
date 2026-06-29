// Per-Studio identity sent to the bridge.
//
// instanceId is a fresh GUID generated once per plugin load, held in memory. It is
// the bridge's connection key, unique even when two Studio windows open the same
// place (placeId/placeName are shared, so they cannot be keys). installId is a GUID
// persisted in plugin settings for labeling across restarts, not a key (plugin
// settings are one shared file per install, so every window reads the same value).

import { HttpService } from "@rbxts/services";

export const INSTANCE_ID = HttpService.GenerateGUID(false);

const INSTALL_ID_KEY = "tripwireInstallId";

// `plugin` is a global only in the plugin's main script, not in required modules,
// so the caller (main.server) passes it in.
export function installId(pluginInstance: Plugin): string {
	const existing = pluginInstance.GetSetting(INSTALL_ID_KEY);
	if (typeIs(existing, "string")) return existing;
	const created = HttpService.GenerateGUID(false);
	pluginInstance.SetSetting(INSTALL_ID_KEY, created);
	return created;
}
