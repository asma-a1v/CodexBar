import { describe, expect, it } from "vitest";
import source from "./Settings.tsx?raw";

describe("Settings navigation", () => {
  it("routes providers separately from general", () => {
    expect(source).toContain('{ id: "providers", labelKey: "TabProviders" }');
    expect(source).toMatch(/activeTab === "general"[\s\S]*?<GeneralTab[\s\S]*?activeTab === "providers"[\s\S]*?<ProvidersTab/);
  });
});
