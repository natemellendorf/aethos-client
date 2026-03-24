import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawn, spawnSync } from "node:child_process";
import { Builder, By, Capabilities, until } from "selenium-webdriver";

const __dirname = fileURLToPath(new URL(".", import.meta.url));
const E2E_DIR = path.resolve(__dirname, "..");
const DESKTOP_DIR = path.resolve(E2E_DIR, "..");
const TAURI_BIN = path.resolve(DESKTOP_DIR, "src-tauri", "target", "debug", "aethos");

const TARGET_WAYFARER_ID = String(process.env.AETHOS_SEND_WAYFARER_ID || process.argv[2] || "")
  .trim()
  .toLowerCase();
if (!/^[0-9a-f]{64}$/.test(TARGET_WAYFARER_ID)) {
  throw new Error("AETHOS_SEND_WAYFARER_ID or argv[2] must be a 64-char lowercase hex wayfarer_id");
}

const RUN_ID = `manual-send-${Date.now()}`;
const STATE_ROOT = path.resolve(DESKTOP_DIR, "e2e", "workdir", RUN_ID, "sender");
const ARTIFACT_ROOT = path.resolve(DESKTOP_DIR, "e2e", "artifacts", RUN_ID);
const DRIVER_PORT = Number(process.env.AETHOS_SEND_DRIVER_PORT || 4494);
const NATIVE_PORT = Number(process.env.AETHOS_SEND_NATIVE_PORT || 4495);
const MEDIA_FILE_OVERRIDE = String(process.env.AETHOS_SEND_MEDIA_FILE || "").trim();
const MEDIA_CAP_B64 = Number(process.env.AETHOS_SEND_MEDIA_MAX_ITEM_PAYLOAD_B64_BYTES || "65536");

const cleanupFns = [];

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

async function readLogTail(filePath, maxChars = 15000) {
  try {
    const raw = await fs.readFile(filePath, "utf8");
    return raw.length > maxChars ? raw.slice(raw.length - maxChars) : raw;
  } catch {
    return "";
  }
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

async function clickTab(driver, tabId) {
  await clickElement(driver, `[data-testid=\"tab-${tabId}\"]`);
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

async function openContactsAndAdd(driver, id, alias) {
  await clickTab(driver, "contacts");
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
      const matches = await driver.findElements(By.css(`[data-testid='chat-contact-${id}']`));
      return matches.length > 0;
    } catch {
      return false;
    }
  }, 20000, 400);
  if (!saved) throw new Error(`contact save did not surface in chats: ${id}`);
}

async function clickContactInChats(driver, wayfarerId) {
  await clickTab(driver, "chats");
  await clickElement(driver, `[data-testid='chat-contact-${wayfarerId}']`, 30000);
}

async function attachFileAndSend(driver, filePath, caption) {
  await clickTab(driver, "chats");
  const expectedFileName = path.basename(filePath);
  const composer = await driver.findElement(By.css("[data-testid='chat-composer']"));
  await composer.clear();
  await composer.sendKeys(caption);

  const input = await driver.findElement(By.css("[data-testid='chat-attachment-input']"));
  await input.sendKeys(filePath);

  const attachedAccepted = await waitFor(async () => {
    try {
      const attachedText = await driver.executeScript(
        "const row=document.querySelector('[data-testid=\"chat-attachment-input\"]')?.parentElement; const chip=row?.querySelector('div.rounded-md span.truncate'); return (chip?.textContent||'').trim();"
      );
      return attachedText === expectedFileName;
    } catch {
      return false;
    }
  }, 45000, 200);
  if (!attachedAccepted) {
    throw new Error(`attachment chip did not appear for ${expectedFileName}`);
  }

  const sendBtn = await driver.findElement(By.css("[data-testid='chat-send']"));
  await sendBtn.click();

  const attachedCleared = await waitFor(async () => {
    try {
      const attachedText = await driver.executeScript(
        "const row=document.querySelector('[data-testid=\"chat-attachment-input\"]')?.parentElement; const chip=row?.querySelector('div.rounded-md span.truncate'); return (chip?.textContent||'').trim();"
      );
      return attachedText.length === 0;
    } catch {
      return false;
    }
  }, 20000, 200);
  if (!attachedCleared) {
    throw new Error("attachment chip did not clear after send");
  }
}

async function waitForOutgoingMediaState(stateRoot, wayfarerId, expectedText, timeoutMs = 120000) {
  const chatPath = path.join(stateRoot, "chat-history.json");
  return waitFor(async () => {
    try {
      const chat = await readJsonFile(chatPath);
      const thread = chat?.threads?.[wayfarerId] || [];
      for (const msg of thread.slice().reverse()) {
        const text = String(msg?.text || "");
        if (msg?.direction !== "Outgoing") continue;
        if (!msg?.media) continue;
        if (expectedText && text !== expectedText) continue;
        const failedError =
          String(msg?.lastSyncError || "") ||
          String(msg?.outboundState?.failed?.error || "") ||
          String(msg?.media?.error || "");
        return {
          found: true,
          failed: Boolean(failedError),
          error: failedError,
          msgId: String(msg?.msgId || ""),
          transferId: String(msg?.media?.transferId || ""),
          outboundState: String(msg?.outboundState || "")
        };
      }
      return false;
    } catch {
      return false;
    }
  }, timeoutMs, 600);
}

function buildMediaFixture() {
  const outPath = path.resolve(ARTIFACT_ROOT, "manual-send-image.png");
  const scriptPath = path.resolve(E2E_DIR, "scripts", "generate-aethos-large-png.mjs");
  const result = spawnSync(
    "node",
    [scriptPath, "--out", outPath, "--seed", RUN_ID, "--width", "2200", "--height", "1600"],
    { cwd: DESKTOP_DIR, encoding: "utf8" }
  );
  if (result.status !== 0) {
    throw new Error(`failed generating image fixture: ${result.stderr || result.stdout}`);
  }
  return outPath;
}

async function seedCapabilities(stateRoot, targetId) {
  const mediaDir = path.join(stateRoot, "media");
  await fs.mkdir(mediaDir, { recursive: true });
  const cachePath = path.join(mediaDir, "capabilities-cache.json");
  const now = Date.now();
  const cache = {
    peers: {
      [targetId]: {
        mediaV1: true,
        maxItemPayloadB64Bytes: MEDIA_CAP_B64,
        updatedAtUnixMs: now
      }
    }
  };
  await fs.writeFile(cachePath, `${JSON.stringify(cache, null, 2)}\n`, "utf8");
}

async function main() {
  await fs.mkdir(STATE_ROOT, { recursive: true });
  await fs.mkdir(ARTIFACT_ROOT, { recursive: true });

  spawnSync("bash", ["-lc", "pkill -f 'src-tauri/target/debug/aethos' || true; pkill -f tauri-driver || true; pkill -f WebKitWebDriver || true"], {
    cwd: DESKTOP_DIR,
    stdio: "inherit"
  });

  const build = spawnSync("npx", ["tauri", "build", "--debug", "--no-bundle"], {
    cwd: DESKTOP_DIR,
    stdio: "inherit",
    shell: true
  });
  if (build.status !== 0) throw new Error("tauri build failed");

  await seedCapabilities(STATE_ROOT, TARGET_WAYFARER_ID);

  const driverProc = spawn("tauri-driver", ["--port", String(DRIVER_PORT), "--native-port", String(NATIVE_PORT)], {
    stdio: ["ignore", "inherit", "inherit"],
    env: process.env
  });
  cleanupFns.push(async () => {
    if (!driverProc.killed) driverProc.kill("SIGTERM");
  });

  await new Promise((resolve) => setTimeout(resolve, 1400));

  const env = {
    ...process.env,
    TAURI_AUTOMATION: "true",
    TAURI_WEBVIEW_AUTOMATION: "true",
    AETHOS_STATE_DIR: STATE_ROOT,
    XDG_DATA_HOME: STATE_ROOT,
    XDG_STATE_HOME: STATE_ROOT,
    AETHOS_E2E: "0",
    AETHOS_MEDIA_WIRE_BUCKET_SUSTAINED_BYTES_PER_MIN:
      process.env.AETHOS_MEDIA_WIRE_BUCKET_SUSTAINED_BYTES_PER_MIN || "67108864",
    AETHOS_MEDIA_WIRE_BUCKET_BURST_BYTES:
      process.env.AETHOS_MEDIA_WIRE_BUCKET_BURST_BYTES || "67108864"
  };

  const capabilities = new Capabilities();
  capabilities.setBrowserName("wry");
  capabilities.set("tauri:options", {
    application: TAURI_BIN,
    args: [
      `--aethos-state-dir=${STATE_ROOT}`,
      `--aethos-media-e2e-max-item-payload-b64-bytes=${String(MEDIA_CAP_B64)}`
    ],
    env
  });

  const driver = await new Builder()
    .usingServer(`http://127.0.0.1:${DRIVER_PORT}/`)
    .withCapabilities(capabilities)
    .build();
  cleanupFns.push(async () => {
    try {
      await driver.quit();
    } catch {
      // ignore
    }
  });

  try {
    await waitForSplashToClear(driver);
    await openContactsAndAdd(driver, TARGET_WAYFARER_ID, "Manual Target");
    await clickContactInChats(driver, TARGET_WAYFARER_ID);

    const mediaFile = MEDIA_FILE_OVERRIDE || buildMediaFixture();
    const caption = `manual-media-${Date.now()}`;
    await attachFileAndSend(driver, mediaFile, caption);

    const outgoing = await waitForOutgoingMediaState(STATE_ROOT, TARGET_WAYFARER_ID, caption, 180000);
    if (!outgoing?.found) {
      throw new Error("outgoing media message was not recorded in chat state");
    }
    if (outgoing.failed) {
      throw new Error(`outgoing media message entered failed state: ${outgoing.error}`);
    }

    const logPath = path.join(STATE_ROOT, "aethos-linux", "aethos-linux.log");
    const logTail = await readLogTail(logPath, 60000);
    if (logTail.includes("media_send_failed:")) {
      throw new Error("local sender log shows media_send_failed");
    }

    const statusEl = await driver.wait(until.elementLocated(By.css("[data-testid='status-text']")), 5000);
    const statusText = await statusEl.getText();
    const result = {
      ok: true,
      runId: RUN_ID,
      wayfarerId: TARGET_WAYFARER_ID,
      stateRoot: STATE_ROOT,
      artifactRoot: ARTIFACT_ROOT,
      mediaFile,
      caption,
      senderStatus: statusText,
      chatMessageId: outgoing.msgId,
      transferId: outgoing.transferId,
      logPath
    };
    await fs.writeFile(path.join(ARTIFACT_ROOT, "manual-send-result.json"), `${JSON.stringify(result, null, 2)}\n`, "utf8");
    console.log(JSON.stringify(result, null, 2));
  } finally {
    while (cleanupFns.length) {
      const fn = cleanupFns.pop();
      try {
        await fn();
      } catch {
        // ignore
      }
    }
    spawnSync("bash", ["-lc", "pkill -f 'src-tauri/target/debug/aethos' || true; pkill -f tauri-driver || true; pkill -f WebKitWebDriver || true"], {
      cwd: DESKTOP_DIR,
      stdio: "inherit"
    });
  }
}

main().catch((error) => {
  console.error(`manual send failed: ${String(error?.stack || error)}`);
  process.exit(1);
});
