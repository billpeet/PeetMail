import { describe, expect, it } from "vitest";
import { autostartLabel, isDesktop, platformOf } from "./platform";

describe("platform", () => {
  it("tells WebKitGTK on Linux from WebView2 on Windows", () => {
    expect(platformOf("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Safari/605.1.15")).toBe("linux");
    expect(platformOf("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0")).toBe("windows");
  });

  it("tells Android from Linux, though its user agent says Linux too", () => {
    const pixel = "Mozilla/5.0 (Linux; Android 17; Pixel 8 Build/CP1A; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/146.0.0.0 Mobile Safari/537.36";
    expect(platformOf(pixel)).toBe("android");
    expect(isDesktop("android")).toBe(false);
    expect(isDesktop("linux") && isDesktop("windows")).toBe(true);
  });

  it("names autostart the way each desktop does", () => {
    expect(autostartLabel("windows").label).toBe("Start with Windows");
    expect(autostartLabel("linux").label).toBe("Start when you log in");
  });
});
