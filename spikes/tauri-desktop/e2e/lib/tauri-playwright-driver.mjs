import fs from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";

import { PluginClient, TauriPage } from "@srsholmes/tauri-playwright";

const DEFAULT_WAIT_TIMEOUT_MS = 5000;

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(predicate, timeoutMs = DEFAULT_WAIT_TIMEOUT_MS, intervalMs = 100) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      const value = await predicate();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await sleep(intervalMs);
  }
  if (lastError) {
    throw lastError;
  }
  return false;
}

function selectorFromLocator(locator) {
  if (typeof locator === "string") return locator;
  if (locator && typeof locator.value === "string") return locator.value;
  throw new Error(`unsupported locator: ${JSON.stringify(locator)}`);
}

function guessMimeType(filePath) {
  const ext = path.extname(filePath).toLowerCase();
  switch (ext) {
    case ".png":
      return "image/png";
    case ".jpg":
    case ".jpeg":
      return "image/jpeg";
    case ".gif":
      return "image/gif";
    case ".webp":
      return "image/webp";
    case ".txt":
      return "text/plain";
    case ".json":
      return "application/json";
    default:
      return "application/octet-stream";
  }
}

function serializeScriptArg(arg) {
  if (arg instanceof CompatElement) {
    return {
      __compatElement: true,
      selector: arg.selector,
      index: arg.index
    };
  }
  return arg;
}

class CompatElement {
  constructor(driver, selector, index = null) {
    this.driver = driver;
    this.selector = selector;
    this.index = index;
  }

  locator() {
    let locator = this.driver.page.locator(this.selector);
    if (this.index !== null) {
      locator = locator.nth(this.index);
    }
    return locator;
  }

  async setValue(value) {
    await this.driver.page.evaluate(`(() => {
      const matches = Array.from(document.querySelectorAll(${JSON.stringify(this.selector)}));
      const el = matches[${this.index ?? 0}] ?? null;
      if (!el) throw new Error('element not found for setValue');
      const nextValue = ${JSON.stringify(String(value ?? ""))};
      const proto = el instanceof HTMLTextAreaElement
        ? HTMLTextAreaElement.prototype
        : el instanceof HTMLInputElement
          ? HTMLInputElement.prototype
          : null;
      const desc = proto ? Object.getOwnPropertyDescriptor(proto, 'value') : null;
      if (desc?.set) {
        desc.set.call(el, nextValue);
      } else {
        el.value = nextValue;
      }
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
      return true;
    })()`);
  }

  async click() {
    await this.locator().click();
  }

  async clear() {
    try {
      await this.setValue("");
    } catch {
      await this.locator().fill("");
    }
  }

  async sendKeys(...parts) {
    const text = parts.map((part) => String(part ?? "")).join("");
    if (parts.length === 1 && typeof parts[0] === "string" && existsSync(parts[0])) {
      const filePath = path.resolve(parts[0]);
      const buffer = await fs.readFile(filePath);
      await this.driver.page.setInputFiles(this.selector, [
        {
          name: path.basename(filePath),
          mimeType: guessMimeType(filePath),
          buffer
        }
      ]);
      return;
    }

    const locator = this.locator();
    try {
      const currentValue = await locator.inputValue();
      await this.setValue(`${currentValue}${text}`);
      return;
    } catch {
      // fall through for non-input elements
    }

    await locator.pressSequentially(text);
  }

  async getText() {
    return await this.locator().innerText();
  }

  async isDisplayed() {
    return await this.locator().isVisible();
  }
}

class CompatDriver {
  constructor({ page, client, child, socketPath }) {
    this.page = page;
    this.client = client;
    this.child = child;
    this.socketPath = socketPath;
    this.closed = false;
  }

  async wait(condition, timeoutMs = DEFAULT_WAIT_TIMEOUT_MS) {
    return await waitFor(() => condition(this), timeoutMs, 100);
  }

  async findElement(locator) {
    const selector = selectorFromLocator(locator);
    const count = await this.page.count(selector);
    if (count < 1) {
      throw new Error(`element not found: ${selector}`);
    }
    return new CompatElement(this, selector, 0);
  }

  async findElements(locator) {
    const selector = selectorFromLocator(locator);
    const count = await this.page.count(selector);
    return Array.from({ length: count }, (_, index) => new CompatElement(this, selector, index));
  }

  async executeScript(script, ...args) {
    const serializedArgs = JSON.stringify(args.map(serializeScriptArg));
    return await this.page.evaluate(`(() => {
      const __serialized = ${serializedArgs};
      const __resolve = (value) => {
        if (value && value.__compatElement) {
          const matches = Array.from(document.querySelectorAll(value.selector));
          return matches[value.index ?? 0] ?? null;
        }
        return value;
      };
      const __args = __serialized.map(__resolve);
      const arguments = __args;
      return (function () { ${script} }).apply(null, __args);
    })()`);
  }

  async takeScreenshot() {
    const buffer = await this.page.screenshot();
    return buffer.toString("base64");
  }

  async quit() {
    if (this.closed) return;
    this.closed = true;
    try {
      this.client.disconnect();
    } catch {
      // ignore disconnect errors
    }
    if (this.child && !this.child.killed) {
      this.child.kill("SIGTERM");
      await Promise.race([
        new Promise((resolve) => this.child.once("exit", resolve)),
        sleep(1500)
      ]);
      if (!this.child.killed) {
        this.child.kill("SIGKILL");
      }
    }
    if (this.socketPath && existsSync(this.socketPath)) {
      await fs.rm(this.socketPath, { force: true }).catch(() => {});
    }
  }
}

export const By = {
  css(selector) {
    return { using: "css selector", value: selector };
  }
};

export const until = {
  elementLocated(locator) {
    return async (driver) => {
      try {
        return await driver.findElement(locator);
      } catch {
        return false;
      }
    };
  },
  elementIsVisible(element) {
    return async () => {
      try {
        return (await element.isDisplayed()) ? element : false;
      } catch {
        return false;
      }
    };
  }
};

export async function launchTauriSession({
  application,
  args = [],
  cwd,
  env = {},
  socketPath,
  startupTimeoutMs = 45000
}) {
  if (!socketPath) {
    throw new Error("socketPath is required");
  }
  if (socketPath.length > 95) {
    throw new Error(`socketPath too long for unix socket: ${socketPath}`);
  }

  await fs.mkdir(path.dirname(socketPath), { recursive: true });
  if (existsSync(socketPath)) {
    await fs.rm(socketPath, { force: true });
  }

  const child = spawn(application, args, {
    cwd,
    env: {
      ...process.env,
      ...env,
      TAURI_PLAYWRIGHT_SOCKET: socketPath
    },
    stdio: ["ignore", "pipe", "pipe"]
  });

  const output = [];
  const onData = (chunk) => {
    output.push(String(chunk || ""));
    if (output.length > 200) {
      output.shift();
    }
  };
  child.stdout?.on("data", onData);
  child.stderr?.on("data", onData);

  let exited = false;
  child.once("exit", () => {
    exited = true;
  });

  const socketReady = await waitFor(async () => {
    if (exited) {
      throw new Error(`Tauri app exited before bridge socket became ready\n${output.join("")}`);
    }
    return existsSync(socketPath);
  }, startupTimeoutMs, 100);
  if (!socketReady) {
    throw new Error(`Timed out waiting for Playwright bridge socket: ${socketPath}\n${output.join("")}`);
  }

  const client = new PluginClient(socketPath);
  await client.connect();

  return new CompatDriver({
    page: new TauriPage(client),
    client,
    child,
    socketPath
  });
}
