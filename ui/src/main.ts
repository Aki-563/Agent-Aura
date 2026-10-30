import { invoke } from "@tauri-apps/api/core";
import "@fontsource-variable/inter";
import "./styles/app.css";

// Events from the app arrive through a polled queue (see `poll_events` in the app): Tauri's own
// event listener is not permitted by this build's capabilities.
type Handler<T> = (payload: T) => void;
const handlers = new Map<string, Handler<any>[]>();
let lastEventSeq = 0;
let polling = false;
function on<T>(name: string, handler: Handler<T>): void {
  handlers.set(name, [...(handlers.get(name) ?? []), handler]);
}
async function pollEvents(): Promise<void> {
  if (polling) return;
  polling = true;
  try {
    const reply = await invoke<{ last: number; events: { seq: number; event: string; payload: unknown }[] }>("poll_events", { after: lastEventSeq });
    // A restarted queue (smaller counter) starts over.
    lastEventSeq = reply.last < lastEventSeq ? 0 : reply.last;
    for (const item of reply.events) for (const handler of handlers.get(item.event) ?? []) handler(item.payload);
  } catch {
    // The app is busy or closing; the next poll tries again.
  } finally {
    polling = false;
  }
}
window.setInterval(() => void pollEvents(), 150);

// ---- Types -------------------------------------------------------------------------------------

type ApiProtocol = "open_ai_responses" | "open_ai_compatible_chat";

interface Preset {
  id: string;
  name: string;
  protocol: ApiProtocol;
  baseUrl: string;
  model: string;
  hasApiKey: boolean;
  active: boolean;
  coordinates: string | null;
}

interface Progress {
  phase: string;
  step: number;
  message: string;
  thought: string | null;
  actions: Record<string, unknown>[] | null;
}

interface RunSummary {
  id: string;
  prompt: string;
  status: string;
  summary: string | null;
  startedAt: string | null;
  completedAt: string | null;
  steps: number;
}

interface RunStep {
  index: number;
  thought: string;
  target: string;
  expected: string;
  action: Record<string, unknown> | null;
  outcome: string;
  window: string;
  screenshot: string | null;
}

interface Schedule {
  id: string;
  task: string;
  when: string;
  enabled: boolean;
  nextInSeconds: number;
}

// ---- Icons -------------------------------------------------------------------------------------

const svg = (body: string, cls = "i") => `<svg class="${cls}" viewBox="0 0 24 24" aria-hidden="true">${body}</svg>`;
const ICON = {
  gear: svg('<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>'),
  activity: svg('<path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2"/>'),
  arrow: svg('<path d="M12 19V5M5 12l7-7 7 7"/>', "i arrow"),
  stop: svg('<rect x="6.5" y="6.5" width="11" height="11" rx="2.5"/>', "i stop"),
  steps: svg('<path d="M4 16v-2.38C4 11.5 2.97 10.5 3 8c.03-2.72 1.49-6 4.5-6C9.37 2 10 3.8 10 5.5c0 3.11-2 5.66-2 8.68V16a2 2 0 1 1-4 0Z"/><path d="M20 20v-2.38c0-2.12 1.03-3.12 1-5.62-.03-2.72-1.49-6-4.5-6C14.63 6 14 7.8 14 9.5c0 3.11 2 5.66 2 8.68V20a2 2 0 1 0 4 0Z"/><path d="M16 17h4"/><path d="M4 13h4"/>'),
  speakerOn: svg('<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><path d="M16 9a5 5 0 0 1 0 6"/><path d="M19.364 18.364a9 9 0 0 0 0-12.728"/>', "i on"),
  speakerOff: svg('<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><path d="M22 9l-6 6M16 9l6 6"/>', "i off"),
  micOn: svg('<rect x="9" y="3" width="6" height="12" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3"/>', "i on"),
  micOff: svg('<rect x="9" y="3" width="6" height="12" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3M4 4l16 16"/>', "i off"),
  pause: svg('<rect x="6" y="5" width="4" height="14" rx="1.2"/><rect x="14" y="5" width="4" height="14" rx="1.2"/>'),
  play: svg('<path d="M7 5.5v13a1 1 0 0 0 1.5.86l10.6-6.5a1 1 0 0 0 0-1.72L8.5 4.64A1 1 0 0 0 7 5.5z"/>'),
  end: svg('<rect x="6" y="6" width="12" height="12" rx="2.5"/>'),
  trash: svg('<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>'),
  check: svg('<path d="M20 6 9 17l-5-5"/>'),
};

// ---- Markup ------------------------------------------------------------------------------------

const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `
  <section id="run-view" class="run-view" aria-live="polite">
    <div id="mini" class="mini">
      <div id="mini-bar" class="mini-bar">
        <span id="mini-orb" class="mini-orb phase-starting"></span>
        <div class="mini-text">
          <div id="mini-main" class="swap"><span class="swap-line current">Getting ready</span></div>
          <div class="mini-sub"><span id="mini-phase">Starting</span><span id="mini-step"></span></div>
        </div>
        <button id="mini-voice" class="mini-icon" type="button" aria-pressed="true" title="Step reader">${ICON.speakerOn}${ICON.speakerOff}</button>
      </div>
      <div class="mini-more">
        <div id="mini-more-inner" class="mini-more-inner">
          <div class="mini-actions">
            <button id="run-pause" class="mini-btn" type="button"><span id="run-pause-icon">${ICON.pause}</span><span id="run-pause-label">Pause</span></button>
            <button id="run-mic" class="mini-btn icon-only" type="button" aria-pressed="false" title="Voice input (coming soon)">${ICON.micOn}${ICON.micOff}</button>
            <button id="run-stop" class="mini-btn danger" type="button">${ICON.end}<span>End</span></button>
          </div>
          <form id="steer-form" class="steer" autocomplete="off">
            <input id="steer-text" type="text" placeholder="Tell Aura what to change…" maxlength="400" />
            <button id="steer-send" class="steer-send" type="submit" aria-label="Send" disabled>${ICON.arrow}</button>
          </form>
          <div id="confirm" class="run-confirm hidden" role="alertdialog" aria-live="assertive">
            <div id="confirm-title" class="run-confirm-title">Allow this action?</div>
            <div id="confirm-text" class="run-confirm-text"></div>
            <div class="run-confirm-buttons">
              <button id="confirm-deny" class="mini-btn" type="button">No</button>
              <button id="confirm-allow" class="mini-btn primary" type="button">Allow</button>
            </div>
          </div>
        </div>
      </div>
    </div>
  </section>

  <main id="home" class="home">
    <header class="topbar">
      <div class="brand"><span class="brand-orb"></span>Aura</div>
      <div class="top-actions">
        <button id="open-activity" class="icon-btn" type="button" title="Activity" aria-label="Activity">${ICON.activity}</button>
        <button id="status" class="status-pill" type="button" title="Model connection"><span class="status-dot"></span><span id="status-text">No model</span></button>
        <button id="open-settings" class="icon-btn" type="button" title="Settings" aria-label="Settings">${ICON.gear}</button>
      </div>
    </header>
    <section class="stage">
      <div class="orb" aria-hidden="true"></div>
      <form id="composer" class="composer" autocomplete="off">
        <textarea id="task" rows="1" placeholder="What should Aura do?" spellcheck="true"></textarea>
        <div class="composer-tools">
          <button id="steps-btn" class="steps-btn" type="button" aria-expanded="false" title="Maximum steps">${ICON.steps}<span id="steps-value">20</span></button>
          <button id="send" class="send-btn" type="submit" aria-label="Run" disabled>${ICON.arrow}${ICON.stop}</button>
        </div>
      </form>
    </section>
  </main>

  <div id="toast" class="toast" role="status"><span id="toast-text" class="toast-text"></span><button id="toast-action" class="toast-action" type="button"></button></div>

  <div id="settings" class="backdrop" aria-hidden="true">
    <div class="sheet" role="dialog" aria-label="Settings">
      <div class="sheet-head"><h2>Settings</h2><button class="text-btn" type="button" data-close>Done</button></div>
      <div class="sheet-body">
        <div class="group-label">Models</div>
        <div id="presets" class="group"></div>
        <div id="add-block">
        <div class="group-label">Add a model</div>
        <div class="group">
          <form id="model-form" class="form" autocomplete="off">
            <div id="protocol" class="segmented"><span class="thumb"></span><button type="button" data-protocol="open_ai_compatible_chat" class="on">Local server</button><button type="button" data-protocol="open_ai_responses">OpenAI</button></div>
            <label class="field"><span>Server</span><input id="base-url" spellcheck="false" placeholder="http://localhost:1234/v1" /></label>
            <label class="field"><span>Model</span><input id="model" spellcheck="false" placeholder="qwen/qwen3-vl-4b" /></label>
            <label class="field"><span id="key-label">API key (optional)</span><input id="api-key" type="password" spellcheck="false" placeholder="Saved encrypted on this PC" /></label>
            <div id="model-error" class="form-error"></div>
            <button id="connect" class="primary-btn" type="submit">Connect</button>
          </form>
        </div>
        </div>
        <div class="group-label">Voice</div>
        <div class="group">
          <div class="row"><div class="row-main"><div class="row-title">Read each step aloud</div></div><button id="voice-switch" class="switch" type="button" role="switch" aria-checked="true" aria-label="Read each step aloud"></button></div>
        </div>
        <div id="schedules-block" class="hidden">
          <div class="group-label">Scheduled</div>
          <div id="schedules" class="group"></div>
        </div>
      </div>
    </div>
  </div>

  <div id="activity" class="backdrop" aria-hidden="true">
    <div class="sheet wide" role="dialog" aria-label="Activity">
      <div class="sheet-head"><h2>Activity</h2><button class="text-btn" type="button" data-close>Done</button></div>
      <div class="sheet-body">
        <div id="runs" class="runs"></div>
        <div id="run-detail"></div>
      </div>
    </div>
  </div>
`;

function $<T extends HTMLElement = HTMLElement>(id: string): T {
  const value = document.getElementById(id);
  if (!value) throw new Error(`Missing UI element: ${id}`);
  return value as T;
}

function toMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return "Unknown error";
  }
}

const escapeHtml = (text: string) => text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

// ---- Toast -------------------------------------------------------------------------------------

let toastTimer: number | undefined;
let toastHandler: (() => void) | null = null;
function toast(text: string, options: { action?: string; onAction?: () => void; error?: boolean; ms?: number } = {}): void {
  const box = $("toast");
  $("toast-text").textContent = text;
  $("toast-action").textContent = options.action ?? "";
  toastHandler = options.onAction ?? null;
  box.classList.toggle("error", !!options.error);
  box.classList.add("show");
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => box.classList.remove("show"), options.ms ?? 4200);
}
$("toast-action").addEventListener("click", () => {
  $("toast").classList.remove("show");
  const handler = toastHandler;
  toastHandler = null;
  handler?.();
});

// ---- Connection status -----------------------------------------------------------------------

let presets: Preset[] = [];

function setStatus(state: "none" | "checking" | "online" | "offline", text: string, title = ""): void {
  const pill = $("status");
  pill.className = `status-pill ${state}`;
  $("status-text").textContent = text;
  pill.title = title || text;
}

function activePreset(): Preset | undefined {
  return presets.find((p) => p.active);
}

async function loadPresets(): Promise<void> {
  try {
    presets = await invoke<Preset[]>("list_model_presets");
  } catch (error) {
    presets = [];
    toast(toMessage(error), { error: true });
  }
  renderPresets();
}

async function checkConnection(): Promise<void> {
  const active = activePreset();
  if (!active) {
    setStatus("none", "No model", "Add a model in Settings");
    return;
  }
  setStatus("checking", active.name);
  try {
    const reply = await invoke<{ state: string; model?: string; message?: string }>("ping_provider");
    if (reply.state === "online") setStatus("online", active.name, `${active.model} · connected`);
    else if (reply.state === "offline") setStatus("offline", active.name, reply.message ?? "Not reachable");
    else setStatus("none", "No model", "Add a model in Settings");
  } catch (error) {
    setStatus("offline", active.name, toMessage(error));
  }
}

$("status").addEventListener("click", () => openSheet("settings"));

// ---- Sheets ------------------------------------------------------------------------------------

function openSheet(id: "settings" | "activity"): void {
  const sheet = $(id);
  sheet.setAttribute("aria-hidden", "false");
  sheet.classList.add("open");
  if (id === "settings") {
    void loadPresets();
    void refreshSchedules();
    void loadVoice();
  } else {
    void loadRuns();
  }
}

function closeSheet(sheet: HTMLElement): void {
  sheet.classList.remove("open");
  sheet.setAttribute("aria-hidden", "true");
}

for (const id of ["settings", "activity"]) {
  const sheet = $(id);
  sheet.addEventListener("mousedown", (event) => {
    if (event.target === sheet) closeSheet(sheet);
  });
  sheet.querySelector("[data-close]")!.addEventListener("click", () => closeSheet(sheet));
}
document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  for (const sheet of document.querySelectorAll<HTMLElement>(".backdrop.open")) closeSheet(sheet);
  closeStepsPop();
});
$("open-settings").addEventListener("click", () => openSheet("settings"));
$("open-activity").addEventListener("click", () => openSheet("activity"));

// ---- Settings: models --------------------------------------------------------------------------

let protocol: ApiProtocol = "open_ai_compatible_chat";
const calibrating = new Set<string>();

function setProtocol(next: ApiProtocol): void {
  protocol = next;
  const seg = $("protocol");
  seg.classList.toggle("second", next === "open_ai_responses");
  for (const b of seg.querySelectorAll<HTMLButtonElement>("button")) b.classList.toggle("on", b.dataset.protocol === next);
  const baseUrl = $<HTMLInputElement>("base-url");
  if (next === "open_ai_responses") {
    baseUrl.value = "https://api.openai.com/v1";
    $("key-label").textContent = "API key";
    $<HTMLInputElement>("model").placeholder = "Model ID";
  } else {
    if (baseUrl.value === "https://api.openai.com/v1") baseUrl.value = "";
    $("key-label").textContent = "API key (optional)";
    $<HTMLInputElement>("model").placeholder = "qwen/qwen3-vl-4b";
  }
}
for (const b of $("protocol").querySelectorAll<HTMLButtonElement>("button")) b.addEventListener("click", () => setProtocol(b.dataset.protocol as ApiProtocol));

function coordinatesLabel(preset: Preset): string {
  if (calibrating.has(preset.model)) return "";
  switch (preset.coordinates) {
    case "norm1000":
      return "0–1000 grid";
    case "norm1":
      return "0–1 grid";
    case "pixels":
      return "pixels";
    default:
      return "";
  }
}

let addOpen = false;

function renderPresets(): void {
  const list = $("presets");
  list.replaceChildren();
  $("add-block").classList.toggle("hidden", presets.length > 0 && !addOpen);
  if (presets.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "No models yet.";
    list.append(empty);
    return;
  }
  for (const preset of presets) {
    const row = document.createElement("div");
    row.className = `row clickable${preset.active ? " active" : ""}`;
    const host = preset.protocol === "open_ai_responses" ? "OpenAI" : preset.baseUrl.replace(/^https?:\/\//, "").replace(/\/v1\/?$/, "");
    const coords = coordinatesLabel(preset);
    row.innerHTML = `
      <span class="row-check">${preset.active ? ICON.check : ""}</span>
      <div class="row-main"><div class="row-title"></div><div class="row-meta"></div></div>
      ${calibrating.has(preset.model) ? '<span class="badge busy">Calibrating…</span>' : coords ? `<span class="badge" title="How this model writes coordinates">${coords}</span>` : ""}
      <button class="row-btn" type="button" title="Delete" aria-label="Delete">${ICON.trash}</button>`;
    row.querySelector(".row-title")!.textContent = preset.name;
    row.querySelector(".row-meta")!.textContent = `${preset.model} · ${host}`;
    const remove = row.querySelector<HTMLButtonElement>(".row-btn")!;
    remove.addEventListener("click", (event) => {
      event.stopPropagation();
      if (!remove.classList.contains("confirm")) {
        remove.classList.add("confirm");
        remove.textContent = "Delete";
        window.setTimeout(() => {
          if (remove.isConnected && remove.classList.contains("confirm")) renderPresets();
        }, 3000);
        return;
      }
      void deletePreset(preset);
    });
    row.addEventListener("click", () => {
      if (!preset.active) void usePreset(preset);
    });
    list.append(row);
  }
  if (!addOpen) {
    const add = document.createElement("div");
    add.className = "row clickable";
    add.innerHTML = '<span class="row-check row-add">+</span><div class="row-main"><div class="row-title row-add">Add model</div></div>';
    add.addEventListener("click", () => {
      addOpen = true;
      renderPresets();
      $<HTMLInputElement>("base-url").focus();
    });
    list.append(add);
  }
}

async function usePreset(preset: Preset): Promise<void> {
  setStatus("checking", preset.name);
  try {
    await invoke<Preset>("use_model_preset", { id: preset.id });
    await loadPresets();
    await checkConnection();
  } catch (error) {
    toast(toMessage(error), { error: true });
    await loadPresets();
    await checkConnection();
  }
}

async function deletePreset(preset: Preset): Promise<void> {
  try {
    await invoke("delete_model_preset", { id: preset.id });
  } catch (error) {
    toast(toMessage(error), { error: true });
  }
  await loadPresets();
  await checkConnection();
}

$("model-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const button = $<HTMLButtonElement>("connect");
  const error = $("model-error");
  error.textContent = "";
  button.disabled = true;
  button.textContent = "Connecting…";
  try {
    const preset = await invoke<Preset>("connect_model", {
      input: {
        protocol,
        baseUrl: $<HTMLInputElement>("base-url").value.trim(),
        model: $<HTMLInputElement>("model").value.trim(),
        apiKey: $<HTMLInputElement>("api-key").value,
      },
      name: null,
    });
    $<HTMLInputElement>("api-key").value = "";
    $<HTMLInputElement>("model").value = "";
    addOpen = false;
    await loadPresets();
    setStatus("online", preset.name, `${preset.model} · connected`);
    toast(`Connected to ${preset.name}`);
  } catch (e) {
    error.textContent = toMessage(e);
    await loadPresets();
    await checkConnection();
  } finally {
    button.disabled = false;
    button.textContent = "Connect";
  }
});

on<{ model: string; state: string; coordinates?: string; message?: string }>("model-calibration", (event) => {
  if (event.state === "running") calibrating.add(event.model);
  else calibrating.delete(event.model);
  if (event.state === "done") {
    for (const p of presets) if (p.model === event.model) p.coordinates = event.coordinates ?? p.coordinates;
  }
  renderPresets();
});

// ---- Settings: voice and schedules -------------------------------------------------------------

let voiceOn = true;
function renderVoice(): void {
  $("voice-switch").setAttribute("aria-checked", String(voiceOn));
  const mini = $("mini-voice");
  mini.setAttribute("aria-pressed", String(voiceOn));
  mini.title = voiceOn ? "Step reader on" : "Step reader off";
}
async function loadVoice(): Promise<void> {
  try {
    voiceOn = await invoke<boolean>("get_voice_enabled");
  } catch {
    // Keep the last known value.
  }
  renderVoice();
}
async function toggleVoice(): Promise<void> {
  voiceOn = !voiceOn;
  renderVoice();
  try {
    voiceOn = await invoke<boolean>("set_voice_enabled", { enabled: voiceOn });
  } catch (error) {
    voiceOn = !voiceOn;
    toast(toMessage(error), { error: true });
  }
  renderVoice();
}
$("voice-switch").addEventListener("click", () => void toggleVoice());
$("mini-voice").addEventListener("click", (event) => {
  event.stopPropagation();
  void toggleVoice();
});

function countdown(seconds: number): string {
  if (seconds < 90) return `in ${seconds} s`;
  if (seconds < 5400) return `in ${Math.round(seconds / 60)} min`;
  if (seconds < 172800) return `in ${Math.round(seconds / 3600)} h`;
  return `in ${Math.round(seconds / 86400)} days`;
}

async function refreshSchedules(): Promise<void> {
  let rows: Schedule[] = [];
  try {
    rows = await invoke<Schedule[]>("list_schedules");
  } catch {
    rows = [];
  }
  const live = rows.filter((r) => r.enabled);
  $("schedules-block").classList.toggle("hidden", live.length === 0);
  const list = $("schedules");
  list.replaceChildren();
  for (const row of live) {
    const item = document.createElement("div");
    item.className = "row";
    item.innerHTML = `<div class="row-main"><div class="row-title"></div><div class="row-meta"></div></div><button class="row-btn" type="button" title="Delete" aria-label="Delete">${ICON.trash}</button>`;
    item.querySelector(".row-title")!.textContent = row.task;
    item.querySelector(".row-meta")!.textContent = `${row.when} · next ${countdown(row.nextInSeconds)}`;
    item.querySelector("button")!.addEventListener("click", async () => {
      await invoke("remove_schedule", { id: row.id }).catch((e) => toast(toMessage(e), { error: true }));
      await refreshSchedules();
    });
    list.append(item);
  }
}

// ---- Composer ----------------------------------------------------------------------------------

const task = $<HTMLTextAreaElement>("task");
const send = $<HTMLButtonElement>("send");
let maxSteps = Number(localStorage.getItem("aura.maxSteps")) || 20;
let running = false;

function autosize(): void {
  task.style.height = "auto";
  task.style.height = `${Math.min(task.scrollHeight, 168)}px`;
  send.disabled = !running && task.value.trim().length === 0;
  $("home").classList.toggle("typing", task.value.length > 0);
}
task.addEventListener("input", autosize);
task.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    $<HTMLFormElement>("composer").requestSubmit();
  }
});
$("composer").addEventListener("submit", (event) => {
  event.preventDefault();
  if (running) void stopRun();
  else void submitTask();
});

function setRunningUi(on: boolean): void {
  running = on;
  send.classList.toggle("stop-mode", on);
  send.setAttribute("aria-label", on ? "Stop" : "Run");
  autosize();
}

// Maximum steps: an icon in the composer opens a small stepper.
function renderSteps(): void {
  $("steps-value").textContent = String(maxSteps);
  const out = document.getElementById("steps-out");
  if (out) out.textContent = String(maxSteps);
  for (const chip of document.querySelectorAll<HTMLButtonElement>("#steps-pop .chips button")) chip.classList.toggle("on", Number(chip.dataset.v) === maxSteps);
}
function setSteps(value: number): void {
  maxSteps = Math.max(1, Math.min(100, Math.round(value)));
  localStorage.setItem("aura.maxSteps", String(maxSteps));
  renderSteps();
}
function closeStepsPop(): void {
  const pop = document.getElementById("steps-pop");
  if (!pop) return;
  $("steps-btn").setAttribute("aria-expanded", "false");
  pop.classList.add("closing");
  window.setTimeout(() => pop.remove(), 180);
}
$("steps-btn").addEventListener("click", (event) => {
  event.stopPropagation();
  if (document.getElementById("steps-pop")) {
    closeStepsPop();
    return;
  }
  const pop = document.createElement("div");
  pop.id = "steps-pop";
  pop.className = "steps-pop";
  pop.innerHTML = `
    <div class="steps-pop-title">Maximum steps</div>
    <div class="stepper"><button type="button" data-d="-1" aria-label="Fewer">−</button><output id="steps-out"></output><button type="button" data-d="1" aria-label="More">+</button></div>
    <div class="chips">${[10, 20, 40, 80].map((v) => `<button type="button" data-v="${v}">${v}</button>`).join("")}</div>`;
  pop.addEventListener("click", (e) => e.stopPropagation());
  for (const b of pop.querySelectorAll<HTMLButtonElement>(".stepper button")) b.addEventListener("click", () => setSteps(maxSteps + Number(b.dataset.d)));
  for (const b of pop.querySelectorAll<HTMLButtonElement>(".chips button")) b.addEventListener("click", () => setSteps(Number(b.dataset.v)));
  $("composer").append(pop);
  $("steps-btn").setAttribute("aria-expanded", "true");
  renderSteps();
});
document.addEventListener("click", () => closeStepsPop());
renderSteps();

async function submitTask(): Promise<void> {
  const text = task.value.trim();
  if (!text) return;
  if (!activePreset()) {
    toast("Add a model first", { action: "Settings", onAction: () => openSheet("settings") });
    return;
  }
  // "every day at 9am check my inbox": saved as a schedule instead of running now.
  try {
    const scheduled = await invoke<{ id: string; task: string; when: string } | null>("schedule_from_prompt", { prompt: text });
    if (scheduled) {
      task.value = "";
      autosize();
      toast(`Scheduled · ${scheduled.when} · ${scheduled.task}`, {
        action: "Run now instead",
        ms: 7000,
        onAction: () => {
          void invoke("remove_schedule", { id: scheduled.id }).finally(() => void startRun(text));
        },
      });
      return;
    }
  } catch (error) {
    toast(toMessage(error), { error: true });
    return;
  }
  await startRun(text);
}

async function startRun(text: string): Promise<void> {
  setRunningUi(true);
  resetMini();
  document.body.classList.add("running");
  try {
    const response = await invoke<{ completed: boolean; stopped: boolean; stepsExecuted: number; message: string }>("run_autonomous_task", { task: text, maxSteps });
    task.value = "";
    const done = response.completed ? "Done" : response.stopped ? "Stopped" : "Not finished";
    toast(`${done} · ${response.stepsExecuted} step${response.stepsExecuted === 1 ? "" : "s"}`, { action: "View", ms: 8000, onAction: () => openSheet("activity") });
  } catch (error) {
    toast(toMessage(error), { error: true, ms: 7000 });
  } finally {
    document.body.classList.remove("running");
    setRunningUi(false);
    task.focus();
  }
}

async function stopRun(): Promise<void> {
  try {
    await invoke("stop_autonomous_task");
  } catch (error) {
    toast(toMessage(error), { error: true });
  }
}

// ---- Mini player -------------------------------------------------------------------------------

const PHASES: Record<string, { label: string; cls: string }> = {
  starting: { label: "Starting", cls: "phase-starting" },
  capturing: { label: "Looking", cls: "phase-capturing" },
  observing: { label: "Looking", cls: "phase-capturing" },
  proposing: { label: "Thinking", cls: "phase-thinking" },
  retrying: { label: "Thinking again", cls: "phase-thinking" },
  verifying: { label: "Checking the plan", cls: "phase-thinking" },
  acting: { label: "Acting", cls: "phase-acting" },
  settling: { label: "Waiting for the screen", cls: "phase-settling" },
  result: { label: "Checked", cls: "phase-capturing" },
  checking: { label: "Checking the result", cls: "phase-thinking" },
  paused: { label: "Paused", cls: "phase-settling" },
  confirm: { label: "Needs your OK", cls: "phase-settling" },
};

let miniOpen = false;
let steerOpen = false;
let paused = false;
let hasVoiceLine = false;
let collapseTimer: number | undefined;
let phaseClass = "phase-starting";

/** Shows `text` as the main line: every older line lifts away, the new one rises in. However
 *  fast the updates come, only one line is ever "current", so words never pile up. */
function setMiniText(text: string): void {
  const host = $("mini-main");
  const lines = [...host.querySelectorAll<HTMLElement>(".swap-line")];
  const newest = lines[lines.length - 1];
  if (newest && !newest.classList.contains("leaving") && newest.textContent === text) return;
  for (const line of lines) {
    if (line.classList.contains("leaving")) continue;
    line.classList.remove("entering", "current");
    line.classList.add("leaving");
    window.setTimeout(() => line.remove(), 420);
  }
  // Never more than one line on its way out.
  const leaving = host.querySelectorAll(".swap-line.leaving");
  for (let i = 0; i < leaving.length - 1; i++) leaving[i].remove();
  const next = document.createElement("span");
  next.className = "swap-line entering";
  next.textContent = text;
  next.title = text;
  host.append(next);
  const promote = () => {
    if (next.classList.contains("entering")) next.classList.replace("entering", "current");
  };
  requestAnimationFrame(() => requestAnimationFrame(promote));
  window.setTimeout(promote, 60);
}

function setMiniPhase(label: string, cls: string): void {
  if (cls !== phaseClass) {
    $("mini-orb").classList.replace(phaseClass, cls);
    phaseClass = cls;
  }
  const phase = $("mini-phase");
  if (phase.textContent === label) return;
  phase.classList.add("fading");
  window.setTimeout(() => {
    phase.textContent = label;
    phase.classList.remove("fading");
  }, 140);
}

function resetMini(): void {
  hasVoiceLine = false;
  miniOpen = false;
  $("mini").classList.remove("open");
  $("confirm").classList.add("hidden");
  setPausedUi(false);
  setMiniPhase("Starting", "phase-starting");
  setMiniText("Getting ready");
  $("mini-step").textContent = "";
  renderVoice();
}

function onProgress(progress: Progress): void {
  const info = PHASES[progress.phase] ?? { label: progress.phase, cls: "phase-starting" };
  if (!paused || progress.phase === "paused") setMiniPhase(info.label, info.cls);
  if (progress.step > 0) $("mini-step").textContent = `· Step ${progress.step}`;
  if (!hasVoiceLine && progress.message) setMiniText(progress.message.length > 80 ? info.label : progress.message);
}

on<Progress>("agent-progress", onProgress);
on<{ text: string }>("agent-voice", (event) => {
  hasVoiceLine = true;
  setMiniText(event.text);
});
on<{ id: string; message: string; kind?: string }>("agent-confirm", (event) => {
  showConfirmation(event.id, event.message, event.kind ?? "approval");
  openMini();
});

let pendingConfirmation: string | null = null;
function showConfirmation(id: string, message: string, kind: string): void {
  const help = kind === "help";
  $("confirm-title").textContent = help ? "Aura needs your help" : "Allow this action?";
  $("confirm-allow").textContent = help ? "Done, continue" : "Allow";
  $("confirm-deny").textContent = help ? "I can't" : "No, another way";
  pendingConfirmation = id;
  $("confirm-text").textContent = message;
  $("confirm").classList.remove("hidden");
  setMiniPhase("Needs your OK", "phase-settling");
}
async function answerConfirmation(allow: boolean): Promise<void> {
  const id = pendingConfirmation;
  pendingConfirmation = null;
  $("confirm").classList.add("hidden");
  if (!id) return;
  try {
    await invoke("answer_confirmation", { id, allow });
  } catch (error) {
    setMiniText(toMessage(error));
  }
  scheduleCollapse();
}
$("confirm-allow").addEventListener("click", () => void answerConfirmation(true));
$("confirm-deny").addEventListener("click", () => void answerConfirmation(false));

function setPausedUi(on: boolean): void {
  paused = on;
  $("run-pause-label").textContent = on ? "Resume" : "Pause";
  $("run-pause-icon").innerHTML = on ? ICON.play : ICON.pause;
  $("run-pause").classList.toggle("active", on);
  if (on) setMiniPhase("Paused", "phase-settling");
}
$("run-pause").addEventListener("click", async () => {
  try {
    await invoke(paused ? "resume_autonomous_task" : "pause_autonomous_task");
  } catch (error) {
    setMiniText(toMessage(error));
  }
});
$("run-stop").addEventListener("click", () => void stopRun());
$("run-mic").addEventListener("click", () => {
  const button = $("run-mic");
  const next = button.getAttribute("aria-pressed") !== "true";
  button.setAttribute("aria-pressed", String(next));
  localStorage.setItem("aura.mic", next ? "on" : "off");
});
$("run-mic").setAttribute("aria-pressed", String(localStorage.getItem("aura.mic") === "on"));

/** The window is sized to the player: the page tells the app how tall it needs to be. */
async function syncHeight(): Promise<void> {
  const height = Math.ceil($("mini").getBoundingClientRect().height) + 12;
  try {
    await invoke("set_mini_height", { height: miniOpen ? Math.max(height, 68) : 68 });
  } catch {
    // Cosmetic only.
  }
}

function openMini(): void {
  window.clearTimeout(collapseTimer);
  if (miniOpen) return;
  miniOpen = true;
  // Grow the (transparent) window to the full height first, then let the card unfold into it.
  const target = 54 + 12 + $("mini-more-inner").scrollHeight + 14;
  void invoke("set_mini_height", { height: target })
    .catch(() => undefined)
    .then(() => $("mini").classList.add("open"));
}

function scheduleCollapse(): void {
  window.clearTimeout(collapseTimer);
  collapseTimer = window.setTimeout(() => {
    const asking = !$("confirm").classList.contains("hidden");
    if (steerOpen || asking || !miniOpen) return;
    miniOpen = false;
    $("mini").classList.remove("open");
    window.setTimeout(() => {
      if (!miniOpen) void syncHeight();
    }, 520);
  }, 650);
}

const runView = $("run-view");
runView.addEventListener("mouseenter", openMini);
runView.addEventListener("mouseleave", scheduleCollapse);
new ResizeObserver(() => {
  if (miniOpen) void syncHeight();
}).observe($("mini-more-inner"));

// Drag the player by its bar; the app remembers where it was put down.
$("mini-bar").addEventListener("mousedown", (event) => {
  if (event.button !== 0 || (event.target as HTMLElement).closest("button")) return;
  void invoke("start_window_drag").catch(() => undefined);
});

// Typing a correction pauses the run and lets the box take the keyboard.
const steerText = $<HTMLInputElement>("steer-text");
steerText.addEventListener("input", () => ($<HTMLButtonElement>("steer-send").disabled = steerText.value.trim().length === 0));
steerText.addEventListener("focus", async () => {
  if (steerOpen) return;
  steerOpen = true;
  await invoke("set_steer_box_open", { open: true }).catch(() => undefined);
});
steerText.addEventListener("blur", () => void endSteering(false));
steerText.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    steerText.value = "";
    steerText.blur();
  }
});
$("steer-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const text = steerText.value.trim();
  if (!text) return;
  steerText.value = "";
  $<HTMLButtonElement>("steer-send").disabled = true;
  try {
    await invoke("steer_autonomous_task", { text });
    setMiniText("Got it, adjusting");
  } catch (error) {
    setMiniText(toMessage(error));
  }
  steerOpen = true;
  await endSteering(true);
  steerText.blur();
});
async function endSteering(sent: boolean): Promise<void> {
  if (!steerOpen) return;
  steerOpen = false;
  try {
    await invoke("set_steer_box_open", { open: false });
    // Leaving the box empty-handed resumes; a half-written message keeps the run paused.
    if (!sent && steerText.value.trim().length === 0) await invoke("resume_autonomous_task");
  } catch {
    // Best effort.
  }
  scheduleCollapse();
}

window.setInterval(async () => {
  if (!document.body.classList.contains("running")) return;
  try {
    const status = await invoke<{ paused: boolean; pauseReason: string; openSteerBox: boolean }>("run_control_status");
    if (status.paused !== paused) setPausedUi(status.paused);
    if (status.openSteerBox) {
      openMini();
      steerText.focus();
    }
  } catch {
    // Best effort.
  }
}, 250);
window.setInterval(() => {
  if (document.body.classList.contains("running")) void invoke("keep_run_panel_on_top").catch(() => undefined);
}, 2000);

// A run this page did not start (a schedule) still turns the window into the player.
let externalRun = false;
window.setInterval(async () => {
  try {
    const status = await invoke<{ running: boolean }>("run_control_status");
    if (status.running && !document.body.classList.contains("running")) {
      externalRun = true;
      for (const sheet of document.querySelectorAll<HTMLElement>(".backdrop.open")) closeSheet(sheet);
      resetMini();
      document.body.classList.add("running");
      setRunningUi(true);
    } else if (!status.running && externalRun) {
      externalRun = false;
      document.body.classList.remove("running");
      setRunningUi(false);
      toast("Scheduled task finished", { action: "View", ms: 8000, onAction: () => openSheet("activity") });
    }
  } catch {
    // Best effort.
  }
}, 1000);

// ---- Activity ----------------------------------------------------------------------------------

let runs: RunSummary[] = [];
let selectedRun: string | null = null;

async function loadRuns(): Promise<void> {
  try {
    runs = await invoke<RunSummary[]>("list_task_runs", { limit: 30 });
  } catch (error) {
    runs = [];
    toast(toMessage(error), { error: true });
  }
  const strip = $("runs");
  strip.replaceChildren();
  if (runs.length === 0) {
    $("run-detail").innerHTML = '<div class="empty">No tasks yet.</div>';
    return;
  }
  if (!selectedRun || !runs.some((r) => r.id === selectedRun)) selectedRun = runs[0].id;
  for (const run of runs) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = `run-chip${run.id === selectedRun ? " on" : ""}`;
    chip.innerHTML = `<span class="state-dot ${run.status}"></span><span></span>`;
    chip.lastElementChild!.textContent = run.prompt;
    chip.title = run.prompt;
    chip.addEventListener("click", () => {
      selectedRun = run.id;
      for (const c of strip.children) c.classList.toggle("on", c === chip);
      void showRun(run.id);
    });
    strip.append(chip);
  }
  await showRun(selectedRun);
}

function parseTime(value: string | null): number | null {
  if (!value) return null;
  const t = Date.parse(value.replace(" ", "T") + "Z");
  return Number.isNaN(t) ? null : t;
}

function duration(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

function describeAction(action: Record<string, unknown> | null): string {
  if (!action) return "No action";
  const type = String(action.type ?? "");
  switch (type) {
    case "click":
      return `${action.button && action.button !== "left" ? `${action.button} ` : ""}click`;
    case "double_click":
      return "Double-click";
    case "triple_click":
      return "Triple-click";
    case "type_text":
      return `Type “${String(action.text ?? "").slice(0, 60)}”${action.press_enter ? " ⏎" : ""}`;
    case "hotkey":
      return `Press ${Array.isArray(action.keys) ? action.keys.join(" + ") : ""}`;
    case "key_press":
      return `Press ${action.key}`;
    case "open_app":
      return `Open ${action.name}`;
    case "focus_window":
      return `Switch to ${action.name}`;
    case "open_url":
      return `Open ${action.url}`;
    case "scroll":
      return `Scroll ${Number(action.clicks) > 0 ? "up" : "down"}`;
    case "drag_and_drop":
      return "Drag";
    case "wait_settle":
    case "wait":
      return "Wait";
    case "done":
      return "Finish";
    case "fail":
      return "Give up";
    default:
      return type.replace(/_/g, " ") || "Action";
  }
}

/** How a step ended, as the harness recorded it ("succeeded", "failed", "uncertain", ...). */
function outcomeKind(outcome: string): "ok" | "bad" | "unsure" {
  if (outcome === "succeeded" || outcome === "success") return "ok";
  if (outcome === "failed" || outcome === "refused" || outcome === "error") return "bad";
  return "unsure";
}
const OUTCOME_LABEL = { ok: "Worked", bad: "Failed", unsure: "Unsure" };

const STATUS_LABEL: Record<string, string> = { completed: "Done", stopped: "Stopped", incomplete: "Not finished", failed: "Failed", running: "Running" };

let mistakeStep: number | null = null;

async function showRun(id: string): Promise<void> {
  const host = $("run-detail");
  let detail: { run: RunSummary; steps: RunStep[] };
  try {
    detail = await invoke("get_task_run_detail", { id });
  } catch (error) {
    host.innerHTML = `<div class="empty">${escapeHtml(toMessage(error))}</div>`;
    return;
  }
  if (selectedRun !== id) return;
  mistakeStep = null;
  const { run, steps } = detail;
  const start = parseTime(run.startedAt);
  const end = parseTime(run.completedAt);
  const total = start && end ? end - start : null;
  const good = steps.filter((s) => outcomeKind(s.outcome) === "ok").length;
  host.innerHTML = `
    <div class="run-head"><div class="run-task"></div><div class="run-summary"></div></div>
    <div class="stats">
      <div class="stat"><div class="stat-value">${STATUS_LABEL[run.status] ?? escapeHtml(run.status)}</div><div class="stat-label">Result</div></div>
      <div class="stat"><div class="stat-value">${steps.length}</div><div class="stat-label">Steps</div></div>
      <div class="stat"><div class="stat-value">${total != null ? duration(total) : "—"}</div><div class="stat-label">Time</div></div>
      <div class="stat"><div class="stat-value">${steps.length ? `${Math.round((good / steps.length) * 100)}%` : "—"}</div><div class="stat-label">Steps that worked</div></div>
    </div>
    <div class="steps"></div>
    <div class="feedback"></div>`;
  host.querySelector(".run-task")!.textContent = run.prompt;
  host.querySelector(".run-summary")!.textContent = run.summary ?? "";
  const strip = host.querySelector(".steps")!;
  steps.forEach((step, i) => {
    const card = document.createElement("div");
    card.className = "step";
    card.style.animationDelay = `${Math.min(i, 10) * 35}ms`;
    const kind = outcomeKind(step.outcome);
    card.innerHTML = `
      <div class="step-shot">${step.screenshot ? `<img loading="lazy" alt="" />` : ""}<span class="num">${step.index}</span><span class="result ${kind}">${OUTCOME_LABEL[kind]}</span></div>
      <div class="step-body"><div class="step-action"></div><div class="step-thought"></div></div>`;
    if (step.screenshot) card.querySelector("img")!.src = step.screenshot;
    const target = step.target ? ` · ${step.target}` : "";
    const what = describeAction(step.action);
    card.querySelector(".step-action")!.textContent = `${what.charAt(0).toUpperCase()}${what.slice(1)}${target}`;
    card.querySelector(".step-thought")!.textContent = step.thought;
    card.addEventListener("click", () => {
      mistakeStep = mistakeStep === step.index ? null : step.index;
      for (const c of strip.children) c.classList.toggle("selected", c === card && mistakeStep !== null);
      renderFeedback(run, steps);
    });
    strip.append(card);
  });
  renderFeedback(run, steps);
}

function renderFeedback(run: RunSummary, steps: RunStep[]): void {
  const box = document.querySelector<HTMLElement>("#run-detail .feedback");
  if (!box || steps.length === 0 || run.status === "running") {
    if (box) box.replaceChildren();
    return;
  }
  if (mistakeStep === null) {
    box.innerHTML = `<button class="pill-btn good" type="button">Remember this way</button><span class="feedback-hint">Tap a step that went wrong to correct it.</span>`;
    box.querySelector("button")!.addEventListener("click", () => void review({ runId: run.id, verdict: "perfect" }));
    return;
  }
  box.innerHTML = `<input class="note" placeholder="What should Aura do at step ${mistakeStep} instead?" /><button class="pill-btn bad" type="button">Save correction</button>`;
  const note = box.querySelector<HTMLInputElement>("input")!;
  note.focus();
  box.querySelector("button")!.addEventListener("click", () => {
    const step = steps.find((s) => s.index === mistakeStep);
    void review({ runId: run.id, verdict: "mistake", stepIndex: mistakeStep, forbiddenAction: step ? JSON.stringify(step.action) : "", correction: note.value });
  });
}

async function review(input: Record<string, unknown>): Promise<void> {
  try {
    const message = await invoke<string>("submit_task_review", { input });
    toast(message);
  } catch (error) {
    toast(toMessage(error), { error: true });
  }
}

// ---- Start -------------------------------------------------------------------------------------

autosize();
void loadVoice();
void loadPresets().then(checkConnection);
window.setInterval(() => {
  if (!running && document.visibilityState === "visible") void checkConnection();
}, 120_000);
task.focus();
