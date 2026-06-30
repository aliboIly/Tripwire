// Screenshot capture. The supported path (used by the maintained robloxstudio-mcp and
// weppy plugins) is a three-call chain: CaptureService:CaptureScreenshot fires a callback
// with a temporary content id, AssetService:CreateEditableImageAsync promotes it to an
// EditableImage (needs "Allow Mesh / Image APIs" on, edit DataModel only), and
// EditableImage:ReadPixelsBuffer hands back the raw RGBA. The server turns the bytes into
// a JPEG. CaptureScreenshot has None security, so it works from the plugin in edit mode.

import { AssetService, CaptureService } from "@rbxts/services";
import { BridgeCommand, CommandResult } from "protocol";
import { encode } from "base64";

const CAPTURE_WAIT_SECONDS = 10;
// Read at most this many pixels per axis so the readback and the bridge payload stay
// bounded. A larger viewport is cropped to this, not scaled (v1 limit).
const MAX_DIMENSION = 1024;

export function handleCapture(cmd: BridgeCommand): CommandResult | undefined {
	if (cmd.type !== "capture_screenshot") return undefined;

	let contentId: string | undefined;
	CaptureService.CaptureScreenshot((id) => {
		contentId = id;
	});
	// The callback fires after a frame renders; wait for it without blocking forever.
	const started = os.clock();
	while (contentId === undefined && os.clock() - started < CAPTURE_WAIT_SECONDS) {
		task.wait();
	}
	if (contentId === undefined) {
		return {
			ok: false,
			error: "screenshot capture did not complete; the Studio window may be minimized or not rendering",
		};
	}

	const [ok, imageOrError] = pcall(() => AssetService.CreateEditableImageAsync(Content.fromUri(contentId as string)));
	if (!ok || imageOrError === undefined) {
		return {
			ok: false,
			error: `could not read the capture. Enable Game Settings > Security > Allow Mesh / Image APIs, then retry. (${imageOrError})`,
		};
	}
	const image = imageOrError as EditableImage;

	const width = math.min(image.Size.X, MAX_DIMENSION);
	const height = math.min(image.Size.Y, MAX_DIMENSION);
	const pixels = image.ReadPixelsBuffer(new Vector2(0, 0), new Vector2(width, height));
	return { ok: true, data: { width, height, data: encode(pixels) } };
}
