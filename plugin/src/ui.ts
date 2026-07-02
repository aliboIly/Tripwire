// The Tripwire panel: a dockable Studio widget in the same visual language as
// Rojo's, built from plain Instances. The panel is one screen, so a UI
// framework would outweigh it. Every color resolves from the active Studio
// theme and re-resolves on ThemeChanged, so the panel tracks light and dark.
//
// The panel is presentation only: it renders whatever connection state
// main.server pushes in and reports button presses back through a callback.
// The `plugin` global only exists in the main script, so the caller passes it in.

import { TRIPWIRE_VERSION } from "protocol";

export type ConnectionStatus = "disconnected" | "connecting" | "connected" | "reconnecting";

export interface Panel {
	setStatus(status: ConnectionStatus, message?: string): void;
	noteCommand(commandType: string): void;
	toggle(): void;
	isVisible(): boolean;
	onVisibleChanged(callback: (visible: boolean) => void): void;
}

const WIDGET_ID = "TripwirePanel";
const FLOAT_WIDTH = 300;
const FLOAT_HEIGHT = 240;
const MIN_WIDTH = 260;
const MIN_HEIGHT = 200;

const PADDING = 12;
const ROW_GAP = 6;
const HEADER_HEIGHT = 22;
const STATUS_ROW_HEIGHT = 20;
const ROW_HEIGHT = 18;
const CAPTION_WIDTH = 60;
const BUTTON_HEIGHT = 30;
const DOT_SIZE = 8;
const CORNER_RADIUS = 4;

const WORDMARK_TEXT_SIZE = 20;
const TEXT_SIZE = 14;
const BUTTON_TEXT_SIZE = 15;

// The style guide has no "healthy" or "in progress" colors, so the status dot
// uses two fixed ones, deep enough to hold roughly 3:1 contrast against both
// the light theme's white and the dark theme's near-black backgrounds.
const CONNECTED_GREEN = Color3.fromRGB(64, 154, 92);
const BUSY_AMBER = Color3.fromRGB(181, 129, 38);

const StyleColor = Enum.StudioStyleGuideColor;
const StyleModifier = Enum.StudioStyleGuideModifier;

function themeColor(item: Enum.StudioStyleGuideColor, modifier?: Enum.StudioStyleGuideModifier): Color3 {
	return settings().Studio.Theme.GetColor(item, modifier);
}

function makeLabel(text: string, textSize: number, font: Enum.Font): TextLabel {
	const label = new Instance("TextLabel");
	label.BackgroundTransparency = 1;
	label.Font = font;
	label.Text = text;
	label.TextSize = textSize;
	label.TextXAlignment = Enum.TextXAlignment.Left;
	return label;
}

export function createPanel(pluginInstance: Plugin, bridgeUrl: string, onToggleConnection: () => void): Panel {
	const [bridgeAddress] = bridgeUrl.gsub("^https?://", "");

	const widget = pluginInstance.CreateDockWidgetPluginGui(
		WIDGET_ID,
		new DockWidgetPluginGuiInfo(
			Enum.InitialDockState.Float,
			false,
			false,
			FLOAT_WIDTH,
			FLOAT_HEIGHT,
			MIN_WIDTH,
			MIN_HEIGHT,
		),
	);
	// PluginGui.Title exists in the engine but @rbxts/types does not expose it.
	(widget as DockWidgetPluginGui & { Title: string }).Title = "Tripwire";
	widget.Name = "Tripwire";
	widget.ZIndexBehavior = Enum.ZIndexBehavior.Sibling;

	const root = new Instance("Frame");
	root.Size = new UDim2(1, 0, 1, 0);
	root.BorderSizePixel = 0;

	const rootPadding = new Instance("UIPadding");
	rootPadding.PaddingTop = new UDim(0, PADDING);
	rootPadding.PaddingBottom = new UDim(0, PADDING);
	rootPadding.PaddingLeft = new UDim(0, PADDING);
	rootPadding.PaddingRight = new UDim(0, PADDING);
	rootPadding.Parent = root;

	// The rows stack from the top; the connect button anchors to the bottom, so
	// resizing the widget stretches the empty middle rather than the content.
	const body = new Instance("Frame");
	body.BackgroundTransparency = 1;
	body.Size = new UDim2(1, 0, 1, -(BUTTON_HEIGHT + ROW_GAP));
	body.Parent = root;

	const column = new Instance("UIListLayout");
	column.FillDirection = Enum.FillDirection.Vertical;
	column.SortOrder = Enum.SortOrder.LayoutOrder;
	column.Padding = new UDim(0, ROW_GAP);
	column.Parent = body;

	const header = new Instance("Frame");
	header.BackgroundTransparency = 1;
	header.Size = new UDim2(1, 0, 0, HEADER_HEIGHT);
	header.LayoutOrder = 0;
	header.Parent = body;

	const headerRow = new Instance("UIListLayout");
	headerRow.FillDirection = Enum.FillDirection.Horizontal;
	headerRow.SortOrder = Enum.SortOrder.LayoutOrder;
	headerRow.Padding = new UDim(0, ROW_GAP);
	headerRow.Parent = header;

	const wordmark = makeLabel("Tripwire", WORDMARK_TEXT_SIZE, Enum.Font.SourceSansBold);
	wordmark.Size = new UDim2(0, 0, 1, 0);
	wordmark.AutomaticSize = Enum.AutomaticSize.X;
	wordmark.TextYAlignment = Enum.TextYAlignment.Bottom;
	wordmark.LayoutOrder = 0;
	wordmark.Parent = header;

	// One point smaller than body text and raised 2px so its baseline sits
	// optically on the wordmark's; both are bottom-aligned in the header row.
	const version = makeLabel(`v${TRIPWIRE_VERSION}`, TEXT_SIZE - 1, Enum.Font.SourceSans);
	version.Size = new UDim2(0, 0, 1, -2);
	version.AutomaticSize = Enum.AutomaticSize.X;
	version.TextYAlignment = Enum.TextYAlignment.Bottom;
	version.LayoutOrder = 1;
	version.Parent = header;

	const separator = new Instance("Frame");
	separator.BorderSizePixel = 0;
	separator.Size = new UDim2(1, 0, 0, 1);
	separator.LayoutOrder = 1;
	separator.Parent = body;

	const statusRow = new Instance("Frame");
	statusRow.BackgroundTransparency = 1;
	statusRow.Size = new UDim2(1, 0, 0, STATUS_ROW_HEIGHT);
	statusRow.LayoutOrder = 2;
	statusRow.Parent = body;

	const statusLayout = new Instance("UIListLayout");
	statusLayout.FillDirection = Enum.FillDirection.Horizontal;
	statusLayout.SortOrder = Enum.SortOrder.LayoutOrder;
	statusLayout.VerticalAlignment = Enum.VerticalAlignment.Center;
	statusLayout.Padding = new UDim(0, ROW_GAP);
	statusLayout.Parent = statusRow;

	const dot = new Instance("Frame");
	dot.BorderSizePixel = 0;
	dot.Size = new UDim2(0, DOT_SIZE, 0, DOT_SIZE);
	dot.LayoutOrder = 0;
	dot.Parent = statusRow;

	const dotCorner = new Instance("UICorner");
	dotCorner.CornerRadius = new UDim(1, 0);
	dotCorner.Parent = dot;

	const statusText = makeLabel("Disconnected", TEXT_SIZE, Enum.Font.SourceSansSemibold);
	statusText.Size = new UDim2(1, -(DOT_SIZE + ROW_GAP), 1, 0);
	statusText.LayoutOrder = 1;
	statusText.Parent = statusRow;

	const captionLabels: Array<TextLabel> = [];

	function detailRow(order: number, captionText: string): TextLabel {
		const row = new Instance("Frame");
		row.BackgroundTransparency = 1;
		row.Size = new UDim2(1, 0, 0, ROW_HEIGHT);
		row.LayoutOrder = order;
		row.Parent = body;

		const caption = makeLabel(captionText, TEXT_SIZE, Enum.Font.SourceSans);
		caption.Size = new UDim2(0, CAPTION_WIDTH, 1, 0);
		caption.Parent = row;
		captionLabels.push(caption);

		const value = makeLabel("", TEXT_SIZE, Enum.Font.SourceSans);
		value.Position = new UDim2(0, CAPTION_WIDTH, 0, 0);
		value.Size = new UDim2(1, -CAPTION_WIDTH, 1, 0);
		value.TextTruncate = Enum.TextTruncate.AtEnd;
		value.Parent = row;
		return value;
	}

	const serverValue = detailRow(3, "Server");
	const placeValue = detailRow(4, "Place");
	const activityValue = detailRow(5, "Activity");
	serverValue.Text = bridgeAddress;
	activityValue.Text = "no commands yet";

	const messageLabel = makeLabel("", TEXT_SIZE, Enum.Font.SourceSans);
	messageLabel.Size = new UDim2(1, 0, 0, 0);
	messageLabel.TextWrapped = true;
	messageLabel.TextTruncate = Enum.TextTruncate.AtEnd;
	messageLabel.TextYAlignment = Enum.TextYAlignment.Top;
	messageLabel.LayoutOrder = 6;
	messageLabel.Visible = false;
	messageLabel.Parent = body;

	// Fill whatever height the fixed rows leave in the body. AutomaticSize would
	// let a long error grow past the body and disappear under the button.
	const messageFlex = new Instance("UIFlexItem");
	messageFlex.FlexMode = Enum.UIFlexMode.Fill;
	messageFlex.Parent = messageLabel;

	const connectButton = new Instance("TextButton");
	connectButton.AnchorPoint = new Vector2(0, 1);
	connectButton.Position = new UDim2(0, 0, 1, 0);
	connectButton.Size = new UDim2(1, 0, 0, BUTTON_HEIGHT);
	connectButton.AutoButtonColor = false;
	connectButton.BorderSizePixel = 0;
	connectButton.Font = Enum.Font.SourceSansSemibold;
	connectButton.Text = "Connect";
	connectButton.TextSize = BUTTON_TEXT_SIZE;
	connectButton.Parent = root;

	const buttonCorner = new Instance("UICorner");
	buttonCorner.CornerRadius = new UDim(0, CORNER_RADIUS);
	buttonCorner.Parent = connectButton;

	const buttonStroke = new Instance("UIStroke");
	// Contextual (the default) strokes the glyphs on a text object, not the box.
	buttonStroke.ApplyStrokeMode = Enum.ApplyStrokeMode.Border;
	buttonStroke.Parent = connectButton;

	root.Parent = widget;

	let status: ConnectionStatus = "disconnected";
	let message: string | undefined = undefined;
	let hovered = false;
	let commandCount = 0;

	function applyButtonTheme(): void {
		// Connect is the panel's one primary action; Disconnect renders as a
		// neutral button so a healthy connected panel reads calm.
		const primary = status === "disconnected";
		const modifier =
			status === "connecting" ? StyleModifier.Disabled : hovered ? StyleModifier.Hover : StyleModifier.Default;
		connectButton.BackgroundColor3 = themeColor(
			primary ? StyleColor.DialogMainButton : StyleColor.DialogButton,
			modifier,
		);
		connectButton.TextColor3 = themeColor(
			primary ? StyleColor.DialogMainButtonText : StyleColor.DialogButtonText,
			modifier,
		);
		// There is no DialogMainButtonBorder in the style guide, so only the
		// neutral button gets a stroke.
		buttonStroke.Enabled = !primary;
		buttonStroke.Color = themeColor(StyleColor.DialogButtonBorder, modifier);
	}

	function applyStatus(): void {
		if (status === "connected") {
			dot.BackgroundColor3 = CONNECTED_GREEN;
			statusText.Text = "Connected";
		} else if (status === "connecting") {
			dot.BackgroundColor3 = BUSY_AMBER;
			statusText.Text = "Connecting";
		} else if (status === "reconnecting") {
			dot.BackgroundColor3 = BUSY_AMBER;
			statusText.Text = "Reconnecting";
		} else {
			dot.BackgroundColor3 = themeColor(StyleColor.DimmedText);
			statusText.Text = "Disconnected";
		}
		placeValue.Text = game.Name;
		messageLabel.Text = message ?? "";
		messageLabel.Visible = message !== undefined;
		messageLabel.TextColor3 = themeColor(status === "reconnecting" ? StyleColor.WarningText : StyleColor.ErrorText);
		connectButton.Text =
			status === "disconnected" ? "Connect" : status === "connecting" ? "Connecting..." : "Disconnect";
		applyButtonTheme();
	}

	function applyTheme(): void {
		root.BackgroundColor3 = themeColor(StyleColor.MainBackground);
		wordmark.TextColor3 = themeColor(StyleColor.MainText);
		version.TextColor3 = themeColor(StyleColor.DimmedText);
		separator.BackgroundColor3 = themeColor(StyleColor.Border);
		statusText.TextColor3 = themeColor(StyleColor.MainText);
		for (const caption of captionLabels) caption.TextColor3 = themeColor(StyleColor.DimmedText);
		for (const value of [serverValue, placeValue, activityValue])
			value.TextColor3 = themeColor(StyleColor.MainText);
		applyStatus();
	}

	connectButton.MouseEnter.Connect(() => {
		hovered = true;
		applyButtonTheme();
	});
	connectButton.MouseLeave.Connect(() => {
		hovered = false;
		applyButtonTheme();
	});
	connectButton.MouseButton1Click.Connect(() => {
		if (status === "connecting") return;
		onToggleConnection();
	});

	settings().Studio.ThemeChanged.Connect(applyTheme);
	applyTheme();

	return {
		setStatus(newStatus: ConnectionStatus, newMessage?: string): void {
			if (newStatus === status && newMessage === message) return;
			status = newStatus;
			message = newMessage;
			applyStatus();
		},
		noteCommand(commandType: string): void {
			commandCount += 1;
			const noun = commandCount === 1 ? "command" : "commands";
			activityValue.Text = `${commandCount} ${noun}, last ${commandType}`;
		},
		toggle(): void {
			widget.Enabled = !widget.Enabled;
		},
		isVisible(): boolean {
			return widget.Enabled;
		},
		onVisibleChanged(callback: (visible: boolean) => void): void {
			widget.GetPropertyChangedSignal("Enabled").Connect(() => callback(widget.Enabled));
		},
	};
}
