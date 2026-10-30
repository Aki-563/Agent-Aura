import { StepScrubber, type StepThumbnail } from "./Scrubber";

export interface ReviewModalProps {
  taskId: string;
  taskPrompt: string;
  steps: StepThumbnail[];
  onPerfect: () => Promise<void>;
  onOptimize: (instructions: string) => Promise<void>;
  onMistake: (stepIndex: number, forbiddenAction: string, correction: string) => Promise<void>;
  onClose?: () => void;
}

export class ReviewModal {
  private props: ReviewModalProps;
  private container: HTMLElement | null = null;
  private scrubber: StepScrubber | null = null;

  constructor(props: ReviewModalProps) {
    this.props = props;
  }

  public render(parentElement: HTMLElement) {
    this.container = document.createElement("div");
    this.container.className = "modal-backdrop";
    this.container.innerHTML = `
      <div class="glass-card review-modal" id="modal-card">
        <h2>⚡ AGENT AURA TASK REVIEW</h2>
        <p class="task-prompt">${this.props.taskPrompt}</p>

        <div id="review-content">
          <p style="font-size: 14px; margin-bottom: 20px;">Did I complete this task correctly?</p>
          <div class="button-row">
            <button id="btn-perfect" class="btn btn-perfect">✅ Perfect</button>
            <button id="btn-optimize" class="btn btn-optimize">⚡ Yes, but optimize it</button>
            <button id="btn-mistake" class="btn btn-mistake">❌ Mistake made</button>
          </div>
        </div>

        <div id="sub-view-container" style="display: none;"></div>
      </div>
    `;

    parentElement.appendChild(this.container);

    const btnPerfect = this.container.querySelector("#btn-perfect") as HTMLButtonElement;
    const btnOptimize = this.container.querySelector("#btn-optimize") as HTMLButtonElement;
    const btnMistake = this.container.querySelector("#btn-mistake") as HTMLButtonElement;

    btnPerfect.addEventListener("click", async () => {
      try {
        await this.props.onPerfect();
        this.showSuccess("Trajectory promoted to Golden Path skill! 🌟");
        setTimeout(() => this.destroy(), 1200);
      } catch (error) {
        this.showFailure(error);
      }
    });

    btnOptimize.addEventListener("click", () => {
      this.renderOptimizeView();
    });

    btnMistake.addEventListener("click", () => {
      this.renderMistakeView();
    });
  }

  private renderOptimizeView() {
    const subContainer = this.container?.querySelector("#sub-view-container") as HTMLElement;
    const reviewContent = this.container?.querySelector("#review-content") as HTMLElement;
    if (!subContainer || !reviewContent) return;

    reviewContent.style.display = "none";
    subContainer.style.display = "block";
    subContainer.innerHTML = `
      <h3 style="color: var(--accent-purple); margin: 0 0 10px 0;">⚡ Suggest a Shortcut Optimization</h3>
      <p style="font-size: 13px; color: var(--text-dim); margin-bottom: 8px;">
        e.g. "Don't click 3 menus to save; just press Ctrl+S next time."
      </p>
      <input type="text" id="optimize-input" class="input-field" placeholder="Enter shortcut or hotkey instruction..." />
      <div class="button-row">
        <button id="btn-optimize-back" class="btn btn-secondary">Back</button>
        <button id="btn-optimize-submit" class="btn btn-primary">Save Optimization</button>
      </div>
    `;

    const backBtn = subContainer.querySelector("#btn-optimize-back") as HTMLButtonElement;
    const submitBtn = subContainer.querySelector("#btn-optimize-submit") as HTMLButtonElement;
    const input = subContainer.querySelector("#optimize-input") as HTMLInputElement;

    backBtn.addEventListener("click", () => {
      subContainer.style.display = "none";
      reviewContent.style.display = "block";
    });

    submitBtn.addEventListener("click", async () => {
      const text = input.value.trim();
      if (text) {
        try {
          await this.props.onOptimize(text);
          this.showSuccess("Optimization saved to Apprenticeship Engine! ⚡");
          setTimeout(() => this.destroy(), 1200);
        } catch (error) {
          this.showFailure(error);
        }
      }
    });
  }

  private renderMistakeView() {
    const subContainer = this.container?.querySelector("#sub-view-container") as HTMLElement;
    const reviewContent = this.container?.querySelector("#review-content") as HTMLElement;
    if (!subContainer || !reviewContent) return;

    reviewContent.style.display = "none";
    subContainer.style.display = "block";
    subContainer.innerHTML = "";

    this.scrubber = new StepScrubber({
      steps: this.props.steps,
      onSaveCorrection: (stepIndex, forbidden, correction) => {
        void this.props.onMistake(stepIndex, forbidden, correction).then(() => {
          this.showSuccess("Correction saved into Guardrails table! 🛡️");
          setTimeout(() => this.destroy(), 1200);
        }).catch((error) => this.showFailure(error));
      },
      onBack: () => {
        subContainer.style.display = "none";
        reviewContent.style.display = "block";
      },
    });

    this.scrubber.render(subContainer);
  }

  private showSuccess(message: string) {
    const card = this.container?.querySelector("#modal-card") as HTMLElement;
    if (card) {
      card.innerHTML = `
        <div style="text-align: center; padding: 20px;">
          <h2 style="color: var(--accent-green); margin-bottom: 8px;">✅ CONFIRMED</h2>
          <p style="color: var(--text-main); font-size: 15px;">${message}</p>
        </div>
      `;
    }
  }

  private showFailure(error: unknown) {
    const card = this.container?.querySelector("#modal-card") as HTMLElement;
    const message = error instanceof Error ? error.message : String(error);
    if (card) {
      card.replaceChildren();
      const wrapper = document.createElement("div");
      wrapper.style.cssText = "text-align:center;padding:20px";
      const heading = document.createElement("h2");
      heading.style.color = "var(--accent-red)";
      heading.textContent = "SAVE FAILED";
      const detail = document.createElement("p");
      detail.textContent = message;
      wrapper.append(heading, detail);
      card.append(wrapper);
    }
  }

  public destroy() {
    this.container?.remove();
    this.container = null;
    this.props.onClose?.();
  }
}
