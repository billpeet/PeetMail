import { describe, expect, it } from "vitest";
import { join } from "node:path";
import { crossEnv, findNdk } from "./android-lint.ts";

describe("android lint", () => {
  it("takes NDK_HOME first, else the newest NDK in the SDK", () => {
    expect(findNdk({ NDK_HOME: "/ndk", ANDROID_HOME: "/sdk" })).toBe("/ndk");
    expect(findNdk({ ANDROID_HOME: "/sdk" }, () => ["27.3.13750724", "29.0.14206865", "28.2.13676358"])).toBe(join("/sdk", "ndk", "29.0.14206865"));
    expect(findNdk({ ANDROID_HOME: "/sdk" }, () => [])).toBeUndefined();
    expect(findNdk({})).toBeUndefined();
  });

  it("points cargo and cc at the NDK's clang for the host", () => {
    const linux = crossEnv("/ndk", "linux");
    expect(linux.CC_aarch64_linux_android).toBe(join("/ndk", "toolchains", "llvm", "prebuilt", "linux-x86_64", "bin", "aarch64-linux-android24-clang"));
    expect(linux.CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER).toBe(linux.CC_aarch64_linux_android);
    const win = crossEnv("C:\ndk", "win32");
    expect(win.CC_aarch64_linux_android.endsWith("aarch64-linux-android24-clang.cmd")).toBe(true);
    expect(win.AR_aarch64_linux_android.endsWith("llvm-ar.exe")).toBe(true);
  });
});
