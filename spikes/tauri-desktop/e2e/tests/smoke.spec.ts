import { test, expect } from "../fixtures";

test.describe("app smoke tests", () => {
  test("app loads and shows tab bar", async ({ tauriPage }) => {
    await expect(tauriPage.getByTestId("tab-chats")).toBeVisible();
    await expect(tauriPage.getByTestId("tab-contacts")).toBeVisible();
    await expect(tauriPage.getByTestId("tab-share")).toBeVisible();
    await expect(tauriPage.getByTestId("tab-settings")).toBeVisible();
  });

  test("status text is visible", async ({ tauriPage }) => {
    await expect(tauriPage.getByTestId("status-text")).toBeVisible();
  });

  test("navigates to contacts tab", async ({ tauriPage }) => {
    await tauriPage.getByTestId("tab-contacts").click();
    await expect(tauriPage.getByTestId("contact-wayfarer-id")).toBeVisible();
    await expect(tauriPage.getByTestId("contact-alias")).toBeVisible();
    await expect(tauriPage.getByTestId("contact-save")).toBeVisible();
  });

  test("navigates to share tab and shows wayfarer ID", async ({ tauriPage }) => {
    await tauriPage.getByTestId("tab-share").click();
    await expect(tauriPage.getByTestId("share-wayfarer-id")).toBeVisible();
    const idText = await tauriPage.getByTestId("share-wayfarer-id").innerText();
    expect(idText.length).toBeGreaterThan(0);
  });

  test("navigates to settings tab", async ({ tauriPage }) => {
    await tauriPage.getByTestId("tab-settings").click();
    await expect(tauriPage.getByTestId("settings-relay-sync")).toBeVisible();
    await expect(tauriPage.getByTestId("settings-gossip-sync")).toBeVisible();
    await expect(tauriPage.getByTestId("settings-verbose-logging")).toBeVisible();
    await expect(tauriPage.getByTestId("settings-log-path")).toBeVisible();
  });

  test("settings toggles can be clicked", async ({ tauriPage }) => {
    await tauriPage.getByTestId("tab-settings").click();
    await tauriPage.getByTestId("settings-verbose-logging").click();
    await tauriPage.getByTestId("settings-gossip-sync").click();
  });

  test("returns to chats tab using tab navigation", async ({ tauriPage }) => {
    await tauriPage.getByTestId("tab-settings").click();
    await tauriPage.getByTestId("tab-chats").click();
    await expect(tauriPage.getByTestId("tab-chats")).toBeVisible();
  });
});
