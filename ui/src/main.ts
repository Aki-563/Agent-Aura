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
    <div id="mini" class="mini">
      <div class="mini-bar">
        <span id="mini-dot" class="mini-dot phase-starting"></span>
        <div class="mini-text">
          <div id="mini-main" class="swap"><span class="swap-line current">Getting ready</span></div>
          <div id="mini-sub" class="mini-sub">Starting</div>
        </div>
      </div>
      <div id="mini-more" class="mini-more">
        <div class="mini-more-inner">
          <div class="mini-actions">
            <button id="run-pause" class="mini-btn" type="button">
              <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14M16 5v14" /></svg><span id="run-pause-label">Pause</span>
            </button>
            <button id="run-mic" class="mini-btn icon-only" type="button" aria-pressed="false" title="Voice input (coming soon)">
              <svg class="mic-on" viewBox="0 0 24 24" aria-hidden="true"><rect x="9" y="3" width="6" height="12" rx="3" /><path d="M5 11a7 7 0 0 0 14 0M12 18v3" /></svg>
              <svg class="mic-off" viewBox="0 0 24 24" aria-hidden="true"><rect x="9" y="3" width="6" height="12" rx="3" /><path d="M5 11a7 7 0 0 0 14 0M12 18v3M4 4l16 16" /></svg>
            </button>
            <button id="run-stop" class="mini-btn danger" type="button">
              <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="2" /></svg><span>End task</span>
            </button>
          </div>
          <form id="run-steer-form" class="steer" autocomplete="off">
            <input id="run-steer-text" type="text" placeholder="Tell Aura what to change…" maxlength="400" />
            <button id="run-steer-send" class="steer-send" type="submit" aria-label="Send" disabled>
              <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 19V5M5 12l7-7 7 7" /></svg>
            </button>
          </form>
          <div id="run-confirm" class="run-confirm hidden" role="alertdialog" aria-live="assertive">
            <div id="run-confirm-title" class="run-confirm-title">Allow this action?</div>
            <div id="run-confirm-text" class="run-confirm-text"></div>
            <div class="run-confirm-buttons">
              <button id="run-confirm-deny" class="mini-btn" type="button">No, choose another way</button>
              <button id="run-confirm-allow" class="mini-btn primary" type="button">Yes, allow it</button>
            </div>
          </div>
        </div>
      </div>
    </div>
    <!-- Details the progress handlers still fill in; not shown in the mini player. -->
    <div class="run-legacy hidden">
      <span id="run-step"></span><div id="run-status"></div><span id="run-phase"></span><span id="run-detail"></span>
      <div id="run-thought"></div><div id="run-action"></div><div id="run-result"></div>
      <img id="run-shot" alt="" /><div id="run-log"></div>
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
          <p>Settings are saved on this computer. The API key is encrypted with Windows (only your Windows account can read it) and is never written to logs.</p>
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
          <input id="api-key" type="password" autocomplete="off" spellcheck="false" placeholder="Saved encrypted on this computer" />
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
element<HTMLButtonElement>("run-mic").addEventListener("click", toggleMic);
initMiniPlayer();
window.setInterval(() => void pollRunControls(), 250);
window.setInterval(() => void keepOnTop(), 2000);

updateProtocolFields();
void refreshProviderStatus();
void listen<{ text: string }>("agent-voice", (event) => {
  hasVoiceLine = true;
  setMiniText(event.payload.text);
});
void listen<AutonomousProgress>("agent-progress", (event) => renderAutonomousProgress(event.payload));
void listen<{ id: string; message: string; kind?: string }>("agent-confirm", (event) =>
  { showConfirmation(event.payload.id, event.payload.message, event.payload.kind ?? "approval"); openMini(); },
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
    apiKey.placeholder = "Required; saved encrypted on this computer";
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
    log("info", "Saved provider settings, the stored API key and the pending plan were deleted.");
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
let hasVoiceLine = false;

function resetRunView(maxSteps: number): void {
  runMaxSteps = maxSteps;
  hasVoiceLine = false;
  miniOpen = false;
  element<HTMLElement>("mini").classList.remove("open");
  setPausedUi(false);
  setRunStatus("starting", "Getting ready");
  element<HTMLElement>("run-step").textContent = `Step 0 of ${maxSteps}`;
  element<HTMLElement>("run-thought").textContent = "—";
  element<HTMLElement>("run-action").textContent = "—";
  element<HTMLElement>("run-result").textContent = "—";
  element<HTMLElement>("run-log").replaceChildren();
  element<HTMLImageElement>("run-shot").classList.add("hidden");
}

function setRunStatus(phase: string, detail: string): void {
  const info = PHASE_LABELS[phase] ?? { label: phase, className: "phase-starting" };
  setMiniPhase(info.className);
  if (!runPaused || phase === "paused") setMiniSub(info.label);
  if (!hasVoiceLine) setMiniText(detail.length > 70 ? info.label : detail);
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

// ---- Mini player: pause, steer, mic, changing words ----------------------------------------

let runPaused = false;
let steerOpen = false;
let miniOpen = false;
let collapseTimer: number | undefined;
let lastPhaseClass = "phase-starting";

async function togglePause(): Promise<void> {
  try {
    await invoke(runPaused ? "resume_autonomous_task" : "pause_autonomous_task");
  } catch (error) {
    log("error", toMessage(error));
  }
}

function setPausedUi(paused: boolean): void {
  runPaused = paused;
  element<HTMLElement>("run-pause-label").textContent = paused ? "Resume" : "Pause";
  element<HTMLElement>("run-pause").classList.toggle("active", paused);
  if (paused) {
    setMiniSub("Paused");
  }
}

/** Shows `text` as the main line. The old line lifts away and blurs while the new one rises in. */
function setMiniText(text: string): void {
  const host = element<HTMLElement>("mini-main");
  const current = host.querySelector<HTMLElement>(".swap-line.current");
  if (current && current.textContent === text) return;
  const next = document.createElement("span");
  next.className = "swap-line entering";
  next.textContent = text;
  host.appendChild(next);
  current?.classList.replace("current", "leaving");
  // Two frames so the browser registers the starting state before the transition begins.
  requestAnimationFrame(() => requestAnimationFrame(() => next.classList.replace("entering", "current")));
  window.setTimeout(() => host.querySelectorAll(".swap-line.leaving").forEach((line) => line.remove()), 650);
}

function setMiniSub(text: string): void {
  const sub = element<HTMLElement>("mini-sub");
  if (sub.textContent === text) return;
  sub.classList.add("fading");
  window.setTimeout(() => {
    sub.textContent = text;
    sub.classList.remove("fading");
  }, 160);
}

function setMiniPhase(className: string): void {
  if (className === lastPhaseClass) return;
  const dot = element<HTMLElement>("mini-dot");
  dot.classList.replace(lastPhaseClass, className) || dot.classList.add(className);
  lastPhaseClass = className;
}

/** The window is resized to fit the player: the page tells the window how tall it needs to be. */
function desiredHeight(): number {
  const bar = 56;
  const more = element<HTMLElement>("mini-more-inner").scrollHeight;
  return miniOpen ? bar + more + 36 : 84;
}

async function syncHeight(): Promise<void> {
  try {
    await invoke("set_mini_height", { height: desiredHeight() });
  } catch {
    // Resizing is cosmetic; a failure leaves the player at its current size.
  }
}

function openMini(): void {
  window.clearTimeout(collapseTimer);
  if (miniOpen) return;
  miniOpen = true;
  // Grow the window first (it is transparent), then let the card unfold with an eased animation.
  void syncHeight().then(() => element<HTMLElement>("mini").classList.add("open"));
}

function scheduleCollapse(): void {
  window.clearTimeout(collapseTimer);
  collapseTimer = window.setTimeout(() => {
    const confirmShown = !element<HTMLElement>("run-confirm").classList.contains("hidden");
    if (steerOpen || confirmShown || !miniOpen) return;
    miniOpen = false;
    element<HTMLElement>("mini").classList.remove("open");
    // Shrink the window after the card has folded away.
    window.setTimeout(() => {
      if (!miniOpen) void syncHeight();
    }, 520);
  }, 650);
}

function initMiniPlayer(): void {
  const view = element<HTMLElement>("run-view");
  view.addEventListener("mouseenter", openMini);
  view.addEventListener("mouseleave", scheduleCollapse);
  const input = element<HTMLInputElement>("run-steer-text");
  const send = element<HTMLButtonElement>("run-steer-send");
  input.addEventListener("input", () => (send.disabled = input.value.trim().length === 0));
  input.addEventListener("focus", () => void beginSteering());
  input.addEventListener("blur", () => void endSteering(false));
  element<HTMLFormElement>("run-steer-form").addEventListener("submit", (event) => {
    event.preventDefault();
    void sendSteer();
  });
  input.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      input.value = "";
      input.blur();
    }
  });
  // Keep the window height right when the content changes (approval card appears, text wraps).
  new ResizeObserver(() => miniOpen && void syncHeight()).observe(element<HTMLElement>("mini-more-inner"));
  setMicUi(localStorage.getItem("aura.mic") === "on");
}

/** Typing a correction pauses the run (so nothing happens mid-sentence) and lets the box take focus. */
async function beginSteering(): Promise<void> {
  if (steerOpen) return;
  steerOpen = true;
  try {
    await invoke("set_steer_box_open", { open: true });
  } catch (error) {
    log("error", toMessage(error));
  }
}

async function endSteering(sent: boolean): Promise<void> {
  if (!steerOpen) return;
  steerOpen = false;
  const input = element<HTMLInputElement>("run-steer-text");
  try {
    await invoke("set_steer_box_open", { open: false });
    // Leaving the box empty-handed resumes; a half-written message stays and the run stays paused.
    if (!sent && input.value.trim().length === 0) await invoke("resume_autonomous_task");
  } catch (error) {
    log("error", toMessage(error));
  }
  scheduleCollapse();
}

async function sendSteer(): Promise<void> {
  const input = element<HTMLInputElement>("run-steer-text");
  const text = input.value.trim();
  if (!text) return;
  input.value = "";
  element<HTMLButtonElement>("run-steer-send").disabled = true;
  try {
    await invoke("steer_autonomous_task", { text });
    log("info", `New instruction sent: ${text}`);
    setMiniText("Got it, adjusting");
  } catch (error) {
    log("error", toMessage(error));
  }
  steerOpen = true; // endSteering only acts when the box was open
  await endSteering(true);
  input.blur();
}

function setMicUi(on: boolean): void {
  const button = element<HTMLButtonElement>("run-mic");
  button.setAttribute("aria-pressed", String(on));
  button.classList.toggle("active", on);
  button.title = on ? "Voice input on (coming soon)" : "Voice input off (coming soon)";
}

/** Voice input is not wired yet; the switch keeps the setting for when it is. */
function toggleMic(): void {
  const on = element<HTMLButtonElement>("run-mic").getAttribute("aria-pressed") !== "true";
  localStorage.setItem("aura.mic", on ? "on" : "off");
  setMicUi(on);
}

async function keepOnTop(): Promise<void> {
  if (!document.body.classList.contains("running")) return;
  try {
    await invoke("keep_run_panel_on_top");
  } catch {
    // Best effort.
  }
}

async function pollRunControls(): Promise<void> {
  if (!document.body.classList.contains("running")) return;
  try {
    const status = await invoke<{ paused: boolean; pauseReason: string; openSteerBox: boolean }>("run_control_status");
    if (status.paused !== runPaused) {
      setPausedUi(status.paused);
      log("info", runPaused ? `Paused: ${status.pauseReason}` : "Resumed: the agent looks at the screen again.");
    }
    if (status.openSteerBox) {
      openMini();
      element<HTMLInputElement>("run-steer-text").focus();
    }
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
  if (status.protocol) {
    protocol.value = status.protocol;
    updateProtocolFields();
  }
  if (status.baseUrl) baseUrl.value = status.baseUrl;
  if (status.model) model.value = status.model;
  // The key itself never comes back to the page; the field only says whether one is saved.
  if (status.hasApiKey) apiKey.placeholder = "Key saved (encrypted). Leave empty to keep it, or type a new one";
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
