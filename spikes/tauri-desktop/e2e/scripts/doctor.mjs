import { spawnSync } from "node:child_process";

function requireBinary(bin, installHint) {
  const probe = spawnSync("which", [bin], { stdio: "pipe" });
  if (probe.status !== 0) {
    throw new Error(`missing required binary '${bin}'. ${installHint}`);
  }
}

try {
  requireBinary("cargo", "Install Rust toolchain so the desktop app can be built for E2E.");
  requireBinary("node", "Install Node.js 18+ so the E2E harness can run.");
  process.stdout.write("e2e doctor OK\n");
} catch (error) {
  process.stderr.write(`${String(error.message || error)}\n`);
  process.exit(1);
}
