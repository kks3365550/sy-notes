// C:\Users\kksjmj\Desktop\ai\sy-notes\benchmarks\measure_ipc_cdp.mjs
// Measure IPC round-trip latency for 50,000-character text input via WebView2 CDP

import http from 'node:http';

const CDP_PORT = process.env.CDP_PORT || 9222;
const ITERATIONS = 100;
const TEXT_LENGTH = 50000;

async function getDebuggerUrl() {
  const url = `http://127.0.0.1:${CDP_PORT}/json`;
  for (let attempt = 0; attempt < 30; attempt++) {
    try {
      const res = await fetch(url);
      if (res.ok) {
        const targets = await res.json();
        console.log(`Found ${targets.length} CDP targets.`);
        const page = targets.find(t => t.type === 'page' || t.url.includes('localhost') || t.url.includes('tauri') || t.url.includes('index.html'));
        if (page && page.webSocketDebuggerUrl) {
          return page.webSocketDebuggerUrl;
        }
        if (targets.length > 0 && targets[0].webSocketDebuggerUrl) {
          return targets[0].webSocketDebuggerUrl;
        }
      }
    } catch (e) {
      // wait and retry
    }
    await new Promise(r => setTimeout(r, 500));
  }
  throw new Error(`Could not find CDP target at ${url} after 30 attempts.`);
}

async function run() {
  console.log(`Connecting to CDP on port ${CDP_PORT}...`);
  const wsUrl = await getDebuggerUrl();
  console.log(`Target WebSocket URL: ${wsUrl}`);

  const ws = new WebSocket(wsUrl);

  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });

  console.log("Connected to WebView2 CDP session!");

  let nextId = 1;
  const sendCommand = (method, params = {}) => {
    return new Promise((resolve, reject) => {
      const id = nextId++;
      const handler = (evt) => {
        const msg = JSON.parse(evt.data);
        if (msg.id === id) {
          ws.removeEventListener('message', handler);
          if (msg.error) reject(msg.error);
          else resolve(msg.result);
        }
      };
      ws.addEventListener('message', handler);
      ws.send(JSON.stringify({ id, method, params }));
    });
  };

  // Expression to run inside WebView2:
  const scriptToEvaluate = `
    (async () => {
      const iterations = ${ITERATIONS};
      const textLen = ${TEXT_LENGTH};
      // Generate 50,000 characters
      const largeText = "가나다라마바사아자차카타파하1234567890abcdefghijklmnopqrstuvwxyz!@#$%^&*()_+=".repeat(Math.ceil(textLen / 77)).slice(0, textLen);

      // Warmup 5 iterations
      for (let i = 0; i < 5; i++) {
        if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
          await window.__TAURI_INTERNALS__.invoke("create_or_update_doc", {
            doc: {
              id: "warmup-" + i,
              title: "Warmup " + i,
              content_json: "[]",
              content_text: "Warmup text",
              created_at: Date.now(),
              updated_at: Date.now(),
              deleted_at: null,
              doc_type: "note"
            }
          });
        }
      }

      const timings = [];
      for (let i = 0; i < iterations; i++) {
        const docPayload = {
          id: "ipc-bench-" + (i % 10), // reuse a few IDs to stress update
          title: "IPC 50k Benchmark Iteration " + i,
          content_json: "[]",
          content_text: largeText,
          created_at: Date.now(),
          updated_at: Date.now(),
          deleted_at: null,
          doc_type: "note"
        };

        const t0 = performance.now();
        await window.__TAURI_INTERNALS__.invoke("create_or_update_doc", { doc: docPayload });
        const t1 = performance.now();
        timings.push(t1 - t0);
      }

      timings.sort((a, b) => a - b);
      const min = timings[0];
      const max = timings[timings.length - 1];
      const avg = timings.reduce((a, b) => a + b, 0) / timings.length;
      const p50 = timings[Math.floor(timings.length * 0.50)];
      const p90 = timings[Math.floor(timings.length * 0.90)];
      const p95 = timings[Math.floor(timings.length * 0.95)];
      const p99 = timings[Math.floor(timings.length * 0.99)];

      return {
        iterations,
        textLen: largeText.length,
        min,
        max,
        avg,
        p50,
        p90,
        p95,
        p99,
        all: timings
      };
    })()
  `;

  console.log(`Executing 50,000-char IPC benchmark in WebView2 (${ITERATIONS} iterations)...`);
  const evalResult = await sendCommand("Runtime.evaluate", {
    expression: scriptToEvaluate,
    awaitPromise: true,
    returnByValue: true
  });

  ws.close();

  if (evalResult.exceptionDetails) {
    console.error("Evaluation error:", evalResult.exceptionDetails);
    process.exit(1);
  }

  const result = evalResult.result.value;
  console.log("\n================ IPC BENCHMARK RESULTS ================");
  console.log(`Iterations:       ${result.iterations}`);
  console.log(`Payload text len: ${result.textLen.toLocaleString()} characters`);
  console.log(`Min latency:      ${result.min.toFixed(2)} ms`);
  console.log(`Average latency:  ${result.avg.toFixed(2)} ms`);
  console.log(`p50 latency:      ${result.p50.toFixed(2)} ms`);
  console.log(`p90 latency:      ${result.p90.toFixed(2)} ms`);
  console.log(`p95 latency:      ${result.p95.toFixed(2)} ms`);
  console.log(`p99 latency:      ${result.p99.toFixed(2)} ms`);
  console.log(`Max latency:      ${result.max.toFixed(2)} ms`);
  console.log("=======================================================");

  if (result.p95 < 16.0) {
    console.log(`[PASS] IPC round-trip p95 (${result.p95.toFixed(2)} ms) is strictly below 16ms!`);
  } else {
    console.log(`[FAIL] IPC round-trip p95 (${result.p95.toFixed(2)} ms) exceeds 16ms!`);
    process.exit(1);
  }
}

run().catch((err) => {
  console.error("Fatal error running IPC benchmark:", err);
  process.exit(1);
});
