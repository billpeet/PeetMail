import { describe, expect, it } from "vitest";
import { composeFromShare } from "./android.svelte.js";

describe("composeFromShare", () => {
  const share = (subject: string | null, text: string | null) => ({ subject, text, files: [], failed: [] });

  it("puts a shared link's title in the subject and the link in the body", () => {
    expect(composeFromShare(share("A good read", "https://example.com/a"))).toEqual({ subject: "A good read", body: "https://example.com/a" });
  });

  it("doesn't repeat a title the app also put at the start of the text", () => {
    expect(composeFromShare(share("A good read", "A good read\nhttps://example.com/a"))).toEqual({ subject: "A good read", body: "https://example.com/a" });
  });

  it("copes with files alone", () => {
    expect(composeFromShare(share(null, null))).toEqual({ subject: "", body: "" });
  });
});
