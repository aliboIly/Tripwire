import { ReplicatedStorage, RunService } from "@rbxts/services";

export const REMOTE_NAMES = {
	requestPurchase: "RequestPurchase",
	purchaseResult: "PurchaseResult",
	reportCoinCollected: "ReportCoinCollected",
} as const;

// The server creates each remote; the client waits for it to replicate.
function remoteEvent(name: string): RemoteEvent {
	if (RunService.IsServer()) {
		const existing = ReplicatedStorage.FindFirstChild(name);
		if (existing !== undefined) return existing as RemoteEvent;
		const created = new Instance("RemoteEvent");
		created.Name = name;
		created.Parent = ReplicatedStorage;
		return created;
	}
	return ReplicatedStorage.WaitForChild(name) as RemoteEvent;
}

export const Remotes = {
	requestPurchase: remoteEvent(REMOTE_NAMES.requestPurchase),
	purchaseResult: remoteEvent(REMOTE_NAMES.purchaseResult),
	reportCoinCollected: remoteEvent(REMOTE_NAMES.reportCoinCollected),
};
