"""
Agent Aura - Phase 4 Unit Verification Test Suite
"""
import sys
import os
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "..")))

import io
from PIL import Image
from engine.batcher import (
    ActionBatch,
    ActionItem,
    parse_and_validate_action_batch,
    denormalize_fullscreen_coordinates,
    denormalize_crop_coordinates,
    DestructiveActionGate,
    HumilityGate,
)
from engine.prompt_builder import build_system_prompt, prepare_visual_payload
from engine.vlm_client import VlmClient

def test_action_batch_validation():
    valid_json = """
    {
      "step_id": 1,
      "thought": "Focus the Chrome address bar and navigate to GitHub.",
      "actions": [
        {"type": "hotkey", "keys": ["ctrl", "l"]},
        {"type": "type_text", "text": "https://github.com", "press_enter": true}
      ]
    }
    """
    batch = parse_and_validate_action_batch(valid_json)
    assert batch.step_id == 1
    assert len(batch.actions) == 2
    assert batch.actions[0].type == "hotkey"
    assert batch.actions[0].keys == ["ctrl", "l"]
    assert batch.actions[1].type == "type_text"
    assert batch.actions[1].text == "https://github.com"
    print("ActionBatch validation: PASSED")

def test_coordinate_denormalization():
    # Fullscreen test: 1280x720 canvas mapped to 3840x2160 (4K display)
    x_s, y_s = denormalize_fullscreen_coordinates(
        x_model=640,
        y_model=360,
        w_canvas=1280,
        h_canvas=720,
        w_physical=3840,
        h_physical=2160,
    )
    assert x_s == 1920
    assert y_s == 1080

    # Differential crop test: window at (100, 200) with size 800x600, canvas 800x600
    x_c, y_c = denormalize_crop_coordinates(
        x_model=400,
        y_model=300,
        w_canvas=800,
        h_canvas=600,
        l_win=100,
        t_win=200,
        w_win=800,
        h_win=600,
    )
    assert x_c == 500
    assert y_c == 500
    print("Coordinate denormalization: PASSED")

def test_destructive_gate():
    safe_batch = ActionBatch(
        step_id=1,
        thought="Open calculator app",
        actions=[ActionItem(type="hotkey", keys=["win", "r"])],
    )
    is_dest, _ = DestructiveActionGate.inspect(safe_batch)
    assert not is_dest

    danger_batch = ActionBatch(
        step_id=2,
        thought="Permanently delete all temporary files",
        actions=[ActionItem(type="key_press", key="enter")],
    )
    is_dest, reason = DestructiveActionGate.inspect(danger_batch)
    assert is_dest
    assert "permanently delete" in reason.lower()
    print("DestructiveActionGate: PASSED")

def test_humility_gate():
    gate = HumilityGate(failure_threshold=2)
    # Attempt 1: failure -> should not trigger yet
    triggered, _ = gate.record_attempt("btn_export", success=False)
    assert not triggered

    # Attempt 2: failure -> threshold reached, triggers prompt!
    triggered, prompt = gate.record_attempt("btn_export", success=False)
    assert triggered
    assert "trouble locating" in prompt
    assert "click it once" in prompt

    # Reset on success
    triggered, _ = gate.record_attempt("btn_export", success=True)
    assert not triggered
    print("HumilityGate: PASSED")

def test_prompt_builder():
    shortcuts = [{"app_process": "chrome.exe", "semantic_intent": "Focus URL bar", "hotkey_combo": "Ctrl+L"}]
    guardrails = [{"forbidden_action": "Click Print", "correction_text": "Click Export to PDF"}]

    prompt = build_system_prompt(shortcuts, guardrails, active_process="chrome.exe")
    assert "Focus URL bar" in prompt
    assert "Ctrl+L" in prompt
    assert "Click Print" in prompt
    assert "Click Export to PDF" in prompt
    assert "chrome.exe" in prompt
    print("PromptBuilder: PASSED")

def test_visual_payload_downscaling():
    # Generate 1920x1080 dummy image
    raw_rgba = b"\x80\x40\x20\xFF" * (1920 * 1080)
    webp_bytes, canvas_w, canvas_h = prepare_visual_payload(
        raw_rgba,
        1920,
        1080,
        max_long_edge=1280,
        webp_quality=85,
    )

    assert len(webp_bytes) > 0
    assert canvas_w == 1280
    assert canvas_h == 720
    # Verify valid WebP magic header: starts with RIFF ... WEBP
    assert webp_bytes[:4] == b"RIFF"
    assert webp_bytes[8:12] == b"WEBP"
    print(f"Visual payload downscaling: PASSED (size={len(webp_bytes)} bytes)")

def test_vlm_client_init():
    gemini_client = VlmClient(provider="gemini", model_name="gemini-2.0-flash")
    assert gemini_client.provider == "gemini"
    assert gemini_client.model_name == "gemini-2.0-flash"

    claude_client = VlmClient(provider="claude", model_name="claude-3-7-sonnet-20250219")
    assert claude_client.provider in ("claude", "anthropic")
    print("VlmClient initialization: PASSED")

if __name__ == "__main__":
    test_action_batch_validation()
    test_coordinate_denormalization()
    test_destructive_gate()
    test_humility_gate()
    test_prompt_builder()
    test_visual_payload_downscaling()
    test_vlm_client_init()
    print("\nALL PHASE 4 PYTHON TESTS PASSED SUCCESSFULLY!")
