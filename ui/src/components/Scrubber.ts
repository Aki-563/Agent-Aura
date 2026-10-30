export interface StepThumbnail {
  stepIndex: number;
  thumbnailUrl: string;
  actionSummary: string;
}

export interface StepScrubberProps {
  steps: StepThumbnail[];
  onSaveCorrection: (stepIndex: number, forbiddenAction: string, correctionText: string) => void;
  onBack: () => void;
}

export class StepScrubber {
  private props: StepScrubberProps;
  private selectedStepIndex: number = 1;

  constructor(props: StepScrubberProps) {
    this.props = props;
    if (this.props.steps.length > 0) {
      this.selectedStepIndex = this.props.steps[0].stepIndex;
    }
  }

  public render(container: HTMLElement) {
    const scrubberEl = document.createElement("div");
    scrubberEl.className = "step-scrubber";
    scrubberEl.innerHTML = `
      <h3>🔍 PINPOINT THE MISTAKE</h3>
      <p style="font-size: 13px; color: var(--text-dim); margin-bottom: 12px;">
        Select the step where the agent went off-track:
      </p>

      <div class="carousel" id="thumbnail-carousel">
        ${this.props.steps
          .map(
            (s) => `
          <div class="step-card ${s.stepIndex === this.selectedStepIndex ? "selected" : ""}" data-step="${s.stepIndex}">
            <img src="${s.thumbnailUrl}" alt="Step ${s.stepIndex}" />
            <div class="card-info">
              <div class="card-title">STEP ${s.stepIndex}</div>
              <div>${s.actionSummary}</div>
            </div>
          </div>
        `
          )
          .join("")}
      </div>

      <div style="margin-top: 16px;">
        <label style="font-size: 13px; color: var(--accent-red); font-weight: 600;">
          ⚠️ What was the mistake / forbidden action at Step <span id="selected-step-label">${this.selectedStepIndex}</span>?
        </label>
        <input type="text" id="forbidden-action-input" class="input-field" placeholder="e.g. Clicked 'Print' menu" />

        <label style="font-size: 13px; color: var(--accent-green); font-weight: 600;">
          ✅ What should I do at this step next time?
        </label>
        <input type="text" id="correction-text-input" class="input-field" placeholder="e.g. Click 'Export' -> 'Create PDF' instead" />

        <div class="button-row">
          <button id="scrubber-back-btn" class="btn btn-secondary">Back</button>
          <button id="scrubber-save-btn" class="btn btn-mistake">💾 Save Correction to Memory</button>
        </div>
      </div>
    `;

    container.appendChild(scrubberEl);

    // Add click listeners to step cards in carousel
    const cards = scrubberEl.querySelectorAll(".step-card");
    const label = scrubberEl.querySelector("#selected-step-label") as HTMLElement;

    cards.forEach((card) => {
      card.addEventListener("click", () => {
        cards.forEach((c) => c.classList.remove("selected"));
        card.classList.add("selected");
        const stepNum = parseInt(card.getAttribute("data-step") || "1", 10);
        this.selectedStepIndex = stepNum;
        if (label) label.textContent = stepNum.toString();
      });
    });

    const backBtn = scrubberEl.querySelector("#scrubber-back-btn") as HTMLButtonElement;
    const saveBtn = scrubberEl.querySelector("#scrubber-save-btn") as HTMLButtonElement;
    const forbiddenInput = scrubberEl.querySelector("#forbidden-action-input") as HTMLInputElement;
    const correctionInput = scrubberEl.querySelector("#correction-text-input") as HTMLInputElement;

    backBtn.addEventListener("click", () => {
      this.props.onBack();
    });

    saveBtn.addEventListener("click", () => {
      const forbidden = forbiddenInput.value.trim();
      const correction = correctionInput.value.trim();
      if (forbidden && correction) {
        this.props.onSaveCorrection(this.selectedStepIndex, forbidden, correction);
      }
    });
  }
}
