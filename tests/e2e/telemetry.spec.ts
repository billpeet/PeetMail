import { test, expect, waitForSyncIdle, openSettings } from "./fixtures";
import { hostname, userInfo } from "node:os";
import type { Page } from "@playwright/test";
import type { FakeOtlp } from "../harness/fake-otlp.ts";

const invoke = (page: Page, cmd: string, args?: Record<string, unknown>) =>
  page.evaluate(([cmd, args]) => (window as any).__TAURI_INTERNALS__.invoke(cmd, args), [cmd, args] as const);

/** Values from the fake mailbox and this session that must never reach telemetry. */
const CANARIES = [
  "boris@peetmail.test", // the account
  "Boris Dragon", // its display name
  "priya@fabrikam.io", // a correspondent
  "Priya Nair",
  "Hannah Okafor",
  "Weird formatting in this one", // subjects
  "Q4 plan attached",
  "beetroot", // a subject word, and the search below
  "Had a look at the numbers", // body text
  "msg-hostile", // provider message ids
  "msg-newsletter",
  "conv-thread-1",
  "canary.recipient@contoso-canary.test", // what the test sends
  "Canary subject 7f3a",
  "canary body 9c1e",
  "test-ingest-token", // the auth header's value must only travel as a header
];

function machineCanaries() {
  const user = userInfo().username;
  return [hostname(), `Users\\${user}`, `Users\\\\${user}`, `Users/${user}`];
}

async function flushed(page: Page, telemetry: FakeOtlp, pred: () => boolean) {
  await expect
    .poll(
      async () => {
        await invoke(page, "debug_flush_telemetry");
        return pred();
      },
      { timeout: 15_000 },
    )
    .toBe(true);
}

test("without an endpoint, usage data is off and says so", async ({ inbox: page }) => {
  await openSettings(page, "privacy");
  await expect(page.getByTestId("usage-data")).toBeDisabled();
  await expect(page.getByTestId("usage-data-off")).toBeVisible();
});

test.describe("with a telemetry collector", () => {
  test.use({ withTelemetry: true, undoSendSecs: 0 });

  test("a session is traced end to end, and nothing personal leaves the PC", async ({ inbox: page, telemetry }) => {
    // Open a conversation and go back.
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("reader")).toBeVisible();
    await page.keyboard.press("Escape");

    // Search.
    await page.keyboard.press("/");
    const all = await page.getByTestId("thread-count").textContent();
    await page.getByTestId("search-input").fill("beetroot from:hannah");
    await expect(page.getByTestId("thread-count")).not.toHaveText(all!);
    // Not just any change: entering search can first list all mail (an empty query) if a
    // refresh lands then. Wait for the query's own results, all from Hannah.
    await expect
      .poll(async () => {
        const rows = await page.getByTestId("thread-row").allInnerTexts();
        return rows.length > 0 && rows.every((r) => r.includes("Hannah"));
      })
      .toBe(true);
    const found = Number((await page.getByTestId("thread-count").textContent())!.split(" ")[0]);
    // And on the server, whose request carries the words in its URL.
    await page.getByTestId("search-input").press("Control+Enter");
    await expect(page.getByTestId("server-search-found")).toBeVisible();
    await page.keyboard.press("Escape");

    // Send.
    await page.keyboard.press("c");
    await page.getByTestId("compose-to").fill("canary.recipient@contoso-canary.test");
    await page.getByTestId("compose-subject").fill("Canary subject 7f3a");
    await page.getByTestId("compose-body").fill("canary body 9c1e");
    await page.keyboard.press("Control+Enter");
    await expect(page.getByTestId("composer")).toHaveCount(0);

    // Archive.
    await page.keyboard.press("e");
    await waitForSyncIdle(page);

    const names = () => new Set(telemetry.spans().map((s) => s.name));
    await flushed(page, telemetry, () =>
      ["sync.run", "sync.account", "sync.folder", "http.request", "search.query", "search.server", "ui.action", "body.fetch"].every((n) => names().has(n)) &&
      telemetry.spans().some((s) => s.name === "outbox.op" && s.attrs["peetmail.op"] === "send"),
    );
    const spans = telemetry.spans();

    // HTTP spans carry the route template, never the URL, and sit inside the sync that made them.
    const delta = spans.find((s) => s.name === "http.request" && String(s.attrs["url.template"]).endsWith("/messages/delta"));
    expect(delta?.attrs).toMatchObject({ "http.request.method": "GET", "peetmail.provider": "microsoft_graph", "http.response.status_code": 200, "peetmail.outcome": "ok" });
    expect(delta!.attrs["url.template"]).toBe("/v1.0/me/mailFolders/{id}/messages/delta");
    const folder = spans.find((s) => s.spanId === delta!.parentSpanId);
    expect(folder?.name).toBe("sync.folder");
    expect(folder?.attrs["peetmail.folder"]).toBe("inbox");
    expect(spans.find((s) => s.spanId === folder!.parentSpanId)?.name).toBe("sync.account");

    // Searches report their shape only.
    expect(spans.filter((s) => s.name === "search.query").map((s) => s.attrs)).toContainEqual({
      "peetmail.search.terms": 1,
      "peetmail.search.operators": 1,
      "peetmail.results": found,
    });
    expect(spans.find((s) => s.name === "search.server")?.attrs).toMatchObject({ "peetmail.search.terms": 1, "peetmail.search.operators": 1, "peetmail.outcome": "ok" });
    expect(spans.find((s) => s.name === "http.request" && s.attrs["url.template"] === "/v1.0/me/messages")).toBeTruthy();
    // Keyboard actions by binding id.
    expect(spans.filter((s) => s.name === "ui.action").map((s) => s.attrs["peetmail.binding"])).toEqual(expect.arrayContaining(["list.open", "triage.archive"]));

    // Who and what is reporting.
    const { installId } = await invoke(page, "privacy");
    expect(spans[0].resource).toMatchObject({ "service.name": "peetmail", "service.namespace": "peetsoftware", "peetmail.install.id": installId, "deployment.environment.name": "dev" });
    expect(String(spans[0].resource["service.instance.id"])).toMatch(new RegExp(`^${installId.slice(0, 16)}-[0-9a-f]{8}-\\d+-`));
    expect(telemetry.requests.every((r) => r.headers.authorization === "Bearer test-ingest-token")).toBe(true);

    // Logs and metrics arrive too; logs carry their trace.
    await flushed(page, telemetry, () => telemetry.logs().length > 0 && telemetry.metrics().some((m) => m.name === "http.request.duration"));
    expect(telemetry.metrics().map((m) => m.name)).toEqual(expect.arrayContaining(["peetmail.outbox.depth", "app.startup.duration"]));
    expect(telemetry.logs().some((l) => l.traceId)).toBe(true);

    const sent = telemetry.allText();
    for (const c of [...CANARIES, ...machineCanaries()]) expect(sent, `"${c}" must not leave the PC`).not.toContain(c);
  });

  test("turning usage data off stops it at once", async ({ inbox: page, telemetry }) => {
    await flushed(page, telemetry, () => telemetry.spans().length > 0);
    await openSettings(page, "privacy");
    await expect(page.getByTestId("usage-data")).toBeChecked();
    await page.getByTestId("usage-data").uncheck();
    await page.keyboard.press("Escape");
    await invoke(page, "debug_flush_telemetry");
    const before = telemetry.spans().length;

    await page.keyboard.press("j");
    await page.keyboard.press("Control+r");
    await waitForSyncIdle(page);
    await invoke(page, "debug_flush_telemetry");
    await page.waitForTimeout(1000);
    expect(telemetry.spans().length).toBe(before);
    expect((await invoke(page, "privacy")).usageData).toBe(false);
  });
});
