"""
Agent Aura - Supervisor-Worker IPC Bridge (Phase 7 implementation)
Implements typed bidirectional JSON-lines protocol between the native Rust supervisor
and the Python cognitive brain, as specified in docs/CORE.md Section 6.
"""
import sys
import json
import threading
from typing import Dict, Any, Callable, Optional

class IpcBridge:
    """Bi-directional typed IPC bridge over stdin / stdout JSON-lines."""

    def __init__(self, message_handler: Optional[Callable[[Dict[str, Any]], None]] = None):
        self.message_handler = message_handler
        self._running = False
        self._lock = threading.Lock()

    def send_message(self, msg: Dict[str, Any]):
        """Encodes message as single-line JSON and writes to stdout."""
        with self._lock:
            line = json.dumps(msg) + "\n"
            sys.stdout.write(line)
            sys.stdout.flush()

    # --- Engine -> Rust Outbound Commands ---

    def execute_batch(self, actions: list, step_id: int, thought: str):
        """Dispatches an action batch to the native Rust actuator."""
        self.send_message({
            "type": "EXECUTE_BATCH",
            "step_id": step_id,
            "thought": thought,
            "actions": actions,
        })

    def set_hud_state(self, state: str):
        """Updates the Direct2D screen-edge neon glow overlay state."""
        self.send_message({
            "type": "SET_HUD_STATE",
            "state": state, # 'thinking', 'acting', 'settling', 'emergency', 'inactive'
        })

    def request_radar_scan(self, anchor_id: str, roi: Optional[dict] = None):
        """Requests a 2ms local NCC template scan from the Rust supervisor."""
        self.send_message({
            "type": "REQUEST_RADAR_SCAN",
            "anchor_id": anchor_id,
            "roi": roi,
        })

    # --- Rust -> Engine Inbound Listener ---

    def handle_line(self, line: str):
        """Parses and dispatches a single inbound JSON line."""
        line = line.strip()
        if not line:
            return
        try:
            data = json.loads(line)
            if self.message_handler:
                self.message_handler(data)
        except Exception as e:
            self.send_message({"type": "ERROR", "error": f"Invalid JSON message: {e}"})

    def listen_loop(self):
        """Blocking listener loop reading lines from stdin."""
        self._running = True
        for line in sys.stdin:
            if not self._running:
                break
            self.handle_line(line)

    def stop(self):
        self._running = False
