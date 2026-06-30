// Spatial query handlers. Read-only world queries that give an agent a sense of the
// scene without a screenshot: cast a ray, measure an instance, and find spawns. These
// run in the edit DataModel against the open place. raycast and Model:GetBoundingBox
// use the physics/geometry engine, so they need real Studio (not a headless runtime).

import { Workspace } from "@rbxts/services";
import { BridgeCommand, CommandResult } from "protocol";
import { resolveInstance } from "read";

const DEFAULT_RAY_DISTANCE = 500;

interface Vec3 {
	x: number;
	y: number;
	z: number;
}

function toVector3(v: Vec3): Vector3 {
	return new Vector3(v.x, v.y, v.z);
}

function fromVector3(v: Vector3): Vec3 {
	return { x: v.X, y: v.Y, z: v.Z };
}

function notFound(path: string | undefined): CommandResult {
	return { ok: false, error: `no instance at path: ${path ?? "game"}` };
}

export function handleSpatial(cmd: BridgeCommand): CommandResult | undefined {
	if (cmd.type === "raycast") {
		const p = cmd.payload as { origin: Vec3; direction: Vec3; maxDistance?: number; ignorePath?: string };
		const origin = toVector3(p.origin);
		// The ray length is the direction's magnitude, or maxDistance along its unit if set.
		let direction = toVector3(p.direction);
		if (p.maxDistance !== undefined) {
			direction = direction.Unit.mul(p.maxDistance);
		} else if (direction.Magnitude === 0) {
			direction = direction.mul(0).add(new Vector3(0, -DEFAULT_RAY_DISTANCE, 0));
		}

		const params = new RaycastParams();
		if (p.ignorePath !== undefined) {
			const ignore = resolveInstance(p.ignorePath);
			if (ignore !== undefined) {
				params.FilterDescendantsInstances = [ignore];
				params.FilterType = Enum.RaycastFilterType.Exclude;
			}
		}

		const result = Workspace.Raycast(origin, direction, params);
		if (result === undefined) return { ok: true, data: { hit: false } };
		return {
			ok: true,
			data: {
				hit: true,
				instance: result.Instance !== undefined ? result.Instance.GetFullName() : undefined,
				position: fromVector3(result.Position),
				normal: fromVector3(result.Normal),
				material: tostring(result.Material),
				distance: result.Distance,
			},
		};
	}

	if (cmd.type === "get_bounding_box") {
		const p = cmd.payload as { path?: string };
		const instance = resolveInstance(p.path);
		if (instance === undefined) return notFound(p.path);
		if (instance.IsA("Model")) {
			const [cframe, size] = instance.GetBoundingBox();
			return { ok: true, data: { center: fromVector3(cframe.Position), size: fromVector3(size) } };
		}
		if (instance.IsA("BasePart")) {
			return { ok: true, data: { center: fromVector3(instance.Position), size: fromVector3(instance.Size) } };
		}
		return { ok: false, error: `${instance.GetFullName()} is a ${instance.ClassName}, which has no bounding box` };
	}

	if (cmd.type === "find_spawns") {
		const p = cmd.payload as { path?: string };
		const root = resolveInstance(p.path);
		if (root === undefined) return notFound(p.path);
		const spawns: Array<{ path: string; position: Vec3; enabled: boolean; neutral: boolean }> = [];
		for (const descendant of root.GetDescendants()) {
			if (!descendant.IsA("SpawnLocation")) continue;
			spawns.push({
				path: descendant.GetFullName(),
				position: fromVector3(descendant.Position),
				enabled: descendant.Enabled,
				neutral: descendant.Neutral,
			});
		}
		return { ok: true, data: { spawns } };
	}

	return undefined;
}
