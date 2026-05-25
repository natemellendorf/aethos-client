import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  timeout: 60_000,
  expect: { timeout: 10_000 },
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: [
    ["html", { outputFolder: "../playwright-report" }],
    ["list"],
  ],
  use: {
    baseURL: "http://127.0.0.1:1420",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },
  webServer: {
    command: "npm run dev",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: !process.env.CI,
    cwd: "..",
    timeout: 120_000,
  },
  projects: [
    {
      name: "browser-only",
      use: { ...devices["Desktop Chrome"], mode: "browser" as const },
      testMatch: "**/*.spec.ts",
    },
    {
      name: "tauri",
      use: { mode: "tauri" as const },
      testMatch: "**/*.spec.ts",
    },
  ],
});
