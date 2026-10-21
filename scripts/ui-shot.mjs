// Screenshot and status probe for the Web UI using Chrome's DevTools protocol.
//
// Usage: node scripts/ui-shot.mjs <url> <output.png> [waitMs] [width] [height]
// Requires google-chrome/chromium on PATH and Node >= 22 (global WebSocket).

import { spawn } from "node:child_process";
import { writeFile } from "node:fs/promises";

const url = process.argv[2] ?? "http://localhost:4173/";
const output = process.argv[3] ?? "/tmp/geogenesis-ui.png";
const waitMs = Number(process.argv[4] ?? 8000);
const width = Number(process.argv[5] ?? 1680);
const height = Number(process.argv[6] ?? 1050);
const port = 9333 + Math.floor(Math.random() * 400);
const profile = `/tmp/gg-chrome-${port}`;

const chrome = spawn(
  "google-chrome",
  [
    "--headless=new",
    "--no-sandbox",
    "--disable-dev-shm-usage",
    "--enable-unsafe-swiftshader",
    "--use-gl=angle",
    "--use-angle=swiftshader",
    "--hide-scrollbars",
    `--window-size=${width},${height}`,
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`,
    url,
  ],
  { stdio: "ignore" },
);

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function targets() {
  for (let i = 0; i < 100; i += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`);
      const list = await response.json();
      const page = list.find((entry) => entry.type === "page");
      if (page !== undefined) return page;
    } catch {
      // Chrome is still starting.
    }
    await sleep(100);
  }
  throw new Error("Chrome did not expose a page target");
}

const page = await targets();
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

let nextId = 1;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (message.id !== undefined) {
    pending.get(message.id)?.(message);
    pending.delete(message.id);
  }
});
function send(method, params = {}) {
  const id = nextId++;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve) => pending.set(id, resolve));
}

await send("Runtime.enable");
await send("Page.enable");
await sleep(waitMs);

const action = process.argv[7];
if (action !== undefined && action.length > 0) {
  await send("Runtime.evaluate", { expression: action, awaitPromise: true, returnByValue: true });
  await sleep(Number(process.argv[8] ?? 6000));
}

const probe = await send("Runtime.evaluate", {
  expression: `(() => {
    const status = document.getElementById('hud-status')?.textContent ?? '';
    const time = document.getElementById('hud-time')?.textContent ?? '';
    const error = document.querySelector('#toast:not([hidden])')?.textContent ?? '';
    const canvas = document.querySelector('#viewport canvas');
    return JSON.stringify({
      status, time, error,
      canvas: canvas ? [canvas.width, canvas.height] : null,
      panels: document.querySelectorAll('.param').length,
      ticks: document.querySelectorAll('.timeline-frame-tick').length,
    });
  })()`,
  returnByValue: true,
});
console.log(probe.result?.result?.value ?? JSON.stringify(probe));

const shot = await send("Page.captureScreenshot", { format: "png" });
if (shot.result?.data) {
  await writeFile(output, Buffer.from(shot.result.data, "base64"));
  console.log(`screenshot: ${output}`);
}
socket.close();
chrome.kill("SIGTERM");
await sleep(400);
process.exit(0);
