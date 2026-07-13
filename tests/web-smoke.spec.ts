import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { expect, test, type Page } from "@playwright/test";

const TINY_PNG_BASE64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+X2ioAAAAASUVORK5CYII=";
const TINY_WEBM_BASE64 =
  "GkXfo59ChoEBQveBAULygQRC84EIQoKEd2VibUKHgQJChYECGFOAZwEAAAAAAALaEU2bdLpNu4tTq4QVSalmU6yBoU27i1OrhBZUrmtTrIHWTbuMU6uEElTDZ1OsggEjTbuMU6uEHFO7a1OsggLE7AEAAAAAAABZAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAVSalmsCrXsYMPQkBNgIxMYXZmNjIuMy4xMDFXQYxMYXZmNjIuMy4xMDFEiYhAeQAAAAAAABZUrmvIrgEAAAAAAAA/14EBc8WIXDMlQcL5ey2cgQAitZyDdW5kiIEAhoVWX1ZQOIOBASPjg4QCYloA4JCwgTC6gTCagQJVsIRVuYEBElTDZ/tzc59jwIBnyJlFo4dFTkNPREVSRIeMTGF2ZjYyLjMuMTAxc3PWY8CLY8WIXDMlQcL5ey1nyKFFo4dFTkNPREVSRIeUTGF2YzYyLjExLjEwMCBsaWJ2cHhnyKFFo4hEVVJBVElPTkSHkzAwOjAwOjAwLjQwMDAwMDAwMAAfQ7Z1QRvngQCjv4EAAIAwAwCdASowADAAAEcIhYWIhYSIAgICdaoD+AIHNnNh5y3IAP7wsDf/vdP97p/vdP/3un/+bArmufc0AKOWgQAoANEBAAEQMAAYABhYL/QACJAAAKOWgQBQANEBAAEQMAAYABhYL/QACJAAAKOWgQB4ANEBAAEQMAAYABhYL/QACJAAAKOWgQCgANEBAAEQMAAYABhYL/QACJAAAKOWgQDIANEBAAEQMAAYABhYL/QACJAAAKOWgQDwANEBAAEQMAAYABhYL/QACJAAAKOVgQEYALEBAAEQEBRgAGFgv9AAIkAAo5aBAUAA0QEAARAwABgAGFgv9AAIkAAAo5aBAWgA0QEAARAwABgAGFgv9AAIkAAAHFO7a5G7j7OBALeK94EB8YIBo/CBAw==";

async function chooseInputFile(
  page: Page,
  file: { name: string; mimeType: string; buffer: Buffer } | string,
) {
  const fileChooserPromise = page.waitForEvent("filechooser");
  await page.getByRole("button", { name: "Choose file" }).click();
  const fileChooser = await fileChooserPromise;
  await fileChooser.setFiles(file);
}

async function createSmokeVideoPath() {
  const tempDir = await mkdtemp(path.join(tmpdir(), "stickerfit-web-smoke-"));
  const outputPath = path.join(tempDir, "smoke.webm");
  const buffer = Buffer.from(TINY_WEBM_BASE64, "base64");
  await import("node:fs/promises").then(({ writeFile }) => writeFile(outputPath, buffer));

  return outputPath;
}

async function readPreviewVideoTime(page: Page) {
  return page
    .locator("video")
    .evaluate((video) => (video as HTMLVideoElement).currentTime);
}

test("renders the web preview start screen", async ({ page }) => {
  await page.goto("/");

  await expect(
    page.getByRole("heading", { name: "Discord sticker converter" }),
  ).toBeVisible();
  await expect(
    page.getByText("Web mode supports file inspection, crop, zoom, and timeline review."),
  ).toBeVisible();
});

test("loads a still image in web preview mode", async ({ page }) => {
  await page.goto("/");

  await chooseInputFile(page, {
    name: "smoke.png",
    mimeType: "image/png",
    buffer: Buffer.from(TINY_PNG_BASE64, "base64"),
  });

  await expect(page.getByRole("button", { name: "Convert to PNG" })).toBeVisible();
  await expect(page.getByText("Frame rate")).toHaveCount(0);

  const hasPageScroll = await page.evaluate(
    () => document.documentElement.scrollHeight > window.innerHeight,
  );
  expect(hasPageScroll).toBe(false);
});

test("does not expose a redundant image fitting mode", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");

  await chooseInputFile(page, {
    name: "fit-mode-smoke.png",
    mimeType: "image/png",
    buffer: Buffer.from(TINY_PNG_BASE64, "base64"),
  });

  await expect(page.getByRole("button", { name: "Convert to PNG" })).toBeVisible();
  await expect(page.getByLabel("Image fitting")).toHaveCount(0);

  await page.getByRole("button", { name: "KO" }).click();
  await expect(page.locator("html")).toHaveAttribute("lang", "ko");
  await expect(
    page.getByRole("heading", { name: "디스코드용 스티커 컨버터" }),
  ).toBeVisible();
  await expect(page.getByLabel("이미지 맞춤 방식")).toHaveCount(0);
});

test("opens and closes the advanced settings overlay for video preview", async ({
  page,
}) => {
  await page.goto("/");

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const settingsToggle = page.getByRole("button", { name: "Show advanced settings" });
    const overlayDialog = page.getByRole("dialog");
    await expect(settingsToggle).toBeVisible();
    await settingsToggle.click();
    await expect(
      overlayDialog.getByRole("heading", { name: "Advanced settings" }),
    ).toBeVisible();
    await expect(overlayDialog.locator("[data-dialog-initial-focus]")).toBeFocused();

    const closeButton = overlayDialog.getByRole("button", { name: "Close panel" });
    const lastSelect = overlayDialog.locator("select").last();
    await page.keyboard.press("Shift+Tab");
    await expect(lastSelect).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(closeButton).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(lastSelect).toBeFocused();

    const duplicateIds = await page.evaluate(() => {
      const counts = new Map<string, number>();
      for (const element of document.querySelectorAll<HTMLElement>("[id]")) {
        counts.set(element.id, (counts.get(element.id) ?? 0) + 1);
      }
      return [...counts.entries()].filter(([, count]) => count > 1);
    });
    expect(duplicateIds).toEqual([]);

    await page.keyboard.press("Escape");
    await expect(overlayDialog).toHaveCount(0);
    await expect(settingsToggle).toBeFocused();
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("supports keyboard timeline and crop movement without global shortcut leakage", async ({
  page,
}) => {
  await page.goto("/");

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const timeline = page.getByRole("slider", { name: "Timeline & frame controls" });
    await expect(timeline).toHaveAttribute("aria-valuenow", "0");
    await timeline.press("ArrowRight");
    await expect.poll(async () => Number(await timeline.getAttribute("aria-valuenow")))
      .toBeGreaterThan(0);
    await timeline.press("End");
    const maximumTimeUs = Number(await timeline.getAttribute("aria-valuemax"));
    await expect(timeline).toHaveAttribute("aria-valuenow", String(maximumTimeUs));
    await timeline.press("PageDown");
    await expect(timeline).toHaveAttribute(
      "aria-valuenow",
      String(Math.max(0, maximumTimeUs - 1_000_000)),
    );
    await timeline.press("PageUp");
    await expect(timeline).toHaveAttribute(
      "aria-valuenow",
      String(maximumTimeUs),
    );
    await timeline.press("Home");
    await expect(timeline).toHaveAttribute("aria-valuenow", "0");
    await timeline.press("ArrowUp");
    await expect.poll(async () => Number(await timeline.getAttribute("aria-valuenow")))
      .toBeGreaterThan(0);
    await timeline.press("ArrowDown");
    await expect(timeline).toHaveAttribute("aria-valuenow", "0");

    const selectionBox = page.locator(".selectionBox");
    const topRightHandle = page.getByRole("button", {
      name: "Resize selection from the top right",
    });
    await expect(selectionBox).toHaveAttribute("tabindex", "0");
    await expect(topRightHandle).toHaveAttribute("tabindex", "0");
    const initialHandleBox = await topRightHandle.boundingBox();
    expect(initialHandleBox).not.toBeNull();
    await topRightHandle.press("Shift+ArrowLeft");
    const resizedHandleBox = await topRightHandle.boundingBox();
    expect(resizedHandleBox).not.toBeNull();
    expect(resizedHandleBox!.x).toBeLessThan(initialHandleBox!.x);

    const beforeMove = await selectionBox.boundingBox();
    const liveSummary = selectionBox.locator("[role='status']");
    const beforeMoveSummary = await liveSummary.textContent();
    expect(beforeMove).not.toBeNull();
    await selectionBox.press("ArrowRight");
    const afterMove = await selectionBox.boundingBox();
    expect(afterMove).not.toBeNull();
    expect(afterMove!.x).toBeGreaterThan(beforeMove!.x);
    await expect(liveSummary).not.toHaveText(beforeMoveSummary ?? "");

    await page.getByLabel("Crop ratio").selectOption("1:1");
    const lockedHandleBox = await topRightHandle.boundingBox();
    expect(lockedHandleBox).not.toBeNull();
    await topRightHandle.press("ArrowRight");
    const expandedLockedHandleBox = await topRightHandle.boundingBox();
    expect(expandedLockedHandleBox).not.toBeNull();
    expect(expandedLockedHandleBox!.x).toBeGreaterThan(lockedHandleBox!.x);

    await selectionBox.press("ArrowLeft");
    const cropFocusStyle = await selectionBox.evaluate((element) => {
      const style = getComputedStyle(element);
      return { outlineStyle: style.outlineStyle, outlineOffset: style.outlineOffset };
    });
    expect(cropFocusStyle.outlineStyle).not.toBe("none");
    expect(Number.parseFloat(cropFocusStyle.outlineOffset)).toBeLessThan(0);
    await expect(page.getByRole("button", { name: "Play" })).toBeVisible();
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("uses menu semantics, roving focus, and Escape opener restoration", async ({ page }) => {
  await page.goto("/");

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const frame2 = page.locator('[data-instance-id="frame-2"]');
    await frame2.click({ button: "right" });
    const menu = page.getByRole("menu");
    const menuItems = menu.getByRole("menuitem");
    await expect(menu).toBeVisible();
    await expect(menuItems.first()).toBeFocused();
    await menuItems.first().press("End");
    await expect(menuItems.last()).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(menu).toHaveCount(0);
    await expect(frame2).toBeFocused();

    await frame2.click({ button: "right" });
    await expect(menuItems.first()).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(menu).toHaveCount(0);
    await expect(frame2).toBeFocused();

    const frame1 = page.locator('[data-instance-id="frame-1"]');
    const frame5 = page.locator('[data-instance-id="frame-5"]');
    await frame1.click();
    await frame5.click({ modifiers: ["Shift"] });
    await frame2.click({ button: "right" });
    await menu.getByRole("menuitem", { name: "Cut", exact: true }).click();
    await expect(page.locator("[data-instance-id]")).toHaveCount(0);
    await expect(page.locator(".frameRailEmptyState button:not(:disabled)")).toBeFocused();
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("bounds a synthetic 240-frame rail and keeps one roving tab stop", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(HTMLMediaElement.prototype, "duration", {
      configurable: true,
      get: () => 20,
    });
  });
  await page.goto("/");
  await page.addStyleTag({
    content: ".frameTableBody { max-height: 384px !important; }",
  });

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const frameTableBody = page.locator(".frameTableBody");
    const options = frameTableBody.getByRole("option");
    await expect(options.first()).toHaveAttribute("aria-setsize", "240");
    const initialMetrics = await frameTableBody.evaluate((element) => ({
      clientHeight: element.clientHeight,
      optionCount: element.querySelectorAll("[role='option']").length,
      activeCount: element.querySelectorAll("[role='option'][tabindex='0']").length,
    }));
    expect(initialMetrics.optionCount).toBeLessThanOrEqual(
      Math.ceil(initialMetrics.clientHeight / 48) + 8,
    );
    expect(initialMetrics.activeCount).toBe(1);

    await options.first().press("End");
    const lastFrame = page.locator('[data-instance-id="frame-240"]');
    await expect(lastFrame).toBeFocused();
    await expect(lastFrame).toHaveAttribute("tabindex", "0");
    await expect(frameTableBody.locator("[role='option'][tabindex='0']")).toHaveCount(1);
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("supports Explorer-style frame selection and drag reorder", async ({ page }) => {
  await page.goto("/");

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const frameRows = page.locator("[data-instance-id]");
    await expect(frameRows).toHaveCount(5);

    const frame2 = page.locator('[data-instance-id="frame-2"]');
    const frame3 = page.locator('[data-instance-id="frame-3"]');
    const frame4 = page.locator('[data-instance-id="frame-4"]');
    const frame5 = page.locator('[data-instance-id="frame-5"]');

    await frame2.click();
    await expect(frame2).toHaveAttribute("aria-selected", "true");

    await frame4.click({ modifiers: ["Shift"] });
    await expect(frame2).toHaveAttribute("aria-selected", "true");
    await expect(frame3).toHaveAttribute("aria-selected", "true");
    await expect(frame4).toHaveAttribute("aria-selected", "true");
    await expect(page.locator("[data-preview-frame-canvas='true']")).toBeVisible();
    await expect.poll(() => readPreviewVideoTime(page)).toBeGreaterThan(0.1);
    const frame4PreviewTime = await readPreviewVideoTime(page);

    await frame4.click();
    await expect(frame2).toHaveAttribute("aria-selected", "false");
    await expect(frame3).toHaveAttribute("aria-selected", "false");
    await expect(frame4).toHaveAttribute("aria-selected", "true");

    await frame2.click();
    await expect(frame2).toHaveAttribute("aria-selected", "true");
    await expect(frame3).toHaveAttribute("aria-selected", "false");
    await expect(frame4).toHaveAttribute("aria-selected", "false");
    await expect(page.locator("video")).toHaveJSProperty("paused", true);
    await expect.poll(() => readPreviewVideoTime(page)).toBeLessThan(frame4PreviewTime);

    await frame2.click();
    await frame4.click({ modifiers: ["Shift"] });
    await frame3.click({ modifiers: ["Control"] });
    await expect(frame3).toHaveAttribute("aria-selected", "false");
    await expect(frame2).toHaveAttribute("aria-selected", "true");
    await expect(frame4).toHaveAttribute("aria-selected", "true");

    const frame4Box = await frame4.boundingBox();
    const frame5Box = await frame5.boundingBox();
    expect(frame4Box).not.toBeNull();
    expect(frame5Box).not.toBeNull();
    if (!frame4Box || !frame5Box) {
      return;
    }

    await page.mouse.move(frame4Box.x + frame4Box.width / 2, frame4Box.y + frame4Box.height / 2);
    await page.mouse.down();
    await page.mouse.move(
      frame5Box.x + frame5Box.width / 2,
      frame5Box.y + frame5Box.height - 2,
      { steps: 8 },
    );
    await expect(frame5).toHaveClass(/is-drop-below/);
    await page.mouse.up();

    await expect(frame2).toHaveAttribute("aria-selected", "true");
    await expect(frame4).toHaveAttribute("aria-selected", "true");

    await page.getByRole("button", { name: "Play" }).click();
    await expect(page.locator("video")).toHaveJSProperty("paused", true);

    await expect
      .poll(async () =>
        page
          .locator("[data-instance-id]")
          .evaluateAll((rows) =>
            rows.map((row) => (row as HTMLElement).dataset.instanceId),
          ),
      )
      .toEqual(["frame-1", "frame-3", "frame-5", "frame-2", "frame-4"]);
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("drags an unselected frame as a single selected item", async ({ page }) => {
  await page.goto("/");

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const frameRows = page.locator("[data-instance-id]");
    await expect(frameRows).toHaveCount(5);

    const frame2 = page.locator('[data-instance-id="frame-2"]');
    const frame5 = page.locator('[data-instance-id="frame-5"]');

    const frame2Box = await frame2.boundingBox();
    const frame5Box = await frame5.boundingBox();
    expect(frame2Box).not.toBeNull();
    expect(frame5Box).not.toBeNull();
    if (!frame2Box || !frame5Box) {
      return;
    }

    await page.mouse.move(frame2Box.x + frame2Box.width / 2, frame2Box.y + frame2Box.height / 2);
    await page.mouse.down();
    await page.mouse.move(
      frame5Box.x + frame5Box.width / 2,
      frame5Box.y + frame5Box.height - 2,
      { steps: 8 },
    );
    await expect(frame5).toHaveClass(/is-drop-below/);
    await page.mouse.up();

    await expect(frame2).toHaveAttribute("aria-selected", "true");
    await expect
      .poll(async () =>
        page
          .locator("[data-instance-id]")
          .evaluateAll((rows) =>
            rows.map((row) => (row as HTMLElement).dataset.instanceId),
          ),
      )
      .toEqual(["frame-1", "frame-3", "frame-4", "frame-5", "frame-2"]);
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("clamps roving frame navigation and scrolls Home and End into view", async ({
  page,
}) => {
  await page.goto("/");
  await page.addStyleTag({
    content: ".frameTableBody { max-height: 88px !important; }",
  });

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const frameTableBody = page.locator(".frameTableBody");
    const frame1 = page.locator('[data-instance-id="frame-1"]');
    const frame2 = page.locator('[data-instance-id="frame-2"]');
    const frame5 = page.locator('[data-instance-id="frame-5"]');

    await frame5.click();
    await expect(frame5).toHaveAttribute("aria-selected", "true");
    await expect.poll(() => frameTableBody.evaluate((element) => element.scrollTop))
      .toBeGreaterThan(0);

    await frame5.press("ArrowDown");
    await expect(frame5).toHaveAttribute("aria-selected", "true");
    await expect(frame5).toBeFocused();

    await frame5.press("Home");
    await expect(frame1).toHaveAttribute("aria-selected", "true");
    await expect(frame5).toHaveAttribute("aria-selected", "false");
    await expect(frame1).toBeFocused();
    await expect.poll(() => frameTableBody.evaluate((element) => element.scrollTop))
      .toBe(0);

    await frame1.press("ArrowUp");
    await expect(frame1).toHaveAttribute("aria-selected", "true");
    await expect(frame1).toBeFocused();

    await frame1.press("Shift+ArrowDown");
    await expect(frame1).toHaveAttribute("aria-selected", "true");
    await expect(frame2).toHaveAttribute("aria-selected", "true");
    await expect(frame2).toBeFocused();

    await frame2.press("End");
    await expect(frame5).toHaveAttribute("aria-selected", "true");
    await expect(frame1).toHaveAttribute("aria-selected", "false");
    await expect(frame5).toBeFocused();
    await expect.poll(() => frameTableBody.evaluate((element) => element.scrollTop))
      .toBeGreaterThan(0);
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("limits global shortcuts to editor background and ignores interactive controls", async ({
  page,
}) => {
  await page.goto("/");

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const settingsToggle = page.getByRole("button", { name: "Show advanced settings" });
    const overlayDialog = page.getByRole("dialog");
    const frame2 = page.locator('[data-instance-id="frame-2"]');
    const frame3 = page.locator('[data-instance-id="frame-3"]');

    await frame2.click();
    await expect(frame2).toHaveAttribute("aria-selected", "true");

    await settingsToggle.focus();
    await expect(settingsToggle).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Delete");

    await expect(frame2).toHaveAttribute("aria-selected", "true");
    await expect(frame3).toHaveAttribute("aria-selected", "false");

    await page.keyboard.press("Space");
    await expect(overlayDialog).toBeVisible();
    await expect(page.getByRole("button", { name: "Play" })).toBeVisible();

    const settingsSelect = overlayDialog.getByLabel("Optimization goal");
    await settingsSelect.focus();
    await settingsSelect.dispatchEvent("keydown", {
      key: " ",
      code: "Space",
      bubbles: true,
    });
    await expect(page.getByRole("button", { name: "Play" })).toBeVisible();
    await expect(overlayDialog).toBeVisible();

    await overlayDialog.getByRole("button", { name: "Close panel" }).click();
    const editorSurface = page.locator("[data-editor-shortcut-surface]");
    await expect(editorSurface).toHaveAttribute("tabindex", "0");
    await editorSurface.focus();
    await expect(editorSurface).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(frame2).toHaveAttribute("aria-selected", "false");
    await expect(frame3).toHaveAttribute("aria-selected", "true");

    await editorSurface.focus();
    await page.keyboard.press("Delete");
    await expect(frame3).toHaveCount(0);

    await editorSurface.focus();
    await page.keyboard.press("Space");
    await expect(page.getByRole("button", { name: "Pause" })).toBeVisible();
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("renders a visible focus indicator for the active frame option", async ({ page }) => {
  await page.goto("/");

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const frame1 = page.locator('[data-instance-id="frame-1"]');
    for (let step = 0; step < 20; step += 1) {
      if (await frame1.evaluate((element) => document.activeElement === element)) {
        break;
      }
      await page.keyboard.press("Tab");
    }
    await expect(frame1).toBeFocused();
    const focusStyle = await frame1.evaluate((element) => {
      const style = getComputedStyle(element);
      return { outlineStyle: style.outlineStyle, outlineWidth: style.outlineWidth };
    });

    expect(focusStyle.outlineStyle).not.toBe("none");
    expect(focusStyle.outlineWidth).not.toBe("0px");
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("tracks playback in the frame rail with a single active selection", async ({
  page,
}) => {
  await page.goto("/");
  await page.addStyleTag({
    content: ".frameTableBody { max-height: 88px !important; }",
  });

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const frameTableBody = page.locator(".frameTableBody");
    const frame1 = page.locator('[data-instance-id="frame-1"]');

    await frame1.click();
    await expect(frame1).toHaveAttribute("aria-selected", "true");
    await page.getByRole("button", { name: "Play" }).click();

    await expect
      .poll(
        async () =>
          page
            .locator("[data-instance-id][aria-selected='true']")
            .evaluateAll((rows) =>
              rows.map((row) => (row as HTMLElement).dataset.instanceId),
            ),
        { intervals: [20, 20, 20, 40, 40, 80, 120], timeout: 1000 },
      )
      .not.toEqual(["frame-1"]);

    await expect
      .poll(async () =>
        page.evaluate(() => {
          const activeElement = document.activeElement;
          return activeElement instanceof HTMLElement &&
            activeElement.matches(".frameRow[aria-selected='false']");
        }),
      )
      .toBe(false);

    await expect(page.locator(".frameRow.is-current")).toHaveCount(0);
    await expect(page.locator(".frameDot")).toHaveCount(0);
    await expect.poll(() => frameTableBody.evaluate((element) => element.scrollTop))
      .toBeGreaterThan(0);
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("keeps the selected group intact when dragging from an all-selected frame", async ({ page }) => {
  await page.goto("/");

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const frameRows = page.locator("[data-instance-id]");
    await expect(frameRows).toHaveCount(5);

    const frame1 = page.locator('[data-instance-id="frame-1"]');
    const frame3 = page.locator('[data-instance-id="frame-3"]');
    const frame5 = page.locator('[data-instance-id="frame-5"]');

    await frame1.click();
    await frame5.click({ modifiers: ["Shift"] });
    for (const instanceId of ["frame-1", "frame-2", "frame-3", "frame-4", "frame-5"]) {
      await expect(page.locator(`[data-instance-id="${instanceId}"]`)).toHaveAttribute(
        "aria-selected",
        "true",
      );
    }

    const frame3Box = await frame3.boundingBox();
    expect(frame3Box).not.toBeNull();
    if (!frame3Box) {
      return;
    }

    await page.mouse.move(frame3Box.x + frame3Box.width / 2, frame3Box.y + frame3Box.height / 2);
    await page.mouse.down();
    await page.mouse.move(
      frame3Box.x + frame3Box.width / 2,
      frame3Box.y + frame3Box.height / 2 + 12,
      { steps: 4 },
    );

    for (const instanceId of ["frame-1", "frame-2", "frame-3", "frame-4", "frame-5"]) {
      await expect(page.locator(`[data-instance-id="${instanceId}"]`)).toHaveAttribute(
        "aria-selected",
        "true",
      );
    }
    await page.mouse.up();
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});
