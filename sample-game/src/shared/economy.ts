// Economy is the server-authoritative core of the sample game. It tracks coin
// balances and decides purchases. It has no Roblox API dependencies, so the
// headless test harness can exercise it directly through Open Cloud.

export const STARTING_COINS = 50;

// A real coin pickup is worth a fixed amount that the server decides. The server
// handler is where this is meant to be enforced.
export const COIN_PICKUP_VALUE = 5;

export const ITEM_PRICES = new Map<string, number>([
	["sword", 100],
	["shield", 60],
	["potion", 25],
]);

export interface PurchaseOutcome {
	ok: boolean;
	itemId?: string;
	reason?: string;
	balance: number;
}

export class Economy {
	private readonly balances = new Map<number, number>();
	private readonly owned = new Map<number, Set<string>>();

	getCoins(userId: number): number {
		return this.balances.get(userId) ?? STARTING_COINS;
	}

	grant(userId: number, amount: number): number {
		const updated = this.getCoins(userId) + amount;
		this.balances.set(userId, updated);
		return updated;
	}

	owns(userId: number, itemId: string): boolean {
		return this.owned.get(userId)?.has(itemId) ?? false;
	}

	// Server-authoritative: confirm the item exists and the player can afford it
	// before charging and granting. The client never decides the outcome.
	tryPurchase(userId: number, itemId: string): PurchaseOutcome {
		const price = ITEM_PRICES.get(itemId);
		if (price === undefined) {
			return { ok: false, reason: "unknown item", balance: this.getCoins(userId) };
		}
		const coins = this.getCoins(userId);
		if (coins < price) {
			return { ok: false, reason: "not enough coins", balance: coins };
		}
		const balance = coins - price;
		this.balances.set(userId, balance);
		const set = this.owned.get(userId) ?? new Set<string>();
		set.add(itemId);
		this.owned.set(userId, set);
		return { ok: true, itemId, balance };
	}
}
