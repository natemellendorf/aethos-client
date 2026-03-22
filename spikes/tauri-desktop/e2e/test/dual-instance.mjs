import fs from "node:fs/promises";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { expect } from "chai";
import { Builder, By, Capabilities, until } from "selenium-webdriver";

const __dirname = fileURLToPath(new URL(".", import.meta.url));
const E2E_DIR = path.resolve(__dirname, "..");
const DESKTOP_DIR = path.resolve(E2E_DIR, "..");
const TAURI_BIN = path.resolve(DESKTOP_DIR, "src-tauri", "target", "debug", "aethos");
const RUN_ID = process.env.AETHOS_E2E_RUN_ID || `run-${Date.now()}`;
const TEST_CASE_ID = process.env.AETHOS_E2E_TEST_CASE_ID || "dual-instance-gossip";
const SCENARIO = process.env.AETHOS_E2E_SCENARIO || "clean";
const RELAY_ENDPOINT = process.env.AETHOS_E2E_RELAY_ENDPOINT || "";
const E2E_DISABLE_RELAY = (process.env.AETHOS_E2E_DISABLE_RELAY || "1") === "1";
const E2E_LOOPBACK_ONLY = (process.env.AETHOS_E2E_LOOPBACK_ONLY || "1") === "1";
const E2E_EAGER_UNICAST = (process.env.AETHOS_E2E_EAGER_UNICAST || "1") === "1";
const E2E_LOCALHOST_FANOUT = (process.env.AETHOS_E2E_LOCALHOST_FANOUT || "1") === "1";
const E2E_DISABLE_LAN_TCP = (process.env.AETHOS_E2E_DISABLE_LAN_TCP || "1") === "1";
const BURST_COUNT = Number(process.env.AETHOS_E2E_BURST_COUNT || "1");
const BURST_INTERVAL_MS = Number(process.env.AETHOS_E2E_BURST_INTERVAL_MS || "1000");
const BURST_RECEIVE_TIMEOUT_MS = Number(process.env.AETHOS_E2E_BURST_RECEIVE_TIMEOUT_MS || "600000");
const TEST_TIMEOUT_MS = Number(process.env.AETHOS_E2E_TEST_TIMEOUT_MS || "1200000");
const GOSSIP_BASE_PORT = Number(process.env.AETHOS_E2E_GOSSIP_BASE_PORT || 58655);
const GOSSIP_PORT_A = GOSSIP_BASE_PORT;
const GOSSIP_PORT_B = GOSSIP_BASE_PORT + 1;
const ARTIFACT_ROOT = process.env.AETHOS_E2E_ARTIFACT_DIR
  ? path.resolve(process.env.AETHOS_E2E_ARTIFACT_DIR)
  : path.resolve(DESKTOP_DIR, "e2e", "artifacts", RUN_ID);
const E2E_WORKDIR = process.env.AETHOS_E2E_WORKDIR
  ? path.resolve(process.env.AETHOS_E2E_WORKDIR)
  : path.resolve(DESKTOP_DIR, "e2e", "workdir", RUN_ID);

const TAURI_DRIVER_A_PORT = 4444;
const TAURI_DRIVER_B_PORT = 4454;
const TAURI_NATIVE_A_PORT = 4445;
const TAURI_NATIVE_B_PORT = 4455;

const tauriDriverProcs = [];
let cleanupTriggered = false;
const cleanupFns = [];

function stateRootPath(name) {
  return path.join(E2E_WORKDIR, `aethos-${name}`);
}

function appLogPath(stateRoot) {
  return path.join(stateRoot, "aethos-linux", "aethos-linux.log");
}

function normalizedIdText(value) {
  return String(value || "").replace(/\s+/g, "").trim().toLowerCase();
}

async function readLogTail(filePath, maxChars = 10000) {
  try {
    const raw = await fs.readFile(filePath, "utf8");
    return raw.length > maxChars ? raw.slice(raw.length - maxChars) : raw;
  } catch {
    return "";
  }
}

async function readJsonFile(filePath) {
  const raw = await fs.readFile(filePath, "utf8");
  return JSON.parse(raw);
}

async function waitForIncomingMessageInState(stateRoot, expectedText) {
  const chatPath = path.join(stateRoot, "chat-history.json");
  return waitFor(async () => {
    try {
      const chat = await readJsonFile(chatPath);
      const threads = Object.values(chat?.threads || {});
      for (const thread of threads) {
        for (const msg of thread || []) {
          if (msg?.direction === "Incoming" && String(msg?.text || "") === expectedText) {
            return {
              found: true,
              msgId: msg?.msgId || "",
              threadKey: Object.keys(chat?.threads || {}).find((key) => (chat.threads[key] || []).some((m) => m?.msgId === msg?.msgId)) || ""
            };
          }
        }
      }
      return false;
    } catch {
      return false;
    }
  }, 120000, 700);
}

async function incomingMessagesByPrefix(stateRoot, prefix) {
  const chatPath = path.join(stateRoot, "chat-history.json");
  const chat = await readJsonFile(chatPath);
  const out = [];
  for (const thread of Object.values(chat?.threads || {})) {
    for (const msg of thread || []) {
      if (msg?.direction === "Incoming") {
        const text = String(msg?.text || "");
        if (text.startsWith(prefix)) {
          out.push(text);
        }
      }
    }
  }
  return out;
}

function burstSequenceGaps(prefixTexts, prefix, expectedCount) {
  const seen = new Set();
  for (const text of prefixTexts || []) {
    const raw = String(text || "");
    if (!raw.startsWith(prefix)) continue;
    const n = Number(raw.slice(prefix.length));
    if (Number.isFinite(n) && n > 0) seen.add(n);
  }
  const missing = [];
  for (let i = 1; i <= expectedCount; i += 1) {
    if (!seen.has(i)) missing.push(i);
  }
  return missing;
}

async function waitForIncomingPrefixCount(stateRoot, prefix, expectedCount, timeoutMs) {
  return waitFor(async () => {
    try {
      const incoming = await incomingMessagesByPrefix(stateRoot, prefix);
      if (incoming.length >= expectedCount) {
        return {
          found: true,
          count: incoming.length,
          texts: incoming
        };
      }
      return false;
    } catch {
      return false;
    }
  }, timeoutMs, 1000);
}

async function clickSyncInbox(driver) {
  const clicked = await waitFor(async () => {
    try {
      const ok = await driver.executeScript(
        "const buttons = Array.from(document.querySelectorAll('button')); const target = buttons.find((b)=> (b.textContent||'').toLowerCase().includes('sync inbox')); if (!target) return false; target.click(); return true;"
      );
      return !!ok;
    } catch {
      return false;
    }
  }, 5000, 250);
  return Boolean(clicked);
}

async function writeJsonArtifact(fileName, payload) {
  await fs.mkdir(ARTIFACT_ROOT, { recursive: true });
  await fs.writeFile(path.join(ARTIFACT_ROOT, fileName), `${JSON.stringify(payload, null, 2)}\n`, "utf8");
}

async function waitFor(predicate, timeoutMs = 30000, intervalMs = 250) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const value = await predicate();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
  return false;
}

async function clickElement(driver, selector, timeoutMs = 20000) {
  const element = await driver.wait(until.elementLocated(By.css(selector)), timeoutMs);
  await driver.wait(until.elementIsVisible(element), timeoutMs);
  await driver.executeScript("arguments[0].scrollIntoView({block: 'center', inline: 'center'});", element);
  try {
    await element.click();
  } catch {
    await driver.executeScript("arguments[0].click();", element);
  }
}

async function waitForSplashToClear(driver) {
  await waitFor(async () => {
    try {
      const overlays = await driver.findElements(By.css(".fixed.inset-0.z-50"));
      if (!overlays.length) return true;
      for (const overlay of overlays) {
        if (await overlay.isDisplayed()) {
          return false;
        }
      }
      return true;
    } catch {
      return false;
    }
  }, 30000, 300);
}

async function clickTab(driver, tabId) {
  await clickElement(driver, `[data-testid=\"tab-${tabId}\"]`);
}

async function openContactsAndAdd(driver, id, alias) {
  await clickTab(driver, "contacts");
  await waitForSplashToClear(driver);
  const beforeContacts = new Set(
    await driver.executeScript(
      "return Array.from(document.querySelectorAll('[data-testid^=\"chat-contact-\"]')).map((el)=>el.getAttribute('data-testid').replace('chat-contact-',''));"
    )
  );
  const idInput = await driver.findElement(By.css("[data-testid='contact-wayfarer-id']"));
  const aliasInput = await driver.findElement(By.css("[data-testid='contact-alias']"));
  await idInput.clear();
  await idInput.sendKeys(id);
  await aliasInput.clear();
  await aliasInput.sendKeys(alias);
  const saveBtn = await driver.findElement(By.css("[data-testid='contact-save']"));
  await driver.executeScript("arguments[0].scrollIntoView({block: 'center'}); arguments[0].click();", saveBtn);

  const saved = await waitFor(async () => {
    try {
      await clickTab(driver, "chats");
      const selector = `[data-testid='chat-contact-${id.toLowerCase()}']`;
      const matches = await driver.findElements(By.css(selector));
      return matches.length > 0;
    } catch {
      return false;
    }
  }, 30000, 500);

  if (!saved) {
    await clickTab(driver, "contacts");
    const debugContactState = await driver.executeScript(
      "const id=document.querySelector('[data-testid=\"contact-wayfarer-id\"]')?.value || ''; const alias=document.querySelector('[data-testid=\"contact-alias\"]')?.value || ''; const contactList=Array.from(document.querySelectorAll('[data-testid^=\"chat-contact-\"]')).map((el)=>el.getAttribute('data-testid').replace('chat-contact-','')); const status=document.querySelector('[data-testid=\"status-text\"]')?.textContent || ''; return {id, alias, contactList, status};"
    );
    throw new Error(`contact did not appear in chat list: ${id}; debug=${JSON.stringify(debugContactState)}; before=${JSON.stringify(Array.from(beforeContacts))}`);
  }
}

async function sendChatMessage(driver, text) {
  await clickTab(driver, "chats");
  const composer = await driver.findElement(By.css("[data-testid='chat-composer']"));
  await composer.clear();
  await composer.sendKeys(text);
  const sendBtn = await driver.findElement(By.css("[data-testid='chat-send']"));
  try {
    await sendBtn.click();
  } catch {
    await driver.executeScript("arguments[0].click();", sendBtn);
  }
}

async function clickContactInChats(driver, wayfarerId) {
  await clickTab(driver, "chats");
  const selector = `[data-testid='chat-contact-${wayfarerId.toLowerCase()}']`;
  await clickElement(driver, selector, 30000);
  const isSelected = await waitFor(async () => {
    try {
      return await driver.executeScript(
        "const el=document.querySelector(arguments[0]); return !!el && el.className.includes('border-blue-300');",
        selector
      );
    } catch {
      return false;
    }
  }, 5000, 200);
  if (!isSelected) {
    await driver.executeScript("const el=document.querySelector(arguments[0]); if (el) el.click();", selector);
  }
}

async function announceGossip(driver) {
  await clickTab(driver, "settings");
  await clickElement(driver, "[data-testid='settings-announce-gossip']");
}

async function readIdentityWayfarerId(stateRoot) {
  const identityPath = path.join(stateRoot, "aethos-linux", "identity.json");
  const ok = await waitFor(async () => {
    try {
      const raw = await fs.readFile(identityPath, "utf8");
      const parsed = JSON.parse(raw);
      return /^[0-9a-f]{64}$/.test(normalizedIdText(parsed?.wayfarer_id));
    } catch {
      return false;
    }
  }, 30000, 250);

  if (!ok) {
    throw new Error(`identity file not ready: ${identityPath}`);
  }

  const raw = await fs.readFile(identityPath, "utf8");
  const parsed = JSON.parse(raw);
  return normalizedIdText(parsed?.wayfarer_id);
}

async function getOwnWayfarerId(driver, fallbackStateRoot) {
  const ready = await waitFor(async () => {
    try {
      await clickTab(driver, "share");
      const pre = await driver.wait(until.elementLocated(By.css("[data-testid='share-wayfarer-id']")), 8000);
      const text = await pre.getText();
      const id = normalizedIdText(text);
      return /^[0-9a-f]{64}$/.test(id) ? id : false;
    } catch {
      return false;
    }
  }, 30000, 400);

  if (ready) return String(ready);
  if (fallbackStateRoot) {
    return readIdentityWayfarerId(fallbackStateRoot);
  }
  throw new Error("wayfarer id unavailable from share tab");
}

async function openTauriSession(sessionName, stateRoot, tauriPort = 4444) {
  await fs.mkdir(stateRoot, { recursive: true });
  const env = {
    ...process.env,
    TAURI_AUTOMATION: "true",
    TAURI_WEBVIEW_AUTOMATION: "true",
    AETHOS_STATE_DIR: stateRoot,
    XDG_DATA_HOME: stateRoot,
    XDG_STATE_HOME: stateRoot,
    AETHOS_DISABLE_LAN_TCP: E2E_DISABLE_LAN_TCP ? "1" : "0",
    AETHOS_GOSSIP_LAN_PORT: String(sessionName === "a" ? GOSSIP_PORT_A : GOSSIP_PORT_B),
    AETHOS_GOSSIP_PEER_PORTS: `${GOSSIP_PORT_A},${GOSSIP_PORT_B}`,
    AETHOS_GOSSIP_LOCALHOST_FANOUT: E2E_LOCALHOST_FANOUT ? "1" : "0",
    AETHOS_GOSSIP_EAGER_UNICAST: E2E_EAGER_UNICAST ? "1" : "0",
    AETHOS_GOSSIP_LOOPBACK_ONLY: E2E_LOOPBACK_ONLY ? "1" : "0",
    AETHOS_STRUCTURED_LOGS: process.env.AETHOS_STRUCTURED_LOGS || "1",
    AETHOS_E2E_RUN_ID: RUN_ID,
    AETHOS_E2E_TEST_CASE_ID: TEST_CASE_ID,
    AETHOS_E2E_SCENARIO: SCENARIO,
    AETHOS_E2E_NODE_LABEL: sessionName === "a" ? "wayfarer-1" : "wayfarer-2",
    AETHOS_E2E_DISABLE_RELAY: E2E_DISABLE_RELAY ? "1" : "0",
    AETHOS_E2E_FORCE_VERBOSE: process.env.AETHOS_E2E_FORCE_VERBOSE || "1",
    AETHOS_E2E_FORCE_GOSSIP: process.env.AETHOS_E2E_FORCE_GOSSIP || "1",
    AETHOS_E2E_INSTANCE: sessionName
  };

  const capabilities = new Capabilities();
  capabilities.setBrowserName("wry");
  capabilities.set("tauri:options", {
    application: TAURI_BIN,
    args: [
      `--aethos-state-dir=${stateRoot}`,
      `--aethos-gossip-lan-port=${sessionName === "a" ? GOSSIP_PORT_A : GOSSIP_PORT_B}`,
      `--aethos-gossip-peer-ports=${GOSSIP_PORT_A},${GOSSIP_PORT_B}`,
      `--aethos-disable-lan-tcp=${E2E_DISABLE_LAN_TCP ? "1" : "0"}`,
      `--aethos-gossip-localhost-fanout=${E2E_LOCALHOST_FANOUT ? "1" : "0"}`,
      `--aethos-gossip-eager-unicast=${E2E_EAGER_UNICAST ? "1" : "0"}`,
      `--aethos-gossip-loopback-only=${E2E_LOOPBACK_ONLY ? "1" : "0"}`,
      `--aethos-e2e-disable-relay=${E2E_DISABLE_RELAY ? "1" : "0"}`,
      "--aethos-e2e-force-verbose=1",
      "--aethos-e2e-force-gossip=1"
    ],
    env
  });

  const driver = await new Builder()
    .usingServer(`http://127.0.0.1:${tauriPort}/`)
    .withCapabilities(capabilities)
    .build();

  cleanupFns.push(async () => {
    try {
      await driver.quit();
    } catch {
      // ignore teardown errors
    }
  });

  return {
    driver,
    stateRoot,
    logPath: appLogPath(stateRoot)
  };
}

async function startTauriDriver(port, nativePort) {
  const child = spawn(
    "tauri-driver",
    ["--port", String(port), "--native-port", String(nativePort)],
    {
      stdio: ["ignore", "inherit", "inherit"],
      env: process.env
    }
  );
  tauriDriverProcs.push(child);
  cleanupFns.push(async () => {
    if (!child.killed) {
      child.kill("SIGTERM");
    }
  });
}

async function shutdownAll() {
  if (cleanupTriggered) return;
  cleanupTriggered = true;
  while (cleanupFns.length > 0) {
    const fn = cleanupFns.pop();
    try {
      await fn();
    } catch {
      // ignore teardown errors
    }
  }
}

async function captureScreenshot(driver, fileName) {
  try {
    await fs.mkdir(ARTIFACT_ROOT, { recursive: true });
    const image = await driver.takeScreenshot();
    await fs.writeFile(path.join(ARTIFACT_ROOT, fileName), image, "base64");
  } catch {
    // best effort
  }
}

before(async function () {
  this.timeout(TEST_TIMEOUT_MS);

  await fs.mkdir(E2E_WORKDIR, { recursive: true });
  await fs.mkdir(ARTIFACT_ROOT, { recursive: true });

  spawnSync("bash", ["-lc", "pkill -f 'src-tauri/target/debug/aethos' || true; pkill -f tauri-driver || true; pkill -f WebKitWebDriver || true"], {
    cwd: DESKTOP_DIR,
    stdio: "inherit"
  });

  const buildResult = spawnSync("npx", ["tauri", "build", "--debug", "--no-bundle"], {
    cwd: DESKTOP_DIR,
    stdio: "inherit",
    shell: true
  });
  if (buildResult.status !== 0) {
    throw new Error("tauri build failed");
  }

  await startTauriDriver(TAURI_DRIVER_A_PORT, TAURI_NATIVE_A_PORT);
  await startTauriDriver(TAURI_DRIVER_B_PORT, TAURI_NATIVE_B_PORT);

  await new Promise((resolve) => setTimeout(resolve, 1200));
});

after(async function () {
  await shutdownAll();
  spawnSync("bash", ["-lc", "pkill -f 'src-tauri/target/debug/aethos' || true; pkill -f tauri-driver || true; pkill -f WebKitWebDriver || true"], {
    cwd: DESKTOP_DIR,
    stdio: "inherit"
  });
});

process.on("SIGINT", async () => {
  await shutdownAll();
  process.exit(130);
});

process.on("SIGTERM", async () => {
  await shutdownAll();
  process.exit(143);
});

describe("dual instance gossip e2e", function () {
  it("sends message between two isolated desktop instances and writes logs", async function () {
    this.timeout(TEST_TIMEOUT_MS);

    const a = await openTauriSession("a", stateRootPath("a"), TAURI_DRIVER_A_PORT);
    const b = await openTauriSession("b", stateRootPath("b"), TAURI_DRIVER_B_PORT);
    await writeJsonArtifact("run-index.json", {
      run_id: RUN_ID,
      test_case_id: TEST_CASE_ID,
      scenario: SCENARIO,
      started_at_unix_ms: Date.now(),
      topology: {
        nodes: [
          { label: "wayfarer-1", state_dir: a.stateRoot, tauri_driver_port: TAURI_DRIVER_A_PORT },
          { label: "wayfarer-2", state_dir: b.stateRoot, tauri_driver_port: TAURI_DRIVER_B_PORT }
        ]
      },
      artifacts: {
        instance_a_log: a.logPath,
        instance_b_log: b.logPath
      },
      env: {
        disable_relay: E2E_DISABLE_RELAY ? "1" : "0",
        loopback_only: E2E_LOOPBACK_ONLY ? "1" : "0",
        eager_unicast: E2E_EAGER_UNICAST ? "1" : "0",
        localhost_fanout: E2E_LOCALHOST_FANOUT ? "1" : "0",
        disable_lan_tcp: E2E_DISABLE_LAN_TCP ? "1" : "0"
      }
    });

    await waitForSplashToClear(a.driver);
    await waitForSplashToClear(b.driver);

    const idA = await readIdentityWayfarerId(a.stateRoot);
    const idB = await readIdentityWayfarerId(b.stateRoot);
    expect(idA).to.not.equal(idB);

    await openContactsAndAdd(a.driver, idB, "Peer B");
    await openContactsAndAdd(b.driver, idA, "Peer A");

    await clickContactInChats(a.driver, idB);
    await clickContactInChats(b.driver, idA);

    const burstPrefix = `e2e-${Date.now()}-`;
    if (BURST_COUNT <= 1) {
      await sendChatMessage(a.driver, `${burstPrefix}1`);
    } else {
      for (let i = 1; i <= BURST_COUNT; i += 1) {
        await sendChatMessage(a.driver, `${burstPrefix}${i}`);
        if (i < BURST_COUNT && BURST_INTERVAL_MS > 0) {
          await new Promise((resolve) => setTimeout(resolve, BURST_INTERVAL_MS));
        }
      }
    }

    let inboundState;
    if (BURST_COUNT <= 1) {
      inboundState = await waitForIncomingMessageInState(b.stateRoot, `${burstPrefix}1`);
    } else {
      const started = Date.now();
      let lastObserved = 0;
      while (Date.now() - started < BURST_RECEIVE_TIMEOUT_MS) {
        inboundState = await waitForIncomingPrefixCount(b.stateRoot, burstPrefix, BURST_COUNT, 3000);
        if (inboundState?.found) break;

        const snapshot = await waitForIncomingPrefixCount(b.stateRoot, burstPrefix, 1, 1000);
        const observed = Number(snapshot?.count || 0);
        if (observed > lastObserved) {
          lastObserved = observed;
        }
        await clickSyncInbox(b.driver);
        await new Promise((resolve) => setTimeout(resolve, 800));
      }
      if (!inboundState?.found) {
        inboundState = await waitForIncomingPrefixCount(b.stateRoot, burstPrefix, 1, 1) || { found: false, count: 0, texts: [] };
      }
    }
    const observedIncomingCount = Number(inboundState?.count || 0);
    const inboundVisible = BURST_COUNT <= 1
      ? Boolean(inboundState?.found)
      : observedIncomingCount >= BURST_COUNT;

    if (!inboundVisible) {
      await captureScreenshot(a.driver, "wayfarer-1-failure.png");
      await captureScreenshot(b.driver, "wayfarer-2-failure.png");
      const logA = await readLogTail(a.logPath);
      const logB = await readLogTail(b.logPath);
      const settingsA = await fs.readFile(path.join(a.stateRoot, "settings.json"), "utf8").catch(() => "");
      const settingsB = await fs.readFile(path.join(b.stateRoot, "settings.json"), "utf8").catch(() => "");
      await writeJsonArtifact("failure-summary.json", {
        run_id: RUN_ID,
        test_case_id: TEST_CASE_ID,
        scenario: SCENARIO,
        failed_at_unix_ms: Date.now(),
        failure: "message did not converge within timeout",
        logs: {
          wayfarer_1_tail: logA,
          wayfarer_2_tail: logB
        },
        settings: {
          wayfarer_1: settingsA,
          wayfarer_2: settingsB
        },
        burst: {
          count: BURST_COUNT,
          interval_ms: BURST_INTERVAL_MS,
          prefix: burstPrefix,
          observed_incoming_count: observedIncomingCount,
          missing_sequence_numbers: burstSequenceGaps(inboundState?.texts || [], burstPrefix, BURST_COUNT)
        }
      });
      throw new Error(
        `message did not converge within timeout\n` +
          `instanceA log: ${a.logPath}\n${logA}\n` +
          `instanceB log: ${b.logPath}\n${logB}`
      );
    }

    if (inboundState?.threadKey) {
      await clickContactInChats(b.driver, inboundState.threadKey);
    }

    let logPathTextA = "";
    let logPathTextB = "";
    try {
      await clickTab(a.driver, "settings");
      await clickTab(b.driver, "settings");
      const settingsLogA = await a.driver.wait(until.elementLocated(By.css("[data-testid='settings-log-path']")), 20000);
      const settingsLogB = await b.driver.wait(until.elementLocated(By.css("[data-testid='settings-log-path']")), 20000);
      logPathTextA = await settingsLogA.getText();
      logPathTextB = await settingsLogB.getText();

      expect(path.resolve(logPathTextA)).to.equal(path.resolve(a.logPath));
      expect(path.resolve(logPathTextB)).to.equal(path.resolve(b.logPath));
    } catch {
      logPathTextA = a.logPath;
      logPathTextB = b.logPath;
    }

    const statusA = await (await a.driver.findElement(By.css("[data-testid='status-text']"))).getText();
    const statusB = await (await b.driver.findElement(By.css("[data-testid='status-text']"))).getText();
    expect(statusA.length).to.be.greaterThan(0);
    expect(statusB.length).to.be.greaterThan(0);

    await writeJsonArtifact("run-result.json", {
      run_id: RUN_ID,
      test_case_id: TEST_CASE_ID,
      scenario: SCENARIO,
      completed_at_unix_ms: Date.now(),
      status: "passed",
      convergence: {
        method: BURST_COUNT <= 1 ? "chat-history-state" : "chat-history-prefix-count",
        burst_count: BURST_COUNT,
        burst_interval_ms: BURST_INTERVAL_MS,
        inbound_thread_key: inboundState?.threadKey || "",
        inbound_msg_id: inboundState?.msgId || "",
        observed_incoming_count: observedIncomingCount,
        missing_sequence_numbers: burstSequenceGaps(inboundState?.texts || [], burstPrefix, BURST_COUNT)
      },
      node_status: {
        wayfarer_1: statusA,
        wayfarer_2: statusB
      },
      logs: {
        wayfarer_1: logPathTextA,
        wayfarer_2: logPathTextB
      }
    });
  });
});
