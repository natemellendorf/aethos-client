#!/usr/bin/env node
/**
 * Send a test message to a peer via the running desktop's Playwright bridge.
 * Connects to the existing Playwright socket, no new desktop instance needed.
 *
 * Usage:
 *   node send-via-bridge.mjs \
 *     --peer-id 52f19a2f098f2d52aa3242eea2179c8152ae6ddeb0ab0c88decaef7dd6f5a3db \
 *     --message "Hello from desktop!"
 */

import { PluginClient, TauriPage } from "@srsholmes/tauri-playwright";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = fileURLToPath(new URL(".", import.meta.url));

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
const SOCKET_PATH = args["socket-path"] || "/tmp/aethos-pw-dev-desktop.sock";
const PEER_ID = String(args["peer-id"] || "").trim().toLowerCase();
const MESSAGE = String(args["message"] || "").trim();
const ALIAS = String(args["alias"] || "iOS Device");

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(predicate, timeoutMs = 30000, intervalMs = 250) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const value = await predicate();
    if (value) return value;
    await sleep(intervalMs);
  }
  return false;
}

async function normalizedIdText(value) {
  return String(value || "").replace(/\s+/g, "").trim().toLowerCase();
}

async function clickElement(page, selector, timeoutMs = 20000) {
  await waitFor(async () => {
    try {
      const visible = await page.isVisible(selector);
      return visible;
    } catch { return false; }
  }, timeoutMs, 200);
  await page.evaluate(`document.querySelector('${selector}')?.scrollIntoView({block: 'center'});`);
  await sleep(200);
  await page.click(selector, timeoutMs);
}

async function main() {
  console.log(`Connecting to Playwright bridge at ${SOCKET_PATH}...`);
  const client = new PluginClient(SOCKET_PATH);
  await client.connect();
  const page = new TauriPage(client);
  console.log("Connected.");

  // Wait for splash to clear
  console.log("Waiting for app to load...");
  await waitFor(async () => {
    try {
      const overlays = await page.evaluate(
        `Array.from(document.querySelectorAll('.fixed.inset-0.z-50')).filter(el => { const s=getComputedStyle(el); return s.display !== 'none' && s.visibility !== 'hidden'; }).length`
      );
      return Number(overlays) === 0;
    } catch { return false; }
  }, 30000, 300);
  console.log("App loaded.");

  // Add contact
  const normalizedPeerId = await normalizedIdText(PEER_ID);
  console.log(`Adding contact: ${normalizedPeerId}...`);

  // Click Contacts tab
  console.log("Clicking Contacts tab...");
  await clickElement(page, "[data-testid='tab-contacts']");
  await sleep(500);

  // Fill wayfarer ID
  const idInput = await page.isVisible("[data-testid='contact-wayfarer-id']");
  if (idInput) {
    console.log("Filling wayfarer ID...");
    await page.fill("[data-testid='contact-wayfarer-id']", normalizedPeerId);
    await sleep(200);
  }

  // Fill alias
  const aliasInput = await page.isVisible("[data-testid='contact-alias']");
  if (aliasInput) {
    console.log("Filling alias...");
    await page.fill("[data-testid='contact-alias']", ALIAS);
    await sleep(200);
  }

  // Click Save
  const saveBtn = await page.isVisible("[data-testid='contact-save']");
  if (saveBtn) {
    console.log("Saving contact...");
    await page.evaluate(`document.querySelector("[data-testid='contact-save']")?.click()`);
    await sleep(1000);
  }

  // Go to Chats tab
  console.log("Going to Chats tab...");
  await clickElement(page, "[data-testid='tab-chats']");
  await sleep(500);

  // Find and click the contact in chat list
  const contactSelector = `[data-testid='chat-contact-${normalizedPeerId}']`;
  const contactExists = await waitFor(async () => {
    try { return await page.isVisible(contactSelector); }
    catch { return false; }
  }, 10000, 300);

  if (contactExists) {
    console.log("Clicking contact in chat list...");
    await clickElement(page, contactSelector);
    await sleep(500);
  } else {
    console.log("Contact not found in chat list, trying to select from contacts tab again...");
    await clickElement(page, "[data-testid='tab-contacts']");
    await sleep(300);
    await clickElement(page, "[data-testid='tab-chats']");
    await sleep(1000);
  }

  // Send message
  if (MESSAGE) {
    console.log(`Sending message: "${MESSAGE}"...`);
    const composer = await page.isVisible("[data-testid='chat-composer']");
    if (composer) {
      await page.fill("[data-testid='chat-composer']", MESSAGE);
      await sleep(300);
      await page.evaluate(`document.querySelector("[data-testid='chat-send']")?.click()`);
      console.log("Message sent!");
    } else {
      console.log("Chat composer not found, trying alternative approach...");
      // Try clicking in the general area
      const textareas = await page.evaluate(
        `document.querySelectorAll('textarea, input[type="text"], [contenteditable="true"]').length`
      );
      console.log(`Found ${textareas} input elements`);
      if (Number(textareas) > 0) {
        const firstInput = await page.evaluate(
          `document.querySelector('textarea, input[type="text"], [contenteditable="true"]')?.tagName`
        );
        console.log(`First input type: ${firstInput}`);
      }
    }
  } else {
    console.log("No message specified. Contact added but no message sent.");
  }

  // Get current URL/title for verification
  try {
    const title = await page.title();
    const url = await page.url();
    console.log(`\nDone. Page title: "${title}" URL: "${url}"`);
  } catch { /* ignore */ }

  client.disconnect();
  console.log("\nBridge session complete.");
}

main().catch(async (err) => {
  console.error("Error:", err);
  process.exitCode = 1;
});
