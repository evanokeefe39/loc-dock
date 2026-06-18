#!/usr/bin/env node
/**
 * Downloads LiteLLM's model pricing JSON and saves it as a bundled resource.
 *
 * URL: https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
 * Output: loc-dock-tauri/src-tauri/resources/pricing/litellm.json
 *
 * Run via: node scripts/download-litellm-pricing.mjs
 * Wired into: npm run prebuild / npm run pretauri
 */

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, "..");
const OUTPUT = resolve(ROOT, "src-tauri", "resources", "pricing", "litellm.json");
const URL = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

async function main() {
  console.log(`[litellm-pricing] Downloading from ${URL}`);
  const resp = await fetch(URL);

  if (!resp.ok) {
    throw new Error(`HTTP ${resp.status}: ${resp.statusText}`);
  }

  const text = await resp.text();
  const bytes = Buffer.byteLength(text, "utf8");

  // Validate it's parseable JSON (basic sanity check)
  try {
    JSON.parse(text);
  } catch {
    throw new Error("Downloaded file is not valid JSON");
  }

  // Ensure output dir exists
  mkdirSync(dirname(OUTPUT), { recursive: true });

  // Write pretty-printed
  writeFileSync(OUTPUT, JSON.stringify(JSON.parse(text), null, 2), "utf8");

  console.log(`[litellm-pricing] Written ${OUTPUT}`);
  console.log(`[litellm-pricing] ${(bytes / 1024 / 1024).toFixed(2)} MB (${(bytes / 1024).toFixed(0)} KB)`);
}

main().catch((err) => {
  console.error(`[litellm-pricing] FAILED: ${err.message}`);
  process.exit(1);
});
