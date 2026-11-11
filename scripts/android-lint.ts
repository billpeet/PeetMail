// Clippy and a test build for Android (aarch64), so the `cfg(target_os = "android")` and
// `cfg(mobile)` arms are checked on any PC, as `pnpm lint:rust` checks the native ones.
// Points cargo at the NDK's clang for the C dependencies (ring, SQLite), as `tauri android
// build` does. Needs the NDK (NDK_HOME, or the newest one under ANDROID_HOME/ndk) and
// `rustup target add aarch64-linux-android`.
//
//   pnpm lint:android

import { spawnSync } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const TARGET = "aarch64-linux-android";
/** The API level the C code is compiled for; Tauri's default. The app's minSdk (37) is in gradle. */
export const API = 24;

/** The NDK folder: an explicit variable wins, else the newest NDK in the SDK. */
export function findNdk(env: NodeJS.ProcessEnv, list: (dir: string) => string[] = safeList): string | undefined {
  const direct = env.NDK_HOME ?? env.ANDROID_NDK_HOME ?? env.ANDROID_NDK_LATEST_HOME;
  if (direct) return direct;
  const sdk = env.ANDROID_HOME ?? env.ANDROID_SDK_ROOT;
  if (!sdk) return undefined;
  const versions = list(join(sdk, "ndk")).sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
  return versions.length ? join(sdk, "ndk", versions[versions.length - 1]) : undefined;
}

/** The variables cargo and the `cc` crate read to cross-compile for `TARGET`. */
export function crossEnv(ndk: string, platform: NodeJS.Platform): Record<string, string> {
  const host = platform === "win32" ? "windows-x86_64" : platform === "darwin" ? "darwin-x86_64" : "linux-x86_64";
  const bin = join(ndk, "toolchains", "llvm", "prebuilt", host, "bin");
  const clang = join(bin, `${TARGET}${API}-clang${platform === "win32" ? ".cmd" : ""}`);
  const key = TARGET.replaceAll("-", "_");
  return {
    [`CC_${key}`]: clang,
    [`AR_${key}`]: join(bin, `llvm-ar${platform === "win32" ? ".exe" : ""}`),
    [`CARGO_TARGET_${key.toUpperCase()}_LINKER`]: clang,
  };
}

function safeList(dir: string): string[] {
  return existsSync(dir) ? readdirSync(dir) : [];
}

function cargo(args: string[], env: NodeJS.ProcessEnv) {
  const r = spawnSync("cargo", args, { stdio: "inherit", env });
  if (r.status !== 0) process.exit(r.status ?? 1);
}

function main() {
  const ndk = findNdk(process.env);
  if (!ndk || !existsSync(ndk)) {
    console.error("No Android NDK found: set NDK_HOME, or ANDROID_HOME to an SDK with the NDK installed.");
    process.exit(1);
  }
  const env = { ...process.env, ...crossEnv(ndk, process.platform) };
  const manifest = ["--manifest-path", "src-tauri/Cargo.toml", "--target", TARGET, "--locked"];
  cargo(["clippy", ...manifest, "--all-targets", "--", "-D", "warnings"], env);
  // The tests can't run here (they'd need a device), but they must compile.
  cargo(["test", ...manifest, "--no-run"], env);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
