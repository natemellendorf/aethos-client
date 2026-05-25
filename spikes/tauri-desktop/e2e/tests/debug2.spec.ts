import { test, expect } from "../fixtures";

test("debug page state v2", async ({ tauriPage }) => {
  await new Promise(r => setTimeout(r, 3000));
  
  const hasBody = await tauriPage.evaluate(() => !!document.body);
  const hasRoot = await tauriPage.evaluate(() => !!document.querySelector("#root"));
  const rootChildren = await tauriPage.evaluate(() => document.querySelector("#root")?.childElementCount ?? -1);
  const title = await tauriPage.evaluate(() => document.title);
  const url = await tauriPage.url();
  const mockCalls = await tauriPage.evaluate(() => (window as any).__TAURI_MOCK_CALLS__?.length ?? -1);
  const hasTauriInternals = await tauriPage.evaluate(() => !!(window as any).__TAURI_INTERNALS__);
  
  console.log("URL:", url);
  console.log("Title:", title);
  console.log("Has body:", hasBody);
  console.log("Has #root:", hasRoot);
  console.log("#root children:", rootChildren);
  console.log("Mock calls:", mockCalls);
  console.log("Has __TAURI_INTERNALS__:", hasTauriInternals);
  
  // Check document ready state
  const readyState = await tauriPage.evaluate(() => document.readyState);
  console.log("ReadyState:", readyState);
});
