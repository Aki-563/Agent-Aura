import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ReviewModal } from "./components/ReviewModal";
import "./styles/hud.css";

type ApiProtocol = "open_ai_responses" | "open_ai_compatible_chat";

interface ProviderStatus {
  configured: boolean;
  protocol: ApiProtocol | null;
  baseUrl: string | null;
  model: string | null;
  hasApiKey: boolean;
}

interface ConnectionTestResponse {
  ok: boolean;
  message: string;
  modelsReturned: number | null;
}

interface AutonomousProgress {
  phase: "starting" | "capturing" | "observing" | "proposing" | "verifying" | "acting" | "settling" | "result" | "checking" | "paused" | "confirm" | "retrying" | "completed" | "stopped";
  step: number;
  message: string;
  thought: string | null;
  actions: Record<string, unknown>[] | null;
  screenshotDataUrl: string | null;
  screenshotWidth: number | null;
  screenshotHeight: number | null;
}

interface TaskRunReview {
  run: {
    id: string;
    prompt: string;
    normalizedIntent: string;
    status: string;
    summary: string | null;
  };
  steps: Array<{
    stepIndex: number;
    actionSummary: string;
    outcome: string;
    screenshotDataUrl: string | null;
  }>;
}

interface AutonomousResponse {
  completed: boolean;
  stopped: boolean;
  stepsExecuted: number;
  message: string;
}

const app = document.querySelector<HTMLDivElement>("#app")!;

app.innerHTML = `
  <section id="run-view" class="run-view" aria-live="polite">
    <div class="run-head">
      <span class="run-brand">Agent Aura</span>
      <span id="run-step" class="run-step">Step 0</span>
    </div>
    <div id="run-status" class="run-status phase-starting">
      <span class="run-dot"></span>
      <div>
        <div id="run-phase" class="run-phase">Starting</div>
        <div id="run-detail" class="run-detail">Getting ready…</div>
      </div>
    </div>
    <div class="run-block">
      <div class="run-label">Plan</div>
      <div id="run-thought" class="run-text">—</div>
    </div>
    <div class="run-block">
      <div class="run-label">Action</div>
      <div id="run-action" class="run-text run-mono">—</div>
    </div>
    <div class="run-block">
      <div class="run-label">What happened</div>
      <div id="run-result" class="run-text">—</div>
    </div>
    <div id="run-confirm" class="run-confirm hidden" role="alertdialog" aria-live="assertive">
      <div id="run-confirm-title" class="run-confirm-title">Allow this action?</div>
      <div id="run-confirm-text" class="run-confirm-text"></div>
      <div class="run-confirm-buttons">
        <button id="run-confirm-deny" class="button danger" type="button">No, choose another way</button>
        <button id="run-confirm-allow" class="button primary" type="button">Yes, allow it</button>
      </div>
    </div>
    <div id="run-steer" class="run-confirm hidden" role="dialog">
      <div class="run-confirm-title">Tell the agent what to do differently</div>
      <textarea id="run-steer-text" rows="3" placeholder="For example: wrong, go back · use the other tab · save it as PDF instead"></textarea>
      <div class="run-confirm-buttons">
        <button id="run-steer-cancel" class="button" type="button">Cancel</button>
        <button id="run-steer-send" class="button primary" type="button">Send and continue</button>
      </div>
    </div>
    <img id="run-shot" class="run-shot hidden" alt="Latest screen the agent saw" />
    <div id="run-log" class="run-log"></div>
    <div class="run-controls">
      <button id="run-pause" class="button" type="button">Pause (Pause key)</button>
      <button id="run-steer-open" class="button" type="button">Steer (Ctrl+Alt+Space)</button>
      <button id="run-stop" class="button danger run-stop" type="button">Stop (Esc)</button>
    </div>
  </section>

  <main class="shell">
    <header class="topbar">
      <div>
        <div class="eyebrow">WINDOWS DESKTOP AGENT</div>
        <h1>Agent Aura</h1>
        <p>Observe, plan, and execute real Windows tasks autonomously.</p>
      </div>
      <div id="connection-badge" class="badge offline"><span></span>Not configured</div>
    </header>

    <section class="panel settings-panel">
      <div class="section-heading">
        <div>
          <h2>Model connection</h2>
          <p>The API key is held only in Rust process memory and is cleared when the app exits.</p>
        </div>
        <button id="clear-provider" class="button ghost" type="button">Clear</button>
      </div>

      <div class="form-grid">
        <label>
          <span>API type</span>
          <select id="protocol">
            <option value="open_ai_responses">OpenAI Responses API</option>
            <option value="open_ai_compatible_chat">Local / OpenAI-compatible server</option>
          </select>
        </label>
        <label>
          <span>Model</span>
          <input id="model" autocomplete="off" placeholder="Enter the exact model ID" />
        </label>
        <label class="wide">
          <span>Base URL</span>
          <input id="base-url" autocomplete="off" value="https://api.openai.com/v1" />
        </label>
        <label class="wide">
          <span id="api-key-label">OpenAI API key</span>
          <input id="api-key" type="password" autocomplete="off" spellcheck="false" placeholder="Stored in memory only" />
        </label>
      </div>

      <div class="button-row">
        <button id="save-provider" class="button primary" type="button">Save & test connection</button>
      </div>
    </section>

    <section class="workspace">
      <div class="panel task-panel">
        <div class="section-heading">
          <div>
            <h2>Task</h2>
            <p>Agent Aura runs continuously without step approvals. Press Esc at any time to stop.</p>
          </div>
        </div>
        <label>
          <span>What should the agent do?</span>
          <textarea id="task" rows="5" placeholder="Describe a concrete task on the current Windows desktop"></textarea>
        </label>
        <div class="button-row run-row">
          <label class="step-limit">
            <span>Maximum steps</span>
            <input id="max-steps" type="number" min="1" max="30" value="12" />
          </label>
          <button id="stop-task" class="button danger" type="button" disabled>Stop</button>
          <button id="run-task" class="button primary" type="button">Run autonomously</button>
        </div>

        <div id="plan-card" class="plan-card hidden">
          <div class="plan-copy">
            <div class="eyebrow">MODEL PROPOSAL · ONE STEP</div>
            <h3 id="plan-thought"></h3>
            <div id="action-list" class="action-list"></div>
          </div>
        </div>
      </div>

      <aside class="panel observation-panel">
        <div class="section-heading">
          <div>
            <h2>Latest observation</h2>
            <p id="capture-meta">No screen captured yet.</p>
          </div>
        </div>
        <div class="preview-frame">
          <div id="preview-empty">The real screen capture will appear here.</div>
          <img id="screen-preview" class="hidden" alt="Latest captured Windows desktop" />
        </div>
      </aside>
    </section>

    <section class="panel activity-panel">
      <div class="section-heading">
        <div>
          <h2>Activity</h2>
          <p>Only real connection, capture, planning, and execution events are shown.</p>
        </div>
      </div>
      <div id="activity" class="activity" aria-live="polite"></div>
    </section>
  </main>
`;

const protocol = element<HTMLSelectElement>("protocol");
const model = element<HTMLInputElement>("model");
const baseUrl = element<HTMLInputElement>("base-url");
const apiKey = element<HTMLInputElement>("api-key");
const apiKeyLabel = element<HTMLElement>("api-key-label");
const task = element<HTMLTextAreaElement>("task");
const badge = element<HTMLElement>("connection-badge");
const planCard = element<HTMLElement>("plan-card");
const planThought = element<HTMLElement>("plan-thought");
const actionList = element<HTMLElement>("action-list");
const screenPreview = element<HTMLImageElement>("screen-preview");
const previewEmpty = element<HTMLElement>("preview-empty");
const captureMeta = element<HTMLElement>("capture-meta");
const activity = element<HTMLElement>("activity");

protocol.addEventListener("change", updateProtocolFields);
element<HTMLButtonElement>("save-provider").addEventListener("click", saveAndTestProvider);
element<HTMLButtonElement>("clear-provider").addEventListener("click", clearProvider);
element<HTMLButtonElement>("run-task").addEventListener("click", runAutonomousTask);
element<HTMLButtonElement>("stop-task").addEventListener("click", stopAutonomousTask);
element<HTMLButtonElement>("run-stop").addEventListener("click", stopAutonomousTask);
element<HTMLButtonElement>("run-confirm-allow").addEventListener("click", () => void answerConfirmation(true));
element<HTMLButtonElement>("run-confirm-deny").addEventListener("click", () => void answerConfirmation(false));
element<HTMLButtonElement>("run-pause").addEventListener("click", () => void togglePause());
element<HTMLButtonElement>("run-steer-open").addEventListener("click", () => void openSteerBox());
element<HTMLButtonElement>("run-steer-cancel").addEventListener("click", () => void closeSteerBox(false));
element<HTMLButtonElement>("run-steer-send").addEventListener("click", () => void closeSteerBox(true));
element<HTMLTextAreaElement>("run-steer-text").addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    void closeSteerBox(true);
  }
});
window.setInterval(() => void pollRunControls(), 250);

updateProtocolFields();
void refreshProviderStatus();
void listen<AutonomousProgress>("agent-progress", (event) => renderAutonomousProgress(event.payload));
void listen<{ id: string; message: string; kind?: string }>("agent-confirm", (event) =>
  showConfirmation(event.payload.id, event.payload.message, event.payload.kind ?? "approval"),
);

function element<T extends HTMLElement>(id: string): T {
  const value = document.getElementById(id);
  if (!value) throw new Error(`Missing UI element: ${id}`);
  return value as T;
}

function updateProtocolFields(): void {
  if (protocol.value === "open_ai_responses") {
    baseUrl.value = "https://api.openai.com/v1";
    apiKeyLabel.textContent = "OpenAI API key";
    apiKey.placeholder = "Required; stored in memory only";
  } else {
    if (baseUrl.value === "https://api.openai.com/v1" || baseUrl.value.trim() === "") {
      baseUrl.value = "http://localhost:11434/v1";
    }
    apiKeyLabel.textContent = "API key (optional for local servers)";
    apiKey.placeholder = "Leave empty when the local server does not require one";
  }
}

async function refreshProviderStatus(): Promise<void> {
  try {
    renderProviderStatus(await invoke<ProviderStatus>("get_provider_status"));
  } catch (error) {
    log("error", toMessage(error));
  }
}

async function saveAndTestProvider(): Promise<void> {
  const button = element<HTMLButtonElement>("save-provider");
  await withBusy(button, "Testing…", async () => {
    const status = await invoke<ProviderStatus>("configure_provider", {
      input: {
        protocol: protocol.value as ApiProtocol,
        baseUrl: baseUrl.value.trim(),
        model: model.value.trim(),
        apiKey: apiKey.value,
      },
    });
    apiKey.value = "";
    renderProviderStatus(status);
    log("info", `Configuration loaded for ${status.model} at ${status.baseUrl}.`);

    const result = await invoke<ConnectionTestResponse>("test_provider");
    const count = result.modelsReturned == null ? "" : ` ${result.modelsReturned} model(s) reported.`;
    log("success", `${result.message}${count}`);
    badge.className = "badge online";
    badge.innerHTML = "<span></span>Connected";
  });
}

async function clearProvider(): Promise<void> {
  try {
    renderProviderStatus(await invoke<ProviderStatus>("clear_provider"));
    apiKey.value = "";
    clearPlan();
    log("info", "Provider credentials and pending plan cleared from memory.");
  } catch (error) {
    log("error", toMessage(error));
  }
}

async function runAutonomousTask(): Promise<void> {
  const runButton = element<HTMLButtonElement>("run-task");
  const stopButton = element<HTMLButtonElement>("stop-task");
  const maxSteps = Number(element<HTMLInputElement>("max-steps").value);
  clearPlan();
  runButton.disabled = true;
  runButton.textContent = "Running…";
  stopButton.disabled = false;
  log("info", "Autonomous execution started. Press Esc to stop immediately.");
  resetRunView(maxSteps);
  document.body.classList.add("running");
  try {
    const response = await invoke<AutonomousResponse>("run_autonomous_task", {
      task: task.value,
      maxSteps,
    });
    log(response.completed ? "success" : "info", `${response.message} Steps executed: ${response.stepsExecuted}.`);
    if (response.completed) await showTaskReview();
  } catch (error) {
    log("error", toMessage(error));
  } finally {
    document.body.classList.remove("running");
    runButton.disabled = false;
    runButton.textContent = "Run autonomously";
    stopButton.disabled = true;
  }
}

// ---- Live run panel ------------------------------------------------------------------------

let pendingConfirmation: string | null = null;

function showConfirmation(id: string, message: string, kind: string): void {
  const help = kind === "help";
  element<HTMLElement>("run-confirm-title").textContent = help ? "The agent needs your help" : "Allow this action?";
  element<HTMLButtonElement>("run-confirm-allow").textContent = help ? "Done, continue" : "Yes, allow it";
  element<HTMLButtonElement>("run-confirm-deny").textContent = help ? "I can't" : "No, choose another way";
  pendingConfirmation = id;
  element<HTMLElement>("run-confirm-text").textContent = message;
  element<HTMLElement>("run-confirm").classList.remove("hidden");
  setRunStatus("confirm", "Waiting for your answer");
}

async function answerConfirmation(allow: boolean): Promise<void> {
  const id = pendingConfirmation;
  pendingConfirmation = null;
  element<HTMLElement>("run-confirm").classList.add("hidden");
  if (!id) return;
  try {
    await invoke("answer_confirmation", { id, allow });
    log("info", allow ? "You allowed the action." : "You declined the action.");
  } catch (error) {
    log("error", toMessage(error));
  }
}

const PHASE_LABELS: Record<string, { label: string; className: string }> = {
  starting: { label: "Starting", className: "phase-starting" },
  capturing: { label: "Taking a screenshot", className: "phase-capturing" },
  observing: { label: "Looking at the screen", className: "phase-capturing" },
  proposing: { label: "Thinking", className: "phase-thinking" },
  retrying: { label: "Thinking again", className: "phase-thinking" },
  verifying: { label: "Plan ready", className: "phase-thinking" },
  acting: { label: "Acting", className: "phase-acting" },
  settling: { label: "Waiting for the screen", className: "phase-settling" },
  result: { label: "Checked the result", className: "phase-capturing" },
  checking: { label: "Checking the result", className: "phase-thinking" },
  paused: { label: "Paused", className: "phase-settling" },
  confirm: { label: "Needs your OK", className: "phase-settling" },
};

let runMaxSteps = 0;

function resetRunView(maxSteps: number): void {
  runMaxSteps = maxSteps;
  setRunStatus("starting", "Getting ready…");
  element<HTMLElement>("run-step").textContent = `Step 0 of ${maxSteps}`;
  element<HTMLElement>("run-thought").textContent = "—";
  element<HTMLElement>("run-action").textContent = "—";
  element<HTMLElement>("run-result").textContent = "—";
  element<HTMLElement>("run-log").replaceChildren();
  element<HTMLImageElement>("run-shot").classList.add("hidden");
}

function setRunStatus(phase: string, detail: string): void {
  const info = PHASE_LABELS[phase] ?? { label: phase, className: "phase-starting" };
  element<HTMLElement>("run-status").className = `run-status ${info.className}`;
  element<HTMLElement>("run-phase").textContent = info.label;
  element<HTMLElement>("run-detail").textContent = detail;
}

/** Human-readable one-liner for a typed action. */
function describeAction(action: Record<string, unknown>): string {
  const type = String(action.type ?? "");
  const point = (x: unknown, y: unknown) => `(${x}, ${y})`;
  switch (type) {
    case "click":
      return `${action.button && action.button !== "left" ? `${action.button}-` : ""}click at ${point(action.x, action.y)}`;
    case "double_click":
      return `double-click at ${point(action.x, action.y)}`;
    case "drag_and_drop":
      return `drag ${point(action.from_x, action.from_y)} → ${point(action.to_x, action.to_y)}`;
    case "type_text":
      return `type “${String(action.text ?? "").slice(0, 80)}”${action.press_enter ? " + Enter" : ""}`;
    case "hotkey":
      return `press ${Array.isArray(action.keys) ? action.keys.join("+") : "?"}`;
    case "key_press":
      return `press ${action.key}`;
    case "open_app":
      return `open app “${action.name}”`;
    case "scroll":
      return `scroll ${Number(action.clicks) > 0 ? "up" : "down"} ${Math.abs(Number(action.clicks))}`;
    case "wait_settle":
      return "wait for the screen";
    case "done":
      return `finish: ${action.summary}`;
    case "fail":
      return `give up: ${action.reason}`;
    default:
      return JSON.stringify(action);
  }
}

function updateRunView(progress: AutonomousProgress): void {
  const stepText = progress.step > 0 ? `Step ${progress.step} of ${runMaxSteps}` : "Starting";
  element<HTMLElement>("run-step").textContent = stepText;
  setRunStatus(progress.phase, progress.message);
  if (progress.thought) element<HTMLElement>("run-thought").textContent = progress.thought;
  if (progress.actions?.length) {
    element<HTMLElement>("run-action").textContent = progress.actions.map(describeAction).join("; ");
  }
  if (progress.phase === "result") {
    element<HTMLElement>("run-result").textContent = progress.message.replace(/^Result: /, "");
  }
  if (progress.phase === "proposing") {
    element<HTMLElement>("run-result").textContent = "—";
  }
  if (progress.screenshotDataUrl) {
    const shot = element<HTMLImageElement>("run-shot");
    shot.src = progress.screenshotDataUrl;
    shot.classList.remove("hidden");
  }
  const row = document.createElement("div");
  row.textContent = `${new Date().toLocaleTimeString()}  ${progress.message}`;
  const runLog = element<HTMLElement>("run-log");
  runLog.prepend(row);
  while (runLog.childElementCount > 6) runLog.lastElementChild?.remove();
}

async function showTaskReview(): Promise<void> {
  const review = await invoke<TaskRunReview | null>("get_last_task_run");
  if (!review) return;
  const modal = new ReviewModal({
    taskId: review.run.id,
    taskPrompt: review.run.prompt,
    steps: review.steps.map((step) => ({
      stepIndex: step.stepIndex,
      thumbnailUrl: step.screenshotDataUrl ?? screenPreview.src,
      actionSummary: `${step.actionSummary} · ${step.outcome}`,
    })),
    onPerfect: () => {
      return saveReview({ runId: review.run.id, verdict: "perfect" });
    },
    onOptimize: (optimization) => {
      return saveReview({ runId: review.run.id, verdict: "optimize", optimization });
    },
    onMistake: (stepIndex, forbiddenAction, correction) => {
      return saveReview({
        runId: review.run.id,
        verdict: "mistake",
        stepIndex,
        forbiddenAction,
        correction,
      });
    },
  });
  modal.render(document.body);
}

async function saveReview(input: Record<string, unknown>): Promise<void> {
  try {
    const message = await invoke<string>("submit_task_review", { input });
    log("success", message);
  } catch (error) {
    log("error", toMessage(error));
    throw error;
  }
}

// ---- Pause, resume and steering (the run keeps its progress; nothing restarts from zero) ----

let runPaused = false;

async function togglePause(): Promise<void> {
  try {
    await invoke(runPaused ? "resume_autonomous_task" : "pause_autonomous_task");
  } catch (error) {
    log("error", toMessage(error));
  }
}

async function openSteerBox(): Promise<void> {
  try {
    await invoke("set_steer_box_open", { open: true });
  } catch (error) {
    log("error", toMessage(error));
  }
  element<HTMLElement>("run-steer").classList.remove("hidden");
  element<HTMLTextAreaElement>("run-steer-text").focus();
}

async function closeSteerBox(send: boolean): Promise<void> {
  const box = element<HTMLTextAreaElement>("run-steer-text");
  const text = box.value.trim();
  element<HTMLElement>("run-steer").classList.add("hidden");
  try {
    await invoke("set_steer_box_open", { open: false });
    if (send && text) {
      await invoke("steer_autonomous_task", { text });
      log("info", `New instruction sent: ${text}`);
    } else {
      await invoke("resume_autonomous_task");
    }
  } catch (error) {
    log("error", toMessage(error));
  }
  box.value = "";
}

async function pollRunControls(): Promise<void> {
  if (!document.body.classList.contains("running")) return;
  try {
    const status = await invoke<{ paused: boolean; pauseReason: string; openSteerBox: boolean }>("run_control_status");
    if (status.paused !== runPaused) {
      runPaused = status.paused;
      element<HTMLButtonElement>("run-pause").textContent = runPaused ? "Resume (Pause key)" : "Pause (Pause key)";
      log("info", runPaused ? `Paused: ${status.pauseReason}` : "Resumed: the agent looks at the screen again.");
    }
    if (status.openSteerBox) await openSteerBox();
  } catch {
    // The status poll is best effort; a failed poll changes nothing.
  }
}

async function stopAutonomousTask(): Promise<void> {
  try {
    await invoke("stop_autonomous_task");
    log("info", "Stop requested. Waiting for the current operation to return.");
  } catch (error) {
    log("error", toMessage(error));
  }
}

function renderAutonomousProgress(progress: AutonomousProgress): void {
  updateRunView(progress);
  log("info", `Step ${progress.step}: ${progress.message}`);
  if (progress.screenshotDataUrl) {
    screenPreview.src = progress.screenshotDataUrl;
    screenPreview.classList.remove("hidden");
    previewEmpty.classList.add("hidden");
    captureMeta.textContent = `${progress.screenshotWidth}×${progress.screenshotHeight} WebP · autonomous step ${progress.step}`;
  }
  if (progress.thought && progress.actions) {
    renderStep(progress.thought, progress.actions);
  }
}

function renderProviderStatus(status: ProviderStatus): void {
  if (!status.configured) {
    badge.className = "badge offline";
    badge.innerHTML = "<span></span>Not configured";
    return;
  }
  badge.className = "badge ready";
  badge.innerHTML = "<span></span>Configured";
  if (status.protocol) protocol.value = status.protocol;
  if (status.baseUrl) baseUrl.value = status.baseUrl;
  if (status.model) model.value = status.model;
}

function renderStep(thought: string, actions: Record<string, unknown>[]): void {
  planThought.textContent = thought;
  actionList.replaceChildren();
  for (const [index, action] of actions.entries()) {
    const row = document.createElement("div");
    row.className = "action-row";
    const number = document.createElement("span");
    number.className = "action-number";
    number.textContent = String(index + 1);
    const code = document.createElement("code");
    code.textContent = JSON.stringify(action);
    row.append(number, code);
    actionList.append(row);
  }
  planCard.classList.remove("hidden");
}

function clearPlan(): void {
  planCard.classList.add("hidden");
  planThought.textContent = "";
  actionList.replaceChildren();
}

async function withBusy(button: HTMLButtonElement, busyLabel: string, operation: () => Promise<void>): Promise<void> {
  const original = button.textContent ?? "";
  button.disabled = true;
  button.textContent = busyLabel;
  try {
    await operation();
  } catch (error) {
    log("error", toMessage(error));
  } finally {
    button.disabled = false;
    button.textContent = original;
  }
}

function log(kind: "info" | "success" | "error", message: string): void {
  const row = document.createElement("div");
  row.className = `activity-row ${kind}`;
  const time = document.createElement("time");
  time.textContent = new Date().toLocaleTimeString();
  const text = document.createElement("span");
  text.textContent = message;
  row.append(time, text);
  activity.prepend(row);
}

function toMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try { return JSON.stringify(error); } catch { return "Unknown error"; }
}
