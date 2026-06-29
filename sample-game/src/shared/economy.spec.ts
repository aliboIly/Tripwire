import { Economy } from "shared/economy";
import { equal, isFalse, isTrue } from "shared/harness/testkit";
import type { Suite } from "shared/harness/testkit";

// Each case builds fresh state; there is no beforeEach in the micro runner.
export = (t: Suite) => {
	t.case("starts with the default balance", () => equal(new Economy().getCoins(1), 50));

	t.case("a purchase debits the balance and grants the item", () => {
		const economy = new Economy();
		const outcome = economy.tryPurchase(1, "potion");
		isTrue(outcome.ok);
		equal(outcome.balance, 25);
		isTrue(economy.owns(1, "potion"));
	});

	t.case("rejects an item the player cannot afford", () => {
		const outcome = new Economy().tryPurchase(1, "sword");
		isFalse(outcome.ok);
		equal(outcome.balance, 50);
	});

	t.case("rejects an unknown item", () => {
		const outcome = new Economy().tryPurchase(1, "banana");
		isFalse(outcome.ok);
	});

	t.case("granting coins raises the balance", () => {
		const economy = new Economy();
		economy.grant(1, 30);
		equal(economy.getCoins(1), 80);
	});
};
