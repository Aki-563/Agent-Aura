export interface CautionGateProps {
  actionDescription: string;
  onConfirm: () => void;
  onCancel: () => void;
}

export class CautionGate {
  private props: CautionGateProps;

  constructor(props: CautionGateProps) {
    this.props = props;
  }

  public render(container: HTMLElement) {
    const el = document.createElement("div");
    el.className = "modal-backdrop caution-gate";
    el.innerHTML = `
      <div class="glass-card">
        <h3>⚠️ Human Confirmation Required</h3>
        <p>About to execute: <strong>${this.props.actionDescription}</strong>. Allow this action?</p>
        <div class="button-row">
          <button id="caution-confirm" class="btn btn-warning">Yes, Proceed</button>
          <button id="caution-cancel" class="btn btn-secondary">No, Cancel</button>
        </div>
      </div>
    `;
    container.appendChild(el);
  }
}
