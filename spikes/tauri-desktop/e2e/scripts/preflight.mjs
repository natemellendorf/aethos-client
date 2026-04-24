import { spawnSync } from "node:child_process";

function hasBinary(name) {
  const probe = spawnSync("which", [name], { stdio: "pipe" });
  return probe.status === 0;
}

function runCommand(command, args, env = process.env) {
  const result = spawnSync(command, args, {
    stdio: "inherit",
    env,
    shell: false
  });
  if (result.status !== 0) {
    throw new Error(`command failed: ${command} ${args.join(" ")}`);
  }
}

function binaryWorks(command, args = ["--help"]) {
  const result = spawnSync(command, args, { stdio: "pipe", shell: false });
  const combinedOutput = `${String(result.stdout || "")}${String(result.stderr || "")}`;
  return {
    ok: result.status === 0,
    output: combinedOutput
  };
}

function ensureExplicitE2ERun() {
  const isExplicitE2ERun = process.env.AETHOS_E2E === "1";
  if (isExplicitE2ERun) {
    return true;
  }
  process.stdout.write("e2e preflight: AETHOS_E2E is not set to 1; skipping automatic dependency install\n");
  return false;
}

function ensureCargo() {
  if (!hasBinary("cargo")) {
    throw new Error("missing required binary 'cargo'. Install Rust toolchain to build the Tauri desktop app.");
  }
  process.stdout.write("e2e preflight OK: cargo present\n");
}

function ensurePlaywrightBrowsers() {
  const probe = binaryWorks("npx", ["playwright", "--version"]);
  if (!probe.ok) {
    throw new Error("Playwright CLI is unavailable. Run npm install in spikes/tauri-desktop/e2e.");
  }

  const shouldInstall = (process.env.AETHOS_E2E_AUTO_INSTALL_PLAYWRIGHT || "1") !== "0";
  if (!shouldInstall) {
    process.stdout.write("e2e preflight: skipping Playwright browser install (AETHOS_E2E_AUTO_INSTALL_PLAYWRIGHT=0)\n");
    return;
  }

  process.stdout.write("e2e preflight: ensuring Playwright Chromium runtime is installed\n");
  runCommand("npx", ["playwright", "install", "chromium"]);
}

try {
  if (!ensureExplicitE2ERun()) {
    process.exit(0);
  }
  ensureCargo();
  ensurePlaywrightBrowsers();
} catch (error) {
  process.stderr.write(`${String(error?.message || error)}\n`);
  process.exit(1);
}
