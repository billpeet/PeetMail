import { describe, expect, it } from "vitest";
import { androidVersionCode, canaryVersion, changelogFor, problems, withVersion, type Texts } from "./version.ts";

const texts = (v = "0.2.0"): Texts => ({
  packageJson: `{\n  "name": "peetmail",\n  "version": "${v}",\n  "devDependencies": { "x": "1.0.0" }\n}\n`,
  tauriConf: `{ "productName": "PeetMail", "version": "../package.json" }`,
  cargoToml: `[package]\nname = "peetmail"\nversion = "${v}"\n\n[dependencies]\nserde = { version = "1" }\n`,
  cargoLock: `[[package]]\nname = "parking"\nversion = "9.9.9"\n\n[[package]]\nname = "peetmail"\nversion = "${v}"\ndependencies = []\n`,
});

describe("version check", () => {
  it("passes when every copy agrees, with and without a matching tag", () => {
    expect(problems(texts())).toEqual([]);
    expect(problems(texts(), "v0.2.0")).toEqual([]);
  });

  it("names each copy that drifted", () => {
    const t = texts();
    t.cargoToml = t.cargoToml.replace("0.2.0", "0.1.0");
    t.cargoLock = t.cargoLock.replace(/name = "peetmail"\nversion = "0.2.0"/, 'name = "peetmail"\nversion = "0.1.9"');
    expect(problems(t)).toEqual(["Cargo.toml has 0.1.0, package.json has 0.2.0", "Cargo.lock has 0.1.9, package.json has 0.2.0"]);
  });

  it("rejects a tag that isn't v + the version", () => {
    expect(problems(texts(), "v0.2.1")).toHaveLength(1);
    expect(problems(texts(), "0.2.0")).toHaveLength(1);
  });

  it("insists tauri.conf.json reads the version from package.json", () => {
    const t = texts();
    t.tauriConf = `{ "version": "0.2.0" }`;
    expect(problems(t)[0]).toMatch(/tauri\.conf\.json/);
  });
});

describe("version set", () => {
  it("writes our crate's version and leaves dependencies alone", () => {
    const next = withVersion(texts(), "1.3.0-beta.1");
    expect(problems(next, "v1.3.0-beta.1")).toEqual([]);
    expect(next.cargoLock).toContain('name = "parking"\nversion = "9.9.9"');
    expect(next.cargoToml).toContain('serde = { version = "1" }');
    expect(next.packageJson).toContain('"x": "1.0.0"');
  });

  it("works on CRLF files", () => {
    const t = texts();
    const crlf: Texts = { ...t, cargoToml: t.cargoToml.replaceAll("\n", "\r\n"), cargoLock: t.cargoLock.replaceAll("\n", "\r\n") };
    expect(problems(withVersion(crlf, "0.3.0"))).toEqual([]);
  });

  it("refuses something that isn't semver", () => {
    expect(() => withVersion(texts(), "v1.2")).toThrow(/semver/);
  });
});

describe("canary version", () => {
  it("is the next patch, numbered by the CI run", () => {
    expect(canaryVersion("0.2.0", 42)).toBe("0.2.1-canary.42");
  });

  it("keeps the core of a version that's already a pre-release", () => {
    expect(canaryVersion("0.3.0-beta.1", 7)).toBe("0.3.0-canary.7");
  });

  it("rejects a bad run number", () => {
    expect(() => canaryVersion("0.2.0", 0)).toThrow();
    expect(() => canaryVersion("0.2.0", NaN)).toThrow();
  });
});

describe("android version code", () => {
  it("sorts like the versions, so every update installs over the build before it", () => {
    const order = ["0.2.1", "0.2.2-canary.39", "0.2.2-canary.40", "0.2.2", "0.3.0-canary.1", "0.3.0-beta.2", "0.3.0", "1.0.0", "20.99.99"];
    const codes = order.map(androidVersionCode);
    expect(codes).toEqual([...codes].sort((a, b) => a - b));
    expect(new Set(codes).size).toBe(codes.length);
    expect(androidVersionCode("0.2.2-canary.40")).toBe(2_020_040);
    expect(androidVersionCode("0.2.1")).toBe(2_019_999);
  });

  it("is above Tauri's default code, which the first builds used", () => {
    expect(androidVersionCode("0.2.1")).toBeGreaterThan(2001);
  });

  it("refuses versions it can't order", () => {
    for (const v of ["0.2.2-canary.9999", "0.100.0", "0.2.100", "21.0.0", "0.3.0-beta", "1.2"]) {
      expect(() => androidVersionCode(v), v).toThrow();
    }
  });
});

describe("changelog on set", () => {
  const md = "# What's new\n\n## Unreleased\n\n- A thing\n";

  it("a release takes over the Unreleased lines", () => {
    expect(changelogFor(md, "0.2.0", "2026-11-03")).toContain("## Unreleased\n\n## 0.2.0 — 2026-11-03\n\n- A thing");
  });

  it("a canary build leaves the changelog alone", () => {
    expect(changelogFor(md, "0.2.1-canary.42", "2026-11-03")).toBe(md);
  });
});
