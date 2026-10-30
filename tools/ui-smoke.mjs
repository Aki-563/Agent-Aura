// Drives the running Agent Aura webview over the DevTools protocol: reports page errors, fills the
// task box, presses Run and prints what the page logs.
const pages = await (await fetch("http://127.0.0.1:9222/json")).json();
const ws = new WebSocket(pages[0].webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
const events = [];
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
  else if (msg.method) events.push(msg);
};
await new Promise((r) => (ws.onopen = r));
const send = (method, params = {}) => new Promise((res) => { const i = ++id; pending.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
const evalJs = async (expression) => (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result;
await send("Runtime.enable");
await send("Log.enable");
const task = process.argv[2] ?? "open notepad";
console.log("badge:", JSON.stringify((await evalJs("document.getElementById('connection-badge')?.textContent")).result?.value));
console.log("key field:", JSON.stringify((await evalJs("document.getElementById('api-key')?.value?.length")).result?.value));
console.log("model:", JSON.stringify((await evalJs("document.getElementById('model')?.value")).result?.value));
await evalJs(`(() => { const t = document.getElementById('task'); t.value = ${JSON.stringify(task)}; t.dispatchEvent(new Event('input', {bubbles:true})); document.getElementById('run-task').click(); })()`);
await new Promise((r) => setTimeout(r, Number(process.env.WAIT ?? 12000)));
console.log("activity:", JSON.stringify((await evalJs("[...document.querySelectorAll('#activity .activity-row')].map(r=>r.textContent).slice(0,8)")).result?.value));
console.log("mini:", JSON.stringify((await evalJs("document.getElementById('mini-main')?.textContent + ' | ' + document.getElementById('mini-sub')?.textContent")).result?.value));
for (const e of events) if (e.method === "Runtime.exceptionThrown" || (e.method === "Runtime.consoleAPICalled" && ["error", "warning"].includes(e.params.type)) || e.method === "Log.entryAdded")
  console.log("EVENT", e.method, JSON.stringify(e.params).slice(0, 400));
ws.close();
process.exit(0);
