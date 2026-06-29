import { Remotes } from "shared/remotes";

Remotes.purchaseResult.OnClientEvent.Connect((outcome) => {
	print("[sample] purchase result:", outcome);
});

// Normal client behavior: ask to buy something and report a coin pickup.
Remotes.requestPurchase.FireServer("sword");
Remotes.reportCoinCollected.FireServer(5);
