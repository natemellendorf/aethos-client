import { test, expect } from "../fixtures";

test("debug page state", async ({ tauriPage }) => {
  // Wait a bit for the app to initialize
  await new Promise(r => setTimeout(r, 3000));
  
  // Check what's on the page
  const bodyText = await tauriPage.evaluate(() => document.body?.innerText?.substring(0, 3000) || "NO BODY");
  console.log("=== BODY TEXT ===");
  console.log(bodyText);
  
  const html = await tauriPage.evaluate(() => document.querySelector("#root")?.innerHTML?.substring(0, 2000) || "NO ROOT");
  console.log("=== ROOT HTML ===");
  console.log(html);
  
  // Check for errors
  const errors = await tauriPage.evaluate(() => (window as any).__TAURI_MOCK_CALLS__?.length || 0);
  console.log("Mock calls count:", errors);
});
