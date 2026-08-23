import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SettingsSnapshot } from "../../../../../types/bridge";
import { ClaudeCreds } from "./ClaudeCreds";

const tauriMocks = vi.hoisted(() => ({
  getSettingsSnapshot: vi.fn(),
  updateSettings: vi.fn(),
}));

vi.mock("../../../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../../../lib/tauri")>()),
  ...tauriMocks,
}));

type ClaudeSettingsSnapshot = Pick<
  SettingsSnapshot,
  | "claudeAvoidKeychainPrompts"
  | "claudeDailyRoutinesUsageVisible"
  | "claudeAllowReadingClaudeCodeCredentials"
>;

function snapshot(
  overrides: Partial<ClaudeSettingsSnapshot> = {},
): ClaudeSettingsSnapshot {
  return {
    claudeAvoidKeychainPrompts: false,
    claudeDailyRoutinesUsageVisible: true,
    claudeAllowReadingClaudeCodeCredentials: false,
    ...overrides,
  };
}

describe("ClaudeCreds", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders the Claude Code credentials consent toggle, off by default", async () => {
    tauriMocks.getSettingsSnapshot.mockResolvedValue(snapshot());

    render(<ClaudeCreds t={(key) => key} />);

    const checkbox = await screen.findByRole("checkbox", {
      name: "ProviderClaudeAllowReadingClaudeCodeCredentials ProviderClaudeAllowReadingClaudeCodeCredentialsHelp",
    });
    expect(checkbox).not.toBeChecked();
    expect(
      screen.getByText("ProviderClaudeAllowReadingClaudeCodeCredentialsHelp"),
    ).toBeInTheDocument();
  });

  it("toggling the consent checkbox calls updateSettings and reflects the response", async () => {
    tauriMocks.getSettingsSnapshot.mockResolvedValue(snapshot());
    tauriMocks.updateSettings.mockResolvedValue(
      snapshot({ claudeAllowReadingClaudeCodeCredentials: true }),
    );

    render(<ClaudeCreds t={(key) => key} />);

    const checkbox = await screen.findByRole("checkbox", {
      name: "ProviderClaudeAllowReadingClaudeCodeCredentials ProviderClaudeAllowReadingClaudeCodeCredentialsHelp",
    });

    fireEvent.click(checkbox);

    await waitFor(() =>
      expect(tauriMocks.updateSettings).toHaveBeenCalledWith({
        claudeAllowReadingClaudeCodeCredentials: true,
      }),
    );
    await waitFor(() => expect(checkbox).toBeChecked());
  });
});
