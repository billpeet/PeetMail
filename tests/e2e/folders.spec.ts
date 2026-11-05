// Mailboxes beyond the inbox: Done, Sent, All mail, Junk and Trash, from the rail and g keys,
// and what e, # and ! do in each.
import { test, expect, folderConversations, row, selectedRow, snap } from "./fixtures";
import type { Page } from "@playwright/test";

type Fake = Parameters<Parameters<typeof test>[2]>[0]["fake"];
const folderOf = async (fake: Fake, id: string) => (await fake.control(`message?id=${id}`)).parentFolderId as string;

async function goTo(page: Page, keys: string, mailbox: string) {
  for (const k of keys.split(" ")) await page.keyboard.press(k);
  await expect(page.getByTestId("mailbox-title")).toHaveAttribute("data-mailbox", mailbox);
  await expect(page.getByTestId(`rail-mailbox-${mailbox}`)).toHaveAttribute("data-active", "true");
}

test("g e, g t, g a and g # show Done, Sent, All mail and Trash; g i goes back", async ({ inbox: page }, testInfo) => {
  await expect(page.getByTestId("rail-mailbox-inbox")).toHaveAttribute("data-active", "true");
  await expect(page.getByTestId("rail-unread")).toBeVisible();

  await goTo(page, "g e", "done");
  await expect(page.getByTestId("thread-row")).toHaveCount(1);
  await expect(row(page, "conv-archived")).toBeVisible();
  await expect(page.getByTestId("splits")).toHaveCount(0); // splits are the inbox's
  await snap(page, testInfo, "done");

  await goTo(page, "g t", "sent");
  await expect(row(page, "conv-sent-0")).toBeVisible();
  await expect(row(page, "conv-plain")).toHaveCount(0);

  await goTo(page, "g a", "all");
  for (const c of ["conv-archived", "conv-plain", "conv-sent-0"]) await expect(row(page, c)).toHaveCount(1);
  await expect(row(page, "conv-deleted")).toHaveCount(0);
  await expect(row(page, "conv-junk")).toHaveCount(0);

  await goTo(page, "g #", "trash");
  await expect(page.getByTestId("thread-row")).toHaveCount(1);
  // The reader shows deleted mail here.
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("message")).toHaveCount(1);
  await page.keyboard.press("Escape");

  await page.keyboard.press("g");
  await page.keyboard.press("i");
  await expect(page.getByTestId("splits")).toBeVisible();
  await expect(row(page, "conv-plain")).toBeVisible();
  await expect(page.getByTestId("rail-mailbox-inbox")).toHaveAttribute("data-active", "true");
});

test("the rail opens each mailbox", async ({ inbox: page }) => {
  await page.getByTestId("rail-mailbox-sent").click();
  await expect(page.getByTestId("mailbox-title")).toHaveAttribute("data-mailbox", "sent");
  await page.getByTestId("rail-calendar").click();
  await expect(page.getByTestId("week-view")).toBeVisible();
  await page.getByTestId("rail-mailbox-done").click();
  await expect(row(page, "conv-archived")).toBeVisible();
  await page.getByTestId("rail-mailbox-snoozed").click();
  await expect(page.getByTestId("mailbox-title")).toHaveAttribute("data-mailbox", "snoozed");
  await page.getByTestId("rail-mailbox-scheduled").click();
  await expect(page.getByTestId("scheduled-empty")).toBeVisible();
});

test("e in Done brings a conversation back to the inbox, and z takes it back", async ({ inbox: page, fake }) => {
  await goTo(page, "g e", "done");
  await expect(selectedRow(page)).toHaveAttribute("data-conversation-id", "conv-archived");
  await page.keyboard.press("Enter"); // the reader's toolbar says what e does here
  await expect(page.getByTestId("reader-archive")).toHaveAttribute("title", "Move to Inbox (E)");
  await page.keyboard.press("e");
  await expect(page.getByTestId("toast")).toContainText("Moved to Inbox");
  await expect(row(page, "conv-archived")).toHaveCount(0);
  await expect(page.getByTestId("thread-list-empty")).toHaveText("Nothing archived yet.");
  await expect.poll(() => folderOf(fake, "msg-archived"), { timeout: 10_000 }).toBe("AAMkAGI2-inbox");

  await page.keyboard.press("z");
  await expect(row(page, "conv-archived")).toBeVisible();
  await expect.poll(() => folderOf(fake, "msg-archived"), { timeout: 10_000 }).toBe("AAMkAGI2-archive");
});

test("# in Done deletes; in Trash, # restores to the inbox", async ({ inbox: page, fake }) => {
  await goTo(page, "g e", "done");
  await expect(selectedRow(page)).toHaveAttribute("data-conversation-id", "conv-archived");
  await page.keyboard.press("#");
  await expect(page.getByTestId("toast")).toContainText("Deleted");
  await expect.poll(() => folderOf(fake, "msg-archived"), { timeout: 10_000 }).toBe("AAMkAGI2-deleted");

  await goTo(page, "g #", "trash");
  await expect(row(page, "conv-archived")).toBeVisible();
  await expect(row(page, "conv-deleted")).toBeVisible();
  await row(page, "conv-archived").click();
  await expect(page.getByTestId("reader-trash")).toHaveAttribute("title", "Restore to Inbox (#)");
  await page.keyboard.press("#");
  await expect(page.getByTestId("toast")).toContainText("Moved to Inbox");
  await expect.poll(() => folderOf(fake, "msg-archived"), { timeout: 10_000 }).toBe("AAMkAGI2-inbox");
  expect(await folderConversations(page, "inbox")).toContain("conv-archived");
  expect(await folderConversations(page, "deleteditems")).not.toContain("conv-archived");
});

test("! marks spam; in Junk, ! (or e) says not spam; z undoes both", async ({ inbox: page, fake }) => {
  // Junk mail is synced and listed in Junk, and only there.
  await goTo(page, "g !", "junk");
  await expect(page.getByTestId("thread-row")).toHaveCount(1);
  await expect(row(page, "conv-junk")).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("message")).toHaveCount(1); // the reader shows junk mail here
  await expect(page.getByTestId("reader-spam")).toHaveAttribute("title", "Not spam (!)");
  await expect(page.getByTestId("reader-archive")).toHaveAttribute("title", "Move to Inbox (E)");
  await page.keyboard.press("Escape");

  // !: not spam, back to the inbox.
  await page.keyboard.press("!");
  await expect(page.getByTestId("toast")).toContainText("Not spam: moved to Inbox");
  await expect(page.getByTestId("thread-list-empty")).toHaveText("No junk.");
  await expect.poll(() => folderOf(fake, "msg-junk"), { timeout: 10_000 }).toBe("AAMkAGI2-inbox");
  await page.keyboard.press("z");
  await expect(row(page, "conv-junk")).toBeVisible();
  await expect.poll(() => folderOf(fake, "msg-junk"), { timeout: 10_000 }).toBe("AAMkAGI2-junk");

  // e in Junk is not spam too.
  await page.keyboard.press("e");
  await expect(row(page, "conv-junk")).toHaveCount(0);
  await expect.poll(() => folderOf(fake, "msg-junk"), { timeout: 10_000 }).toBe("AAMkAGI2-inbox");

  // ! in the inbox: spam, out of the inbox and into Junk.
  await page.keyboard.press("g");
  await page.keyboard.press("i");
  await expect(page.getByTestId("splits")).toBeVisible();
  await row(page, "conv-plain").click();
  await expect(page.getByTestId("reader-spam")).toHaveAttribute("title", "Spam (!)");
  await page.keyboard.press("!");
  await expect(page.getByTestId("toast")).toContainText("Marked as spam");
  await page.keyboard.press("Escape");
  await expect(row(page, "conv-plain")).toHaveCount(0);
  await expect.poll(() => folderOf(fake, "msg-plain"), { timeout: 10_000 }).toBe("AAMkAGI2-junk");
  expect(await folderConversations(page, "junkemail")).toContain("conv-plain");
  await page.keyboard.press("z");
  await expect(row(page, "conv-plain")).toBeVisible();
  await expect.poll(() => folderOf(fake, "msg-plain"), { timeout: 10_000 }).toBe("AAMkAGI2-inbox");
});

test("mail the server files as junk leaves the inbox for Junk at the next sync", async ({ inbox: page, fake }) => {
  await expect(row(page, "conv-plain")).toBeVisible();
  await fake.control("move", { id: "msg-plain", folder: "junkemail" });
  await page.keyboard.press("Control+r");
  await expect(row(page, "conv-plain")).toHaveCount(0, { timeout: 15_000 });
  await page.getByTestId("rail-mailbox-junk").click();
  await expect(row(page, "conv-plain")).toBeVisible();
});

test("a key pressed while a mailbox loads never acts on the one you left", async ({ inbox: page, fake }) => {
  await page.keyboard.press("g");
  await page.keyboard.press("e");
  await page.keyboard.press("#"); // straight away, no waiting for Done's list
  await page.waitForTimeout(3000); // past the outbox's grace period
  expect(await folderOf(fake, "msg-hostile")).toBe("AAMkAGI2-inbox");
});

test("Tab cycles split tabs only in the inbox", async ({ inbox: page }) => {
  await goTo(page, "g t", "sent");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("mailbox-title")).toHaveAttribute("data-mailbox", "sent");
});
