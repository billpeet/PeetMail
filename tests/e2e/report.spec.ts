import { test, expect } from "./fixtures";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { inflateRawSync } from "node:zlib";
import type { Page } from "@playwright/test";

/** Things from the signed-in account's mailbox that must never be in diagnostics. */
const PERSONAL = ["boris@peetmail.test", "Boris Dragon", "priya@fabrikam.io", "sam.carter@contoso.com", "Q4 planning doc", "Beetroot supplier"];

/**
 * Every file's text in a zip, by name. Read here from the central directory (no zip library
 * in the tests): extracting with PowerShell's Expand-Archive came back empty on CI's Windows
 * image from late November 2026, and an empty result passes the "nothing personal" checks.
 */
function unzip(zip: Buffer | string): Record<string, string> {
  const buf = typeof zip === "string" ? readFileSync(zip) : zip;
  const end = buf.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  if (end < 0) throw new Error("not a zip: no end of central directory");
  const count = buf.readUInt16LE(end + 10);
  let p = buf.readUInt32LE(end + 16);
  const files: Record<string, string> = {};
  for (let i = 0; i < count; i++) {
    if (buf.readUInt32LE(p) !== 0x02014b50) throw new Error(`zip: bad central directory entry ${i}`);
    const method = buf.readUInt16LE(p + 10);
    const size = buf.readUInt32LE(p + 20);
    const [nameLen, extraLen, commentLen] = [buf.readUInt16LE(p + 28), buf.readUInt16LE(p + 30), buf.readUInt16LE(p + 32)];
    const local = buf.readUInt32LE(p + 42);
    const name = buf.toString("utf8", p + 46, p + 46 + nameLen);
    p += 46 + nameLen + extraLen + commentLen;
    if (name.endsWith("/")) continue;
    if (size === 0xffffffff || local === 0xffffffff) throw new Error(`zip: ${name} needs zip64`);
    const start = local + 30 + buf.readUInt16LE(local + 26) + buf.readUInt16LE(local + 28);
    const data = buf.subarray(start, start + size);
    if (method === 8) files[name] = inflateRawSync(data).toString("utf8");
    else if (method === 0) files[name] = data.toString("utf8");
    else throw new Error(`zip: ${name} uses compression method ${method}`);
  }
  return files;
}

async function openReport(page: Page) {
  await page.keyboard.press("F1");
  await page.getByTestId("help-nav-report").click();
  await expect(page.getByTestId("report")).toBeVisible();
}

function expectNothingPersonal(files: Record<string, string>) {
  const all = Object.values(files).join("\n");
  for (const p of PERSONAL) expect(all, `${p} is in the diagnostics`).not.toContain(p);
}

test("exported diagnostics describe the install without anything personal in them", async ({ inbox: page, app }) => {
  await openReport(page);
  // This build has no DSN: there's no form, only the export.
  await expect(page.getByTestId("feedback-message")).toHaveCount(0);
  await expect(page.getByTestId("report-fallback")).toBeVisible();
  await page.getByTestId("diagnostics-export").click();
  await expect(page.getByTestId("diagnostics-status")).toHaveAttribute("data-kind", "saved");

  const dir = join(app.dataDir, "downloads");
  const zips = readdirSync(dir).filter((f) => /^PeetMail-diagnostics-.*\.zip$/.test(f));
  expect(zips).toHaveLength(1);
  const files = unzip(join(dir, zips[0]));

  expect(Object.keys(files)).toEqual(expect.arrayContaining(["README.txt", "summary.json"]));
  expect(Object.keys(files).some((f) => f.startsWith("logs/"))).toBe(true);
  const summary = JSON.parse(files["summary.json"]);
  expect(summary.app.version).toMatch(/^\d+\.\d+\.\d+/);
  expect(summary.app.installId).toBe(await page.getByTestId("report-install-id").textContent());
  expect(summary.system.os).toMatch(/^Windows/);
  expect(summary.database.integrity).toBe("ok");
  expect(summary.database.accounts).toEqual([{ provider: "microsoft", app: expect.any(String), signedIn: true }]);
  expect(summary.database.rows.messages).toBeGreaterThan(0);
  expectNothingPersonal(files);
});

test.describe("with a fake Sentry", () => {
  test.use({ withSentry: true });

  test("feedback arrives with the install ID and the diagnostics attached", async ({ inbox: page, sentry }) => {
    await openReport(page);
    await expect(page.getByTestId("feedback-send")).toBeDisabled();
    await page.getByTestId("feedback-message").fill("The inbox stopped refreshing after lunch");
    await page.getByTestId("feedback-contact").fill("tester@example.com");
    await page.getByTestId("feedback-send").click();
    await expect(page.getByTestId("feedback-status")).toHaveAttribute("data-kind", "sent");
    await expect(page.getByTestId("feedback-message")).toHaveValue("");

    const installId = await page.getByTestId("report-install-id").textContent();
    const env = sentry.envelopes.find((e) => e.items.some((i) => i.header.type === "feedback"))!;
    expect(env).toBeTruthy();
    const event = JSON.parse(env.items.find((i) => i.header.type === "feedback")!.payload.toString("utf8"));
    expect(event.contexts.feedback.message).toBe("The inbox stopped refreshing after lunch");
    expect(event.contexts.feedback.contact_email).toBe("tester@example.com");
    expect(event.user.id).toBe(installId);
    expect(event.release).toMatch(/^peetmail@/);

    const zip = env.items.find((i) => i.header.type === "attachment")!;
    expect(zip.header.filename).toBe("peetmail-diagnostics.zip");
    expectNothingPersonal(unzip(zip.payload));
  });

  test("unticking diagnostics sends the words alone", async ({ inbox: page, sentry }) => {
    await openReport(page);
    await page.getByTestId("feedback-message").fill("Just a thought");
    await page.getByTestId("feedback-diagnostics").uncheck();
    await page.getByTestId("feedback-message").press("Control+Enter");
    await expect(page.getByTestId("feedback-status")).toHaveAttribute("data-kind", "sent");
    const env = sentry.envelopes.find((e) => e.items.some((i) => i.header.type === "feedback"))!;
    expect(env.items.map((i) => i.header.type)).toEqual(["feedback"]);
  });
});

test.describe("with a feedback address", () => {
  test.use({ feedbackEmail: "dev@peetsoftware.test" });

  test("Prefer email? starts a message to the developers with the version filled in", async ({ inbox: page }) => {
    await openReport(page);
    await page.getByTestId("feedback-email").click();
    await expect(page.getByTestId("help")).toHaveCount(0);
    await expect(page.getByTestId("compose-to")).toHaveValue("dev@peetsoftware.test");
    await expect(page.getByTestId("compose-subject")).toHaveValue("PeetMail feedback");
    await expect(page.getByTestId("compose-body")).toHaveAttribute("data-text", /PeetMail \d+\.\d+\.\d+, install ID [0-9a-f]{32}/);
  });
});

test("Report a problem is in the command palette", async ({ inbox: page }) => {
  await page.keyboard.press("Control+k");
  await page.keyboard.type("Report a problem");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("help")).toHaveAttribute("data-page", "report");
});
