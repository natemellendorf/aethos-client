import { chromium } from 'playwright-core';
import { generateIpcMockScript } from '@srsholmes/tauri-playwright';

const script = generateIpcMockScript({
  app_diagnostics: () => ({ app: "test", version: "0.1.0", profile: "debug", platform: "linux", arch: "x86_64", verboseLoggingEnabled: true, logFilePath: "/tmp/test.log" }),
  bootstrap_state: () => ({ identity: { wayfarerId: "abc123", deviceId: "dev1", verifyingKeyB64: "", deviceName: "Test" }, settings: { relaySyncEnabled: false, gossipSyncEnabled: true, verboseLoggingEnabled: true, relayEndpoints: [], messageTtlSeconds: 3600, enterToSend: true, bleDiscoveryEnabled: false }, contacts: {}, chat: { threads: {}, selectedContact: null, newContacts: [] } }),
  app_version: () => "0.1.0",
  gossip_status: () => ({ enabled: true, running: true, lastActivityMs: Date.now(), lastEvent: "listening" }),
  relay_health_status: () => ({ primaryStatus: "disabled", secondaryStatus: "disabled", chipText: "disabled", chipState: "disabled" }),
  encounter_activity_snapshot: () => ({ events: [] }),
  read_app_log: () => ({ lines: [], logFilePath: "/tmp/test.log" }),
  generate_share_qr: () => ({ wayfarerId: "abc123", filePath: "/tmp/qr.png", pngBase64: "" }),
});

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage();
await page.addInitScript(script);
await page.goto('http://127.0.0.1:1420', { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(3000);

const bodyText = await page.evaluate(() => document.body?.innerText?.substring(0, 2000) || 'NO BODY');
console.log('=== BODY TEXT ===');
console.log(bodyText);

const hasTabs = await page.evaluate(() => !!document.querySelector('[data-testid="tab-chats"]'));
console.log('tab-chats exists:', hasTabs);

const hasComposer = await page.evaluate(() => !!document.querySelector('[data-testid="chat-composer"]'));
console.log('chat-composer exists:', hasComposer);

const errors = await page.evaluate(() => window.__TAURI_MOCK_CALLS__ || []);
console.log('mock calls:', JSON.stringify(errors));

await browser.close();
