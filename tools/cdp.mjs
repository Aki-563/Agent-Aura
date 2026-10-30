// Small DevTools-protocol driver for the running Agent Aura webview (started with
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222).
//   node tools/cdp.mjs eval "<javascript>"     prints the value (promises are awaited)
//   node tools/cdp.mjs shot <file.png>          screenshot of the page itself (works even though
//                                                Aura's windows are excluded from screen capture)
//   node tools/cdp.mjs errors <ms>              prints page exceptions and console errors for <ms>
const [command, arg] = process.argv.slice(2);
const pages = await (await fetch("http://127.0.0.1:9222/json")).json();
const page = pages.find((p) => p.type === "page") ?? pages[0];
const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
const events = [];
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  } else if (msg.method) events.push(msg);
};
await new Promise((r) => (ws.onopen = r));
const send = (method, params = {}) =>
  new Promise((res) => {
    const i = ++id;
    pending.set(i, res);
    ws.send(JSON.stringify({ id: i, method, params }));
  });
await send("Runtime.enable");
if (command === "eval") {
  const reply = await send("Runtime.evaluate", { expression: arg, awaitPromise: true, returnByValue: true });
  if (reply.result?.exceptionDetails) console.log("EXCEPTION", JSON.stringify(reply.result.exceptionDetails).slice(0, 600));
  else console.log(JSON.stringify(reply.result?.result?.value ?? null, null, 1));
} else if (command === "shot") {
  const reply = await send("Page.captureScreenshot", { format: "png" });
  const fs = await import("node:fs");
  fs.writeFileSync(arg, Buffer.from(reply.result.data, "base64"));
  console.log("saved", arg);
} else if (command === "errors") {
  await new Promise((r) => setTimeout(r, Number(arg ?? 3000)));
  for (const e of events)
    if (e.method === "Runtime.exceptionThrown" || (e.method === "Runtime.consoleAPICalled" && ["error", "warning"].includes(e.params.type)))
      console.log(e.method, JSON.stringify(e.params).slice(0, 500));
  console.log("done");
}
ws.close();
process.exit(0);
