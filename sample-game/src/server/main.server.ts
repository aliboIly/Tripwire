import { Economy } from "shared/economy";
import { Remotes } from "shared/remotes";

const economy = new Economy();

// Purchases are server-authoritative: validate the payload, then let the economy
// check affordability and decide. The client only sends the item id.
Remotes.requestPurchase.OnServerEvent.Connect((player, itemId) => {
	if (!typeIs(itemId, "string")) return;
	const outcome = economy.tryPurchase(player.UserId, itemId);
	Remotes.purchaseResult.FireClient(player, outcome);
});

// Awards coins when a player reports collecting one. The amount is taken straight
// from the client, so a crafted FireServer call can grant any number of coins.
// This is the deliberate client-trust hole that review_security is meant to catch.
Remotes.reportCoinCollected.OnServerEvent.Connect((player, amount) => {
	economy.grant(player.UserId, amount as number);
});
