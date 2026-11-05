import { test, expect, messageBody, row, snap, syncNow, waitForSyncIdle, openSettings, showSettingsPage } from "./fixtures";
import type { Page } from "@playwright/test";
import { GMAIL_ACCOUNT, GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET } from "../harness/fake-gmail.ts";

const DAY = 24 * 3600_000;
const daysAgo = (n: number) => new Date(Date.now() - n * DAY).toISOString().replace(/\.\d{3}Z$/, "Z");

async function search(page: Page, text: string) {
  await page.keyboard.press("/");
  const input = page.getByTestId("search-input");
  await expect(input).toBeFocused();
  await input.fill(text);
  return input;
}

type Fake = Parameters<Parameters<typeof test>[2]>[0]["fake"];

/** A two-message conversation from over a year ago, older than the 90-day cache window. */
async function deliverOldThread(fake: Fake) {
  const first = await fake.control("deliver", {
    subject: "Pension statement 2019", preview: "Your annual pension statement is ready.", body: "<p>Your annual pension statement is ready.</p>",
    from: ["Acme Pensions", "statements@acme-pensions.example"], receivedAt: daysAgo(400), folder: "archive", conversationId: "conv-pension",
  });
  const reply = await fake.control("deliver", {
    subject: "Re: your question", preview: "Thanks, filed.", body: "<p>Thanks, filed.</p>",
    from: ["Pat Lee", "pat@acme.example"], receivedAt: daysAgo(399), folder: "archive", conversationId: "conv-pension",
  });
  return { first: first.ids[0] as string, reply: reply.ids[0] as string };
}

test("Ctrl+Enter finds mail older than the cache window; it opens whole and leaves when the search closes", async ({ inbox: page, fake }, testInfo) => {
  const { first, reply } = await deliverOldThread(fake);
  await syncNow(page, fake);
  await waitForSyncIdle(page);

  // Not cached, so local search finds nothing and offers the server.
  await search(page, "pension");
  await expect(page.getByTestId("thread-list-empty")).toContainText("No matches.");
  await expect(page.getByTestId("server-search")).toHaveAttribute("data-state", "offer");

  const before = fake.state.requests.length;
  await page.keyboard.press("Control+Enter");
  await expect(page.getByTestId("server-search-found")).toHaveText("1 message found on the server");
  await expect(row(page, "conv-pension")).toBeVisible();
  // Only the matching message is a result; the query went as KQL.
  await expect(row(page, "conv-pension")).toContainText("Pension statement 2019");
  expect(fake.requestsSince(before).some((r) => r.path === "/v1.0/me/messages" && decodeURIComponent(r.query).includes('$search="pension"'))).toBe(true);
  await snap(page, testInfo, "server-search-results");

  // A sync meanwhile doesn't prune the results.
  await syncNow(page, fake);
  await waitForSyncIdle(page);
  await expect(row(page, "conv-pension")).toBeVisible();

  // Opening it fetches the rest of the conversation and its bodies.
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("reader")).toHaveAttribute("data-conversation-id", "conv-pension");
  await expect(page.getByTestId("message")).toHaveCount(2);
  await expect(messageBody(page).locator("p")).toContainText("Your annual pension statement is ready.");
  await expect(page.getByTestId("message").nth(1)).toContainText("Thanks, filed.");
  // Opening reads it, on the server too.
  await expect.poll(async () => (await fake.control(`message?id=${first}`)).isRead, { timeout: 10_000 }).toBe(true);
  expect((await fake.control(`message?id=${reply}`)).subject).toBe("Re: your question");

  // Closing the search drops what it fetched: a fresh local search finds nothing again.
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("splits")).toBeVisible();
  await search(page, "pension");
  await expect(page.getByTestId("thread-list-empty")).toContainText("No matches.");
  await expect(page.getByTestId("server-search")).toHaveAttribute("data-state", "offer");
});

test("server results can be triaged, and new search text goes back to local results", async ({ inbox: page, fake }) => {
  const { first } = await deliverOldThread(fake);
  await search(page, "pension");
  await page.getByTestId("server-search-run").click();
  await expect(row(page, "conv-pension")).toBeVisible();

  // # on the result moves it to Deleted Items on the server.
  await page.keyboard.press("#");
  await expect.poll(() => fake.state.sent.some((s) => s.kind === "move" && s.messageId === first && s.destination === "deleteditems"), { timeout: 10_000 }).toBe(true);
  await expect(row(page, "conv-pension")).toHaveCount(0);

  // Typing more searches locally again, with the server on offer for the new words.
  await page.getByTestId("search-input").fill("pension statement");
  await expect(page.getByTestId("server-search")).toHaveAttribute("data-state", "offer");
});

test("a new server search replaces the last one's results", async ({ inbox: page, fake }) => {
  await deliverOldThread(fake);
  await fake.control("deliver", { subject: "Gym membership renewal", preview: "Renew by Friday.", receivedAt: daysAgo(300), folder: "archive", conversationId: "conv-gym" });
  await search(page, "pension");
  await page.keyboard.press("Control+Enter");
  await expect(row(page, "conv-pension")).toBeVisible();

  await page.getByTestId("search-input").fill("renewal");
  await page.keyboard.press("Control+Enter");
  await expect(row(page, "conv-gym")).toBeVisible();
  await expect(page.getByTestId("server-search-found")).toHaveText("1 message found on the server");
  await expect(row(page, "conv-pension")).toHaveCount(0);
});

test("a query with nothing the server can search says so", async ({ inbox: page }) => {
  await search(page, "is:focused");
  await page.keyboard.press("Control+Enter");
  await expect(page.getByTestId("toast")).toContainText("Add words, from:, to: or subject: to search the server.");
});

test.describe("Gmail", () => {
  test.use({ withGmail: true });

  test("searching the server finds Gmail mail from before the cache window", async ({ inbox: page, gmail }) => {
    await openSettings(page, "apps");
    await page.getByTestId("google-client-id").fill(GOOGLE_CLIENT_ID);
    await page.getByTestId("google-client-secret").fill(GOOGLE_CLIENT_SECRET);
    await page.getByTestId("google-client-save").click();
    await showSettingsPage(page, "accounts");
    await page.getByTestId("account-add-google").click();
    await expect(page.locator(`[data-testid=account-row][data-email="${GMAIL_ACCOUNT.email}"]`)).toBeVisible({ timeout: 15_000 });
    await page.keyboard.press("Escape");
    await waitForSyncIdle(page);

    // g-8 is 400 days old: "Long ago" / "Remember this?".
    await search(page, "remember account:gmail");
    await expect(page.getByTestId("server-search")).toHaveAttribute("data-state", "offer");
    const before = gmail.state.requests.length;
    await page.keyboard.press("Control+Enter");
    await expect(row(page, "g-thread-ancient")).toBeVisible();
    await expect(row(page, "g-thread-ancient")).toContainText("Long ago");
    // account:gmail kept the Microsoft account out of it.
    expect(gmail.state.requests.slice(before).some((r) => r.path.endsWith("/messages") && new URLSearchParams(r.query).get("q") === "remember")).toBe(true);

    await page.keyboard.press("Enter");
    await expect(messageBody(page).locator("body")).toContainText("Remember this?");
  });
});
