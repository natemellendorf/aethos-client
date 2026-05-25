import { test, expect } from "../fixtures";

test("check console errors", async ({ tauriPage }) => {
  // Collect console messages
  const errors: string[] = [];
  const warnings: string[] = [];
  
  // BrowserPageAdapter doesn't expose the underlying page directly in fixtures
  // But we can use evaluate to check for errors
  
  await new Promise(r => setTimeout(r, 3000));
  
  // Check for any uncaught errors
  const hasError = await tauriPage.evaluate(() => {
    // Check if the main script loaded
    const scripts = document.querySelectorAll('script[type="module"]');
    return {
      scriptCount: scripts.length,
      rootChildren: document.querySelector("#root")?.childElementCount ?? -1,
      url: location.href,
      readyState: document.readyState,
    };
  });
  
  console.log("hasError:", JSON.stringify(hasError));
});
