import { test, expect, getCapturedInvokes, clearCapturedInvokes } from "../fixtures";

test.describe("chat flow", () => {
  test.beforeEach(async ({ tauriPage }) => {
    // Clear captured invoke log so each test starts fresh
    await clearCapturedInvokes(tauriPage);
  });

  test("chat composer and send button are visible", async ({ tauriPage }) => {
    await expect(tauriPage.getByTestId("chat-composer")).toBeVisible();
    await expect(tauriPage.getByTestId("chat-send")).toBeVisible();
  });

  test("composer is disabled when no contact is selected", async ({ tauriPage }) => {
    const composer = tauriPage.getByTestId("chat-composer");
    await expect(composer).toBeVisible();
    await expect(composer).toBeDisabled();
  });

  test("bootstrap_state is called on page load", async ({ tauriPage }) => {
    const calls = await getCapturedInvokes(tauriPage);
    const bootstrapCall = calls.find((c) => c.cmd === "bootstrap_state");
    expect(bootstrapCall).toBeTruthy();
  });

  test("attachment input exists", async ({ tauriPage }) => {
    await expect(tauriPage.getByTestId("chat-attachment-input")).toBeAttached();
  });
});
