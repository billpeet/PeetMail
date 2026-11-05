import { describe, expect, test } from "vitest";
import { ConfigError, loadConfig, parseRegisterRules, PEETMAIL_MICROSOFT_CLIENT_ID } from "./config.js";

describe("ALLOW_REGISTER", () => {
  test("is required, with no open default", () => {
    expect(() => parseRegisterRules(undefined)).toThrow(ConfigError);
    expect(() => parseRegisterRules("  ")).toThrow(/ALLOW_REGISTER/);
  });

  test("takes addresses, domains, tenants, anyone and nobody", () => {
    expect(parseRegisterRules("Priya@Fabrikam.io, @contoso.com tenant:72F988BF-86f1-41af-91ab-2d7cd011db47")).toEqual([
      { kind: "address", value: "priya@fabrikam.io" },
      { kind: "domain", value: "contoso.com" },
      { kind: "tenant", value: "72f988bf-86f1-41af-91ab-2d7cd011db47" },
    ]);
    expect(parseRegisterRules("*")).toEqual([{ kind: "anyone" }]);
    expect(parseRegisterRules("none")).toEqual([]);
  });

  test("refuses what it can't read, rather than guessing", () => {
    expect(() => parseRegisterRules("contoso.com")).toThrow(/contoso\.com/);
    expect(() => parseRegisterRules("tenant:")).toThrow(ConfigError);
    expect(() => parseRegisterRules("@")).toThrow(ConfigError);
  });
});

test("defaults: PeetMail's Microsoft app, no Google app, the real providers", () => {
  const c = loadConfig({ ALLOW_REGISTER: "none" });
  expect(c.clientIds).toEqual({ microsoft: [PEETMAIL_MICROSOFT_CLIENT_ID], google: [] });
  expect(c.discoveryUrls.microsoft).toMatch(/^https:\/\/login\.microsoftonline\.com\//);
  expect(c.databasePath).toMatch(/sync\.db$/);
});

test("a self-hoster's own apps replace the defaults", () => {
  const c = loadConfig({ ALLOW_REGISTER: "none", MICROSOFT_CLIENT_IDS: "a, b", GOOGLE_CLIENT_IDS: "g" });
  expect(c.clientIds).toEqual({ microsoft: ["a", "b"], google: ["g"] });
});
