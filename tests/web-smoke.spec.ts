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

    await overlayDialog.getByRole("button", { name: "Close panel" }).click();
    await expect(overlayDialog).toHaveCount(0);
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

test("wraps keyboard frame navigation and scrolls the selected frame into view", async ({
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
    const frame5 = page.locator('[data-instance-id="frame-5"]');

    await frame5.click();
    await expect(frame5).toHaveAttribute("aria-selected", "true");
    await expect.poll(() => frameTableBody.evaluate((element) => element.scrollTop))
      .toBeGreaterThan(0);

    await frame5.press("ArrowDown");
    await expect(frame1).toHaveAttribute("aria-selected", "true");
    await expect(frame5).toHaveAttribute("aria-selected", "false");
    await expect(frame1).toBeFocused();
    await expect.poll(() => frameTableBody.evaluate((element) => element.scrollTop))
      .toBe(0);

    await frame1.press("ArrowUp");
    await expect(frame5).toHaveAttribute("aria-selected", "true");
    await expect(frame1).toHaveAttribute("aria-selected", "false");
    await expect(frame5).toBeFocused();
    await expect.poll(() => frameTableBody.evaluate((element) => element.scrollTop))
      .toBeGreaterThan(0);
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("uses Space and vertical arrows as global playback and frame shortcuts", async ({
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

    await expect(frame2).toHaveAttribute("aria-selected", "false");
    await expect(frame3).toHaveAttribute("aria-selected", "true");
    await expect(frame3).toBeFocused();

    await settingsToggle.focus();
    await page.keyboard.press("Space");
    await expect(page.getByRole("button", { name: "Pause" })).toBeVisible();
    await expect(overlayDialog).toHaveCount(0);

    await page.keyboard.press("Space");
    await expect(page.getByRole("button", { name: "Play" })).toBeVisible();
    await expect(overlayDialog).toHaveCount(0);
  } finally {
    await rm(path.dirname(videoPath), { force: true, recursive: true });
  }
});

test("does not render a separate frame focus visual state", async ({ page }) => {
  await page.goto("/");

  const videoPath = await createSmokeVideoPath();
  try {
    await chooseInputFile(page, videoPath);

    const visibleFrameFocusRules = await page.evaluate(() =>
      Array.from(document.styleSheets)
        .flatMap((styleSheet) => {
          try {
            return Array.from(styleSheet.cssRules);
          } catch {
            return [];
          }
        })
        .map((rule) => rule.cssText)
        .filter(
          (ruleText) =>
            ruleText.includes(".frameRow:focus") &&
            !ruleText.includes("outline: none"),
        ),
    );

    expect(visibleFrameFocusRules).toEqual([]);
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
    await frame1.focus();
    await expect(frame1).toBeFocused();
    await page.keyboard.press("Space");

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
