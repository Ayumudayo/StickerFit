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
