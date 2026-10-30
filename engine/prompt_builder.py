"""
Agent Aura - System Prompt Builder & Visual Downscaler (Phase 4 implementation)
Prepares token-optimized WebP visual frames (clamped to 1280px) and injects
learned shortcuts and anti-pattern guardrails into the cognitive prompt.
"""
import io
from typing import List, Dict, Any, Optional, Tuple
from PIL import Image

SYSTEM_PROMPT_TEMPLATE = """You are Agent Aura, an ultra-fast, human-like desktop computer-use intelligence.
You interact directly with Windows 10/11 through strongly typed multi-step JSON action batches.

### RULES & CONSTRAINTS:
1. Output MUST be valid, parseable JSON matching the ActionBatch schema. No markdown wrapping, no explanatory text.
2. Ground pixel coordinates accurately on the provided screen image.
3. Use ACTION BATCHING: Combine deterministic multi-step chains into a single step (e.g. click address bar -> type URL -> enter).
4. Prefer pre-loaded keyboard shortcuts over slow multi-click visual menus whenever available.
5. If the requested task is impossible or blocked by permissions, emit a `fail` action with a clear reason.
6. When the user's task is fully accomplished, emit a `done` action with a concise summary.

### JSON SCHEMA:
{
  "step_id": 1,
  "thought": "Focus search bar and query for project roadmap.",
  "actions": [
    {"type": "hotkey", "keys": ["ctrl", "f"]},
    {"type": "type_text", "text": "roadmap", "press_enter": true}
  ]
}

### ALLOWED ACTIONS:
- click: {"type": "click", "x": int, "y": int, "button": "left"|"right"|"middle", "clicks": 1}
- double_click: {"type": "double_click", "x": int, "y": int, "button": "left"}
- drag_and_drop: {"type": "drag_and_drop", "from_x": int, "from_y": int, "to_x": int, "to_y": int, "duration_ms": 250}
- type_text: {"type": "type_text", "text": "...", "press_enter": bool, "overwrite": bool}
- hotkey: {"type": "hotkey", "keys": ["ctrl", "c"]}
- key_press: {"type": "key_press", "key": "enter"}
- scroll: {"type": "scroll", "clicks": int, "horizontal": bool}
- wait_settle: {"type": "wait_settle", "max_timeout_ms": 1000}
- done: {"type": "done", "summary": "..."}
- fail: {"type": "fail", "reason": "..."}
"""

def build_system_prompt(
    shortcuts: Optional[List[Dict[str, Any]]] = None,
    guardrails: Optional[List[Dict[str, Any]]] = None,
    active_process: Optional[str] = None,
) -> str:
    """Builds system prompt injected with context-relevant hotkeys and mistake guardrails."""
    prompt = SYSTEM_PROMPT_TEMPLATE

    if shortcuts:
        prompt += "\n\n### PRE-LOADED APPLICATION HOTKEYS (Use these to bypass visual menu clicks):\n"
        for s in shortcuts:
            app = s.get("app_process", "global")
            intent = s.get("semantic_intent", "")
            hotkey = s.get("hotkey_combo", "")
            prompt += f"- [{app}] {intent} -> `{hotkey}`\n"

    if guardrails:
        prompt += "\n\n### ⚠️ PAST USER CORRECTIONS & MISTAKES TO AVOID (CRITICAL GUARDRAILS):\n"
        for g in guardrails:
            forbidden = g.get("forbidden_action", "")
            correction = g.get("correction_text", "")
            prompt += f"- DO NOT: {forbidden} | INSTEAD: {correction}\n"

    if active_process:
        prompt += f"\n\nActive Process: `{active_process}`\n"

    return prompt

def prepare_visual_payload(
    raw_rgba_bytes: bytes,
    width: u32 if False else int,
    height: int,
    window_crop_rect: Optional[Tuple[int, int, int, int]] = None,
    max_long_edge: int = 1280,
    webp_quality: int = 85,
) -> Tuple[bytes, int, int]:
    """
    Downscales and compresses desktop frame into an aspect-preserved WebP payload.
    Supports Differential Visual Patching by cropping active window when provided.

    Returns:
        (compressed_webp_bytes, canvas_width, canvas_height)
    """
    # 1. Load image from raw RGBA buffer
    img = Image.frombytes("RGBA", (width, height), raw_rgba_bytes)

    # 2. Differential Visual Patching: crop active window if requested
    if window_crop_rect:
        cx, cy, cw, ch = window_crop_rect
        # Clamp crop box to image bounds
        left = max(0, min(cx, width - 1))
        top = max(0, min(cy, height - 1))
        right = max(left + 1, min(cx + cw, width))
        bottom = max(top + 1, min(cy + ch, height))
        img = img.crop((left, top, right, bottom))

    curr_w, curr_h = img.size

    # 3. Lanczos downscaling clamping long edge to max_long_edge (default 1280px)
    long_edge = max(curr_w, curr_h)
    if long_edge > max_long_edge:
        scale = max_long_edge / float(long_edge)
        new_w = int(round(curr_w * scale))
        new_h = int(round(curr_h * scale))
        img = img.resize((new_w, new_h), Image.Resampling.LANCZOS)
    else:
        new_w, new_h = curr_w, curr_h

    # Convert RGBA to RGB for WebP compression efficiency
    rgb_img = img.convert("RGB")

    # 4. WebP encoding (Quality 85)
    buf = io.BytesIO()
    rgb_img.save(buf, format="WEBP", quality=webp_quality, method=4)
    compressed_bytes = buf.getvalue()

    return compressed_bytes, new_w, new_h
