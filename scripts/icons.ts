// Regenerates src-tauri/icons from the sources in src-tauri/icons/source:
//
//   peetmail.svg         the master (1024 grid), for every size from 48 px up
//   peetmail-16/24/32.svg  hand-tuned small sizes, drawn on their pixel grid
//
// `tauri icon` makes the full set from the master; this keeps only the files the repo uses,
// swaps the hand-tuned sizes into icon.ico, and writes 32x32.png and tray.png from them.
// The Android launcher icons go into src-tauri/gen/android: an adaptive icon whose
// foreground is the P alone (peetmail-android.svg, inside the launcher's safe zone) on a
// light background, plus a monochrome layer for themed icons (peetmail-android-mono.svg).
// `tauri icon` would put the whole rounded tile on white, which a round mask clips badly.
// The small SVGs are rendered with Edge (installed with Windows) through Playwright.
//
//   pnpm icons

import { spawnSync } from "node:child_process";
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "@playwright/test";
import { readIco, replaceSizes, writeIco } from "./ico.ts";

const ICONS = "src-tauri/icons";
const SOURCE = `${ICONS}/source`;
const HAND_TUNED = [16, 24, 32];
const ANDROID_RES = "src-tauri/gen/android/app/src/main/res";
/** Adaptive icon layers are 108 dp square. */
const ANDROID_DENSITIES: Record<string, number> = { mdpi: 1, hdpi: 1.5, xhdpi: 2, xxhdpi: 3, xxxhdpi: 4 };
const ADAPTIVE_ICON = `<?xml version="1.0" encoding="utf-8"?>
<!-- Written by scripts/icons.ts (pnpm icons). -->
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
  <background android:drawable="@drawable/ic_launcher_background"/>
  <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
  <monochrome android:drawable="@mipmap/ic_launcher_monochrome"/>
</adaptive-icon>
`;
/** White, full bleed (the launcher's mask shapes it): see peetmail-android.svg for why. */
const ADAPTIVE_BACKGROUND = `<?xml version="1.0" encoding="utf-8"?>
<!-- Written by scripts/icons.ts (pnpm icons). -->
<shape xmlns:android="http://schemas.android.com/apk/res/android" android:shape="rectangle">
  <solid android:color="#ffffff"/>
</shape>
`;

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
  if (existsSync(ANDROID_RES)) {
    const layers = [
      ["ic_launcher_foreground", "peetmail-android.svg"],
      ["ic_launcher_monochrome", "peetmail-android-mono.svg"],
    ];
    for (const [name, file] of layers) {
      const svg = readFileSync(`${SOURCE}/${file}`).toString("base64");
      for (const [density, scale] of Object.entries(ANDROID_DENSITIES)) {
        const size = Math.round(108 * scale);
        await page.setContent(`<body style="margin:0"><img id="i" src="data:image/svg+xml;base64,${svg}" width="${size}" height="${size}" style="display:block"></body>`);
        mkdirSync(`${ANDROID_RES}/mipmap-${density}`, { recursive: true });
        writeFileSync(`${ANDROID_RES}/mipmap-${density}/${name}.png`, await page.locator("#i").screenshot({ omitBackground: true }));
      }
    }
    mkdirSync(`${ANDROID_RES}/drawable`, { recursive: true });
    writeFileSync(`${ANDROID_RES}/drawable/ic_launcher_background.xml`, ADAPTIVE_BACKGROUND);
    writeFileSync(`${ANDROID_RES}/mipmap-anydpi-v26/ic_launcher.xml`, ADAPTIVE_ICON);
    // tauri icon's color resource of the same name would clash with the drawable.
    rmSync(`${ANDROID_RES}/values/ic_launcher_background.xml`, { force: true });
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
