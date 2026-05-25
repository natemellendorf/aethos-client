import { createTauriTest, type CapturedInvoke } from "@srsholmes/tauri-playwright";

/**
 * Mock responses for common Tauri IPC commands.
 * In browser mode these replace real Tauri `invoke()` calls.
 */
const IPC_MOCKS: Record<string, (args?: Record<string, unknown>) => unknown> = {
  app_diagnostics: () => ({
    app: "aethos-tauri-desktop",
    version: "0.1.0",
    profile: "debug",
    platform: "linux",
    arch: "x86_64",
    verboseLoggingEnabled: true,
    logFilePath: "/tmp/aethos-e2e-test.log",
  }),

  app_version: () => "0.1.0",

  bootstrap_state: () => ({
    identity: {
      wayfarerId: "e2e0000000000000000000000000000000000000000000000000000000000000",
      deviceId: "e2e-device-001",
      verifyingKeyB64: "dGVzdC1wdWJrZXk=",
      deviceName: "E2E Test Device",
    },
    settings: {
      relaySyncEnabled: false,
      gossipSyncEnabled: true,
      verboseLoggingEnabled: true,
      relayEndpoints: [],
      messageTtlSeconds: 3600,
      enterToSend: true,
      bleDiscoveryEnabled: false,
    },
    contacts: {} as Record<string, string>,
    chat: {
      threads: {} as Record<string, unknown[]>,
      selectedContact: null as string | null,
      newContacts: [] as string[],
    },
  }),

  chat_snapshot: () => ({
    contacts: {} as Record<string, string>,
    chat: {
      threads: {} as Record<string, unknown[]>,
      selectedContact: null as string | null,
      newContacts: [] as string[],
    },
  }),

  gossip_status: () => ({
    enabled: true,
    running: true,
    lastActivityMs: Date.now(),
    lastEvent: "listening",
  }),

  encounter_activity_snapshot: () => ({
    bleDiscoveryStatus: "BLE discovery inactive (disabled by environment)",
    bleDiscoveryEnabled: false,
    bleAdvertisementsDetectedRecently: false,
    recentBleRejectionsCount: 0,
    lastBleRejectionReason: null,
    lastBleSightingUnixMs: null,
    recentBleSightingsCount: 0,
    lastBleTriggeredEncounterUnixMs: null,
    lastTransferBearer: null,
    lastDiscoveryBearer: null,
    lastTransferProgressMessage: null,
    lastTransferProgressUnixMs: null,
    noTransferPathMessage: null,
    noTransferPathUnixMs: null,
    events: [] as unknown[],
  }),

  relay_health_status: () => ({
    primaryStatus: "relay sync disabled in settings",
    secondaryStatus: "relay sync disabled in settings",
    chipText: "Relays: disabled",
    chipState: "disabled",
  }),

  generate_share_qr: () => ({
    wayfarerId: "e2e0000000000000000000000000000000000000000000000000000000000000",
    filePath: "/tmp/aethos-e2e-share-qr.png",
    pngBase64: "",
  }),

  upsert_contact: (args) => {
    return { [String(args?.wayfarerId || "")]: String(args?.alias || "") };
  },

  remove_contact: () => ({} as Record<string, string>),

  save_chat: (args) => args?.chat || { threads: {} },

  send_message: () => ({
    message: {
      msgId: "e2e-msg-001",
      text: "",
      timestamp: new Date().toISOString(),
      created_at_unix: Math.floor(Date.now() / 1000),
      created_at_unix_ms: Date.now(),
      direction: "Outgoing",
      seen: true,
    },
    chat: { threads: {} },
    contacts: {} as Record<string, string>,
    encounterStatus: "e2e mock: message queued",
    pulledMessages: 0,
  }),

  sync_inbox: () => ({
    chat: { threads: {} },
    contacts: {} as Record<string, string>,
    pulledMessages: 0,
    status: "e2e mock: sync complete",
  }),
};

/**
 * Aethos-specific test fixtures built on @srsholmes/tauri-playwright.
 *
 * Usage:
 *   import { test, expect } from '../fixtures';
 *
 *   test('app loads', async ({ tauriPage }) => {
 *     await expect(tauriPage.getByTestId('tab-chats')).toBeVisible();
 *   });
 */
export const { test, expect } = createTauriTest({
  /** Vite dev server URL (used in browser mode) */
  devUrl: "http://127.0.0.1:1420",

  /** Mock Tauri IPC handlers for browser-mode testing */
  ipcMocks: IPC_MOCKS,

  /** Socket path for Tauri plugin bridge (default: /tmp/tauri-playwright.sock) */
  mcpSocket: process.env.TAURI_PLAYWRIGHT_SOCKET || "/tmp/tauri-playwright.sock",
});

/**
 * Helper: capture Tauri IPC calls made during a test (browser mode only).
 *
 * @example
 *   const calls = await getCapturedInvokes(tauriPage);
 *   expect(calls.some(c => c.cmd === 'bootstrap_state')).toBe(true);
 */
export { getCapturedInvokes, clearCapturedInvokes, emitMockEvent } from "@srsholmes/tauri-playwright";

export type { CapturedInvoke };
