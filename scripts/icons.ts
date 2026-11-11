// Regenerates src-tauri/icons from the sources in src-tauri/icons/source:
//
//   peetmail.svg         the master (1024 grid), for every size from 48 px up
//   peetmail-16/24/32.svg  hand-tuned small sizes, drawn on their pixel grid
//
// `tauri icon` makes the full set from the master; this keeps only the files the repo uses,
// swaps the hand-tuned sizes into icon.ico, and writes 32x32.png and tray.png from them.
// The Android launcher icons (adaptive, every density) go into src-tauri/gen/android.
// The small SVGs are rendered with Edge (installed with Windows) through Playwright.
//
//   pnpm icons

import { spawnSync } from "node:child_process";
import { copyFileSync, cpSync, existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "@playwright/test";
import { readIco, replaceSizes, writeIco } from "./ico.ts";

const ICONS = "src-tauri/icons";
const SOURCE = `${ICONS}/source`;
const HAND_TUNED = [16, 24, 32];
const ANDROID_RES = "src-tauri/gen/android/app/src/main/res";

const out = mkdtempSync(join(tmpdir(), "peetmail-icons-"));
try {
  const r = spawnSync(`pnpm tauri icon "${SOURCE}/peetmail.svg" -o "${out}"`, { stdio: "inherit", shell: true });
  if (r.status !== 0) throw new Error("tauri icon failed");

  // Only refresh the files the app already uses (tauri icon also makes Android, iOS and
  // extra sizes).
  for (const f of readdirSync(out)) {
    if (existsSync(join(ICONS, f)) && f.includes(".")) copyFileSync(join(out, f), join(ICONS, f));
  }
  // Android's launcher icons go straight into the generated project's resources.
  if (existsSync(ANDROID_RES)) cpSync(join(out, "android"), ANDROID_RES, { recursive: true });

  const browser = await chromium.launch({ channel: "msedge" });
  const page = await browser.newPage({ deviceScaleFactor: 1 });
  const small: { size: number; data: Buffer }[] = [];
  for (const size of HAND_TUNED) {
    const svg = readFileSync(`${SOURCE}/peetmail-${size}.svg`).toString("base64");
    await page.setContent(`<body style="margin:0"><img id="i" src="data:image/svg+xml;base64,${svg}" width="${size}" height="${size}" style="display:block"></body>`);
    small.push({ size, data: await page.locator("#i").screenshot({ omitBackground: true }) });
  }
  await browser.close();

  writeFileSync(`${ICONS}/icon.ico`, writeIco(replaceSizes(readIco(readFileSync(`${ICONS}/icon.ico`)), small)));
  const px32 = small.find((s) => s.size === 32)!.data;
  writeFileSync(`${ICONS}/32x32.png`, px32);
  // The tray is drawn at 16-24 px; Windows scales this down from the crisp 32.
  writeFileSync(`${ICONS}/tray.png`, px32);

  const sizes = readIco(readFileSync(`${ICONS}/icon.ico`)).map((i) => i.size);
  console.log(`icons written; icon.ico has ${sizes.join(", ")} px (${HAND_TUNED.join(", ")} hand-tuned)`);
} finally {
  if (existsSync(out)) rmSync(out, { recursive: true, force: true });
}
