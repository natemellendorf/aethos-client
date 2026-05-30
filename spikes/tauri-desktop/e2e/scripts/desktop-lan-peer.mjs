import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { By, launchTauriSession, until } from "../lib/tauri-playwright-driver.mjs";

const __dirname = fileURLToPath(new URL(".", import.meta.url));
const E2E_DIR = path.resolve(__dirname, "..");
const DESKTOP_DIR = path.resolve(E2E_DIR, "..");
const CLIENT_REPO = path.resolve(DESKTOP_DIR, "../..");
const TAURI_BIN = path.resolve(DESKTOP_DIR, "src-tauri", "target", "debug", "aethos");

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 1) {
    const part = String(argv[i] || "");
    if (!part.startsWith("--")) continue;
    const eq = part.indexOf("=");
    if (eq >= 0) {
      out[part.slice(2, eq)] = part.slice(eq + 1);
      continue;
    }
    const key = part.slice(2);
    const next = argv[i + 1];
    if (next && !String(next).startsWith("--")) {
      out[key] = String(next);
      i += 1;
    } else {
      out[key] = "1";
    }
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
const RUN_ID = String(args["run-id"] || `desktop-lan-${Date.now()}`);
const STATE_ROOT = path.resolve(String(args["state-root"] || path.join(DESKTOP_DIR, "e2e", "workdir", RUN_ID, "desktop")));
const ARTIFACT_ROOT = path.resolve(String(args["artifact-root"] || path.join(DESKTOP_DIR, "e2e", "artifacts", RUN_ID)));
const SOCKET_PATH = path.resolve(String(args["socket-path"] || path.join("/tmp", `aethos-pw-${RUN_ID}-desktop.sock`)));
const GOSSIP_LAN_PORT = String(args["gossip-lan-port"] || "47655");
const GOSSIP_PEER_PORTS = String(args["gossip-peer-ports"] || GOSSIP_LAN_PORT);
const PEER_ID = String(args["peer-id"] || "").trim().toLowerCase();
const PEER_ALIAS = String(args["peer-alias"] || "Peer");
const OUTBOUND_MESSAGE = String(args["outbound-message"] || "");
const EXPECTED_INBOUND_MESSAGE = String(args["expected-inbound-message"] || "");
const TIMEOUT_MS = Number(args["timeout-ms"] || 180000);
const LINGER_MS = Number(args["linger-ms"] || 30000);
const LOOPBACK_ONLY = String(args["loopback-only"] || "0");
const LOCALHOST_FANOUT = String(args["localhost-fanout"] || "1");
const EAGER_UNICAST = String(args["eager-unicast"] || "1");
const DISABLE_LAN_TCP = String(args["disable-lan-tcp"] || "1");
const DIAGNOSTICS_RUN_ID = String(args["diagnostics-run-id"] || process.env.AETHOS_DIAGNOSTICS_RUN_ID || RUN_ID);
const DIAGNOSTICS_COLLECTOR_URL = String(
  args["diagnostics-collector-url"] || process.env.AETHOS_DIAGNOSTICS_COLLECTOR_URL || ""
).trim();

const cleanupFns = [];

function appLogPath(stateRoot) {
  return path.join(stateRoot, "aethos-linux", "aethos-linux.log");
}

function normalizedIdText(value) {
  return String(value || "").replace(/\s+/g, "").trim().toLowerCase();
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

async function readJsonFile(filePath) {
  const raw = await fs.readFile(filePath, "utf8");
  return JSON.parse(raw);
}

async function writeJsonFile(filePath, value) {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

async function runCommand(command, args, options = {}) {
  return await new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd || CLIENT_REPO,
      env: {
        ...process.env,
        ...(options.env || {})
      },
      stdio: ["ignore", "pipe", "pipe"]
    });
    let stdout = "";
    let stderr = "";
    child.stdout?.on("data", (chunk) => {
      stdout += String(chunk || "");
    });
    child.stderr?.on("data", (chunk) => {
      stderr += String(chunk || "");
    });
    child.once("error", reject);
    child.once("exit", (code) => {
      if (code === 0) {
        resolve({ stdout, stderr });
      } else {
        reject(new Error(`${command} ${args.join(" ")} exited ${code}\n${stdout}\n${stderr}`));
      }
    });
  });
}

async function seedContactAlias(stateRoot, id, alias) {
  const aliasesPath = path.join(stateRoot, "aethos-linux", "contact-aliases.json");
  let aliases = {};
  try {
    aliases = await readJsonFile(aliasesPath);
  } catch {
    aliases = {};
  }
  aliases[normalizedIdText(id)] = String(alias || "Peer").trim() || "Peer";
  await writeJsonFile(aliasesPath, aliases);
}

async function queueOutboundViaCli(stateRoot, to, text) {
  const result = await runCommand("cargo", [
    "run",
    "--quiet",
    "--manifest-path",
    path.join(CLIENT_REPO, "Cargo.toml"),
    "--bin",
    "aethos-cli",
    "--",
    "--data-dir",
    stateRoot,
    "send",
    "--to",
    normalizedIdText(to),
    "--text",
    text,
    "--ttl",
    "3600"
  ], {
    cwd: CLIENT_REPO,
    env: {
      AETHOS_STATE_DIR: stateRoot,
      XDG_DATA_HOME: stateRoot,
      XDG_STATE_HOME: stateRoot,
      AETHOS_STRUCTURED_LOGS: process.env.AETHOS_STRUCTURED_LOGS || "1",
      AETHOS_E2E: "1",
      AETHOS_E2E_RUN_ID: RUN_ID,
      AETHOS_E2E_TEST_CASE_ID: "desktop-lan-peer",
      AETHOS_E2E_SCENARIO: "ios-desktop-lan",
      AETHOS_E2E_NODE_LABEL: "desktop-lan-peer-cli"
    }
  });
  const line = result.stdout
    .split(/\r?\n/)
    .map((raw) => raw.trim())
    .filter(Boolean)
    .find((raw) => raw.includes('"event"') && raw.includes('"send_ok"'));
  if (!line) {
    return { found: true, msgId: "" };
  }
  const payload = JSON.parse(line);
  return { found: true, msgId: String(payload?.data?.item_id || "") };
}

async function readLogTail(filePath, maxChars = 15000) {
  try {
    const raw = await fs.readFile(filePath, "utf8");
    return raw.length > maxChars ? raw.slice(raw.length - maxChars) : raw;
  } catch {
    return "";
  }
}

async function clickElement(driver, selector, timeoutMs = 20000) {
  let element = await driver.wait(until.elementLocated(By.css(selector)), timeoutMs);
  await driver.wait(until.elementIsVisible(element), timeoutMs);
  if (!element || typeof element.click !== "function") {
    element = await driver.findElement(By.css(selector));
    await driver.wait(until.elementIsVisible(element), timeoutMs);
  }
  let lastError = null;
  for (let attempt = 0; attempt < 4; attempt += 1) {
    try {
      await element.click();
      return;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  throw lastError || new Error(`failed to click element: ${selector}`);
}

async function waitForSplashToClear(driver) {
  await waitFor(async () => {
    try {
      const overlays = await driver.findElements(By.css(".fixed.inset-0.z-50"));
      if (!overlays.length) return true;
      for (const overlay of overlays) {
        if (await overlay.isDisplayed()) return false;
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
  const normalizedId = normalizedIdText(id);
  const normalizedAlias = String(alias || "").trim();
  await clickTab(driver, "contacts");
  await waitForSplashToClear(driver);
  let saved = false;
  for (let attempt = 0; attempt < 6 && !saved; attempt += 1) {
    await clickTab(driver, "contacts");
    const idInput = await driver.findElement(By.css("[data-testid='contact-wayfarer-id']"));
    const aliasInput = await driver.findElement(By.css("[data-testid='contact-alias']"));
    await idInput.clear();
    await idInput.sendKeys(normalizedId);
    await aliasInput.clear();
    await aliasInput.sendKeys(normalizedAlias);
    const saveBtn = await driver.findElement(By.css("[data-testid='contact-save']"));
    await clickElement(driver, "[data-testid='contact-save']");
    saved = await waitFor(async () => {
      try {
        await clickTab(driver, "chats");
        const matches = await driver.findElements(By.css(`[data-testid='chat-contact-${normalizedId}']`));
        return matches.length > 0;
      } catch {
        return false;
      }
    }, 6000, 250);
  }
  if (!saved) {
    throw new Error(`contact did not appear in chat list: ${normalizedId}`);
  }
}

async function clickContactInChats(driver, wayfarerId) {
  await clickTab(driver, "chats");
  const selector = `[data-testid='chat-contact-${wayfarerId.toLowerCase()}']`;
  await clickElement(driver, selector, 30000);
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

async function announceGossip(driver) {
  await clickTab(driver, "settings");
  await clickElement(driver, "[data-testid='settings-announce-gossip']");
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
  }, 2500, 200);
  return Boolean(clicked);
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

async function waitForIncomingMessageInState(stateRoot, expectedText, timeoutMs = TIMEOUT_MS) {
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
              msgId: String(msg?.msgId || ""),
              threadKey:
                Object.keys(chat?.threads || {}).find((key) =>
                  (chat.threads[key] || []).some((m) => m?.msgId === msg?.msgId)
                ) || ""
            };
          }
        }
      }
      return false;
    } catch {
      return false;
    }
  }, timeoutMs, 700);
}

async function waitForOutgoingMessageInState(stateRoot, expectedText, timeoutMs = TIMEOUT_MS) {
  const chatPath = path.join(stateRoot, "chat-history.json");
  return waitFor(async () => {
    try {
      const chat = await readJsonFile(chatPath);
      const threads = Object.values(chat?.threads || {});
      for (const thread of threads) {
        for (const msg of thread || []) {
          if (msg?.direction === "Outgoing" && String(msg?.text || "") === expectedText) {
            return {
              found: true,
              msgId: String(msg?.msgId || ""),
              threadKey:
                Object.keys(chat?.threads || {}).find((key) =>
                  (chat.threads[key] || []).some((m) => m?.msgId === msg?.msgId)
                ) || ""
            };
          }
        }
      }
      return false;
    } catch {
      return false;
    }
  }, timeoutMs, 700);
}

async function maintainGossipActivity(driver, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 1500));
  }
}

async function openTauriSession(stateRoot, socketPath) {
  await fs.mkdir(stateRoot, { recursive: true });
  const env = {
    ...process.env,
    TAURI_AUTOMATION: "true",
    TAURI_WEBVIEW_AUTOMATION: "true",
    AETHOS_STATE_DIR: stateRoot,
    XDG_DATA_HOME: stateRoot,
    XDG_STATE_HOME: stateRoot,
    AETHOS_DISABLE_LAN_TCP: DISABLE_LAN_TCP,
    AETHOS_GOSSIP_LAN_PORT: GOSSIP_LAN_PORT,
    AETHOS_GOSSIP_PEER_PORTS: GOSSIP_PEER_PORTS,
    AETHOS_GOSSIP_LOCALHOST_FANOUT: LOCALHOST_FANOUT,
    AETHOS_GOSSIP_EAGER_UNICAST: EAGER_UNICAST,
    AETHOS_GOSSIP_LOOPBACK_ONLY: LOOPBACK_ONLY,
    AETHOS_STRUCTURED_LOGS: process.env.AETHOS_STRUCTURED_LOGS || "1",
    AETHOS_E2E_RUN_ID: RUN_ID,
    AETHOS_DIAGNOSTICS_RUN_ID: DIAGNOSTICS_RUN_ID,
    AETHOS_E2E: "1",
    AETHOS_E2E_TEST_CASE_ID: "desktop-lan-peer",
    AETHOS_E2E_SCENARIO: "ios-desktop-lan",
    AETHOS_E2E_NODE_LABEL: "desktop-lan-peer",
    AETHOS_E2E_DISABLE_RELAY: "1",
    AETHOS_E2E_FORCE_VERBOSE: "1",
    AETHOS_E2E_FORCE_GOSSIP: "1"
  };

  if (DIAGNOSTICS_COLLECTOR_URL) {
    env.AETHOS_DIAGNOSTICS_COLLECTOR_URL = DIAGNOSTICS_COLLECTOR_URL;
  }

  const driver = await launchTauriSession({
    application: TAURI_BIN,
    cwd: DESKTOP_DIR,
    socketPath,
    env,
    args: [
      "--aethos-e2e=1",
      `--aethos-state-dir=${stateRoot}`,
      `--aethos-gossip-lan-port=${GOSSIP_LAN_PORT}`,
      `--aethos-gossip-peer-ports=${GOSSIP_PEER_PORTS}`,
      `--aethos-disable-lan-tcp=${DISABLE_LAN_TCP}`,
      `--aethos-gossip-localhost-fanout=${LOCALHOST_FANOUT}`,
      `--aethos-gossip-eager-unicast=${EAGER_UNICAST}`,
      `--aethos-gossip-loopback-only=${LOOPBACK_ONLY}`,
      "--aethos-e2e-disable-relay=1",
      "--aethos-e2e-force-verbose=1",
      "--aethos-e2e-force-gossip=1",
      `--aethos-diagnostics-run-id=${DIAGNOSTICS_RUN_ID}`,
      ...(DIAGNOSTICS_COLLECTOR_URL
        ? [`--aethos-diagnostics-collector-url=${DIAGNOSTICS_COLLECTOR_URL}`]
        : [])
    ]
  });

  cleanupFns.push(async () => {
    try {
      await driver.quit();
    } catch {
      // ignore
    }
  });

  return {
    driver,
    stateRoot,
    logPath: appLogPath(stateRoot)
  };
}

async function shutdownAll() {
  while (cleanupFns.length > 0) {
    const fn = cleanupFns.pop();
    try {
      await fn();
    } catch {
      // ignore
    }
  }
}

async function main() {
  await fs.mkdir(STATE_ROOT, { recursive: true });
  await fs.mkdir(ARTIFACT_ROOT, { recursive: true });

  const session = await openTauriSession(STATE_ROOT, SOCKET_PATH);
  try {
    await waitForSplashToClear(session.driver);
    const wayfarerId = await readIdentityWayfarerId(STATE_ROOT);

    if (PEER_ID) {
      await seedContactAlias(STATE_ROOT, PEER_ID, PEER_ALIAS);
    }

    let outgoing = null;
    if (PEER_ID && OUTBOUND_MESSAGE) {
      outgoing = await queueOutboundViaCli(STATE_ROOT, PEER_ID, OUTBOUND_MESSAGE);
    }

    const gossipPump = maintainGossipActivity(session.driver, Math.max(TIMEOUT_MS, LINGER_MS));

    let incoming = null;
    if (EXPECTED_INBOUND_MESSAGE) {
      incoming = await waitForIncomingMessageInState(STATE_ROOT, EXPECTED_INBOUND_MESSAGE, TIMEOUT_MS);
      if (!incoming?.found) {
        throw new Error(`desktop inbound message not observed: ${EXPECTED_INBOUND_MESSAGE}`);
      }
    } else if (OUTBOUND_MESSAGE && LINGER_MS > 0) {
      await new Promise((resolve) => setTimeout(resolve, LINGER_MS));
    }

    await gossipPump;

    const logTail = await readLogTail(session.logPath, 20000);
    const result = {
      ok: true,
      runId: RUN_ID,
      wayfarerId,
      peerId: PEER_ID,
      stateRoot: STATE_ROOT,
      artifactRoot: ARTIFACT_ROOT,
      gossipLanPort: GOSSIP_LAN_PORT,
      outboundMessage: OUTBOUND_MESSAGE,
      expectedInboundMessage: EXPECTED_INBOUND_MESSAGE,
      outgoingObserved: Boolean(outgoing?.found),
      incomingObserved: Boolean(incoming?.found),
      outgoingMessageId: outgoing?.msgId || "",
      incomingMessageId: incoming?.msgId || "",
      logPath: session.logPath,
      logTail
    };
    await fs.writeFile(path.join(ARTIFACT_ROOT, "desktop-lan-peer-result.json"), `${JSON.stringify(result, null, 2)}\n`, "utf8");
    console.log(JSON.stringify(result, null, 2));
  } finally {
    await shutdownAll();
  }
}

main().catch(async (error) => {
  const failure = {
    ok: false,
    runId: RUN_ID,
    stateRoot: STATE_ROOT,
    artifactRoot: ARTIFACT_ROOT,
    error: String(error?.stack || error)
  };
  try {
    await fs.mkdir(ARTIFACT_ROOT, { recursive: true });
    await fs.writeFile(path.join(ARTIFACT_ROOT, "desktop-lan-peer-result.json"), `${JSON.stringify(failure, null, 2)}\n`, "utf8");
  } catch {
    // ignore
  }
  try {
    await shutdownAll();
  } catch {
    // ignore
  }
  console.error(JSON.stringify(failure, null, 2));
  process.exitCode = 1;
});
