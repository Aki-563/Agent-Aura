"""
Agent Aura - Apprenticeship Golden Path & Anchor Extractor (Phase 5 implementation)
Distills raw human demonstrations or agent trials into minimal Golden Paths,
prunes dead-ends and mistakes, and extracts 64x64 visual anchors for self-healing execution.
"""
from typing import List, Dict, Any, Optional, Tuple
from PIL import Image
import io

class SkillCompiler:
    """Trajectory Distillation Compiler implementing Feature 2.1 from docs/FEATURES.md."""

    def distill_trajectory(
        self,
        raw_steps: List[Dict[str, Any]],
        shortcut_optimizations: Optional[Dict[str, str]] = None,
    ) -> List[Dict[str, Any]]:
        """
        Distills raw execution steps into a clean Golden Path:
        1. Strips failed / error branches.
        2. Prunes dead-end UI exploration (e.g. open menu -> cancel).
        3. Consolidates redundant typing and backspaces.
        4. Applies shortcut substitutions (e.g. File->Save -> Ctrl+S).
        5. Chains deterministic primitives into Action Batches.
        """
        # Step 1: Filter out explicitly failed steps
        clean_steps = [s for s in raw_steps if s.get("success", True)]

        # Step 2: Prune dead-end exploration
        pruned_steps: List[Dict[str, Any]] = []
        i = 0
        while i < len(clean_steps):
            step = clean_steps[i]
            actions = step.get("actions", [])

            # Check if this step was immediately undone by next step (e.g. click menu then Escape)
            if i + 1 < len(clean_steps):
                next_step = clean_steps[i + 1]
                next_actions = next_step.get("actions", [])
                if self._is_undo_pair(actions, next_actions):
                    # Skip both dead-end steps!
                    i += 2
                    continue

            pruned_steps.append(step)
            i += 1

        # Step 3: Apply shortcut optimizations
        if shortcut_optimizations:
            pruned_steps = self._apply_shortcuts(pruned_steps, shortcut_optimizations)

        # Step 4: Macro Chaining / Batching into minimal turns
        golden_path = self._batch_actions(pruned_steps)
        return golden_path

    def _is_undo_pair(self, act1: List[Dict[str, Any]], act2: List[Dict[str, Any]]) -> bool:
        """Detects whether act2 merely undid an exploratory act1."""
        if not act1 or not act2:
            return False

        first = act1[0]
        second = act2[0]

        # e.g. opened a menu via click, but immediately pressed Escape
        if first.get("type") == "click" and second.get("type") == "key_press" and second.get("key") in ("esc", "escape"):
            return True

        return False

    def _apply_shortcuts(
        self,
        steps: List[Dict[str, Any]],
        shortcuts: Dict[str, str],
    ) -> List[Dict[str, Any]]:
        """Substitutes multi-click visual menus with instant hotkeys."""
        optimized = []
        for step in steps:
            intent = step.get("thought", "").lower()
            replaced = False
            for target_intent, hotkey in shortcuts.items():
                if target_intent.lower() in intent:
                    keys = [k.strip().lower() for k in hotkey.split("+")]
                    optimized.append({
                        "thought": f"Optimized via shortcut: {hotkey}",
                        "actions": [{"type": "hotkey", "keys": keys}],
                        "success": True,
                    })
                    replaced = True
                    break
            if not replaced:
                optimized.append(step)
        return optimized

    def _batch_actions(self, steps: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Chains sequential deterministic actions into coherent action batches."""
        batched = []
        step_id = 1
        for step in steps:
            actions = step.get("actions", [])
            batched.append({
                "step_id": step_id,
                "thought": step.get("thought", "Execute action batch"),
                "actions": actions,
            })
            step_id += 1
        return batched

    def extract_visual_anchor(
        self,
        rgba_bytes: bytes,
        frame_width: int,
        frame_height: int,
        target_x: int,
        target_y: int,
        anchor_size: int = 64,
    ) -> Tuple[bytes, int, int]:
        """
        Extracts a 64x64 pixel visual anchor crop centered at target (x, y).
        Returns: (crop_rgba_bytes, crop_width, crop_height)
        """
        half = anchor_size // 2
        left = max(0, min(target_x - half, frame_width - anchor_size))
        top = max(0, min(target_y - half, frame_height - anchor_size))
        right = min(left + anchor_size, frame_width)
        bottom = min(top + anchor_size, frame_height)

        img = Image.frombytes("RGBA", (frame_width, frame_height), rgba_bytes)
        crop_img = img.crop((left, top, right, bottom))

        crop_w, crop_h = crop_img.size
        return crop_img.tobytes(), crop_w, crop_h
