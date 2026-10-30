"""
Agent Aura - Action Batcher & Schema Validator (Phase 4 implementation)
Enforces strongly typed JSON action chains without arbitrary code execution,
and handles Destructive Action Confirmation and Humility Anti-Loop Gates.
"""
import re
import json
from typing import List, Dict, Any, Optional, Tuple
from pydantic import BaseModel, Field, field_validator

ALLOWED_ACTION_TYPES = {
    "click",
    "double_click",
    "drag_and_drop",
    "type_text",
    "hotkey",
    "key_press",
    "scroll",
    "wait_settle",
    "done",
    "fail",
}

DESTRUCTIVE_KEYWORDS = [
    "permanently delete",
    "delete file",
    "delete folder",
    "empty recycle bin",
    "drop table",
    "drop database",
    "format disk",
    "format drive",
    "wipe data",
    "rm -rf",
    "rmdir /s",
    "erase disk",
]

class ActionItem(BaseModel):
    type: str
    x: Optional[int] = None
    y: Optional[int] = None
    button: Optional[str] = "left"
    clicks: Optional[int] = 1
    hold_keys: Optional[List[str]] = Field(default_factory=list)
    from_x: Optional[int] = None
    from_y: Optional[int] = None
    to_x: Optional[int] = None
    to_y: Optional[int] = None
    duration_ms: Optional[int] = None
    text: Optional[str] = None
    press_enter: Optional[bool] = False
    overwrite: Optional[bool] = False
    keys: Optional[List[str]] = None
    key: Optional[str] = None
    horizontal: Optional[bool] = False
    max_timeout_ms: Optional[int] = None
    expected_cursor: Optional[str] = None
    summary: Optional[str] = None
    reason: Optional[str] = None

    @field_validator("type")
    @classmethod
    def validate_action_type(cls, v: str) -> str:
        clean = v.lower().strip()
        if clean not in ALLOWED_ACTION_TYPES:
            raise ValueError(f"Action type '{v}' is not allowed. Must be one of {sorted(ALLOWED_ACTION_TYPES)}")
        return clean

class ActionBatch(BaseModel):
    step_id: int = 1
    thought: str
    actions: List[ActionItem]

def parse_and_validate_action_batch(raw_text: str) -> ActionBatch:
    """Parses raw LLM string output into a validated ActionBatch, handling markdown blocks."""
    cleaned = raw_text.strip()

    # Strip markdown codeblocks e.g. ```json ... ```
    if "```" in cleaned:
        match = re.search(r"```(?:json)?\s*([\s\S]*?)\s*```", cleaned)
        if match:
            cleaned = match.group(1).strip()

    data = json.loads(cleaned)
    return ActionBatch.model_validate(data)

def denormalize_fullscreen_coordinates(
    x_model: int,
    y_model: int,
    w_canvas: int,
    h_canvas: int,
    w_physical: int,
    h_physical: int,
) -> Tuple[int, int]:
    """
    Transforms coordinates from aspect-preserved VLM canvas to physical screen coordinates.
    Formula from docs/CORE.md Section 3.2:
    X_screen = round(X_model * W_physical / W_canvas)
    Y_screen = round(Y_model * H_physical / H_canvas)
    """
    x_screen = int(round(x_model * (w_physical / max(1, w_canvas))))
    y_screen = int(round(y_model * (h_physical / max(1, h_canvas))))
    return (max(0, min(x_screen, w_physical - 1)), max(0, min(y_screen, h_physical - 1)))

def denormalize_crop_coordinates(
    x_model: int,
    y_model: int,
    w_canvas: int,
    h_canvas: int,
    l_win: int,
    t_win: int,
    w_win: int,
    h_win: int,
) -> Tuple[int, int]:
    """
    Transforms coordinates from window-relative cropped visual patch to physical screen coordinates.
    Formula from docs/CORE.md Section 3.2:
    X_screen = L_win + round(X_model * W_win / W_canvas)
    Y_screen = T_win + round(Y_model * H_win / H_canvas)
    """
    x_screen = l_win + int(round(x_model * (w_win / max(1, w_canvas))))
    y_screen = t_win + int(round(y_model * (h_win / max(1, h_canvas))))
    return (x_screen, y_screen)

class DestructiveActionGate:
    """Detects whether an action batch requires explicit human caution confirmation."""

    @staticmethod
    def inspect(batch: ActionBatch) -> Tuple[bool, Optional[str]]:
        thought_lower = batch.thought.lower()
        for kw in DESTRUCTIVE_KEYWORDS:
            if kw in thought_lower:
                return True, f"Action appears destructive: '{kw}' detected in thought: \"{batch.thought}\""

        for act in batch.actions:
            if act.type == "type_text" and act.text:
                text_lower = act.text.lower()
                for kw in DESTRUCTIVE_KEYWORDS:
                    if kw in text_lower:
                        return True, f"Action typing destructive command: '{act.text}'"

        return False, None

class HumilityGate:
    """Anti-loop gate: if agent fails 2 consecutive retries, asks user for 1 click direction."""

    def __init__(self, failure_threshold: int = 2):
        self.failure_threshold = failure_threshold
        self.failure_counts: Dict[str, int] = {}

    def record_attempt(self, step_key: str, success: bool) -> Tuple[bool, Optional[str]]:
        if success:
            self.failure_counts[step_key] = 0
            return False, None

        current = self.failure_counts.get(step_key, 0) + 1
        self.failure_counts[step_key] = current

        if current >= self.failure_threshold:
            prompt = (
                f"I'm having trouble locating the target element for '{step_key}' after {current} retries. "
                "Could you please click it once on the screen? I will learn the visual anchor and resume from there."
            )
            return True, prompt

        return False, None

    def reset(self, step_key: Optional[str] = None):
        if step_key:
            self.failure_counts.pop(step_key, None)
        else:
            self.failure_counts.clear()
