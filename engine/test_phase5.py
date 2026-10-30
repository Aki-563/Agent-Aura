"""
Agent Aura - Phase 5 Unit Verification Test Suite
Tests 3-Tier Semantic Cache, Triple-Lock Snapping, and Trajectory Distillation.
"""
import sys
import os
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "..")))

from engine.cache_manager import (
    SemanticCacheManager,
    LevenshteinUtils,
    TripleLockStateSnapper,
    cosine_similarity,
)
from engine.skill_compiler import SkillCompiler

def test_tier1_exact_hash_cache():
    cache = SemanticCacheManager()
    cache.add_entry("clean desktop and organize icons", "skill_clean_desktop_01")

    skill_id, tier = cache.lookup("clean desktop and organize icons")
    assert skill_id == "skill_clean_desktop_01"
    assert tier == "tier1_exact_hash"
    print("Tier 1 Exact Hash: PASSED")

def test_tier2_fuzzy_levenshtein_cache():
    cache = SemanticCacheManager()
    cache.add_entry("export monthly sales report to pdf", "skill_export_sales_pdf")

    # Reordered words and slight difference
    query = "export sales monthly report to pdf"
    ratio = LevenshteinUtils.token_sort_ratio(query, "export monthly sales report to pdf")
    assert ratio >= 0.90

    skill_id, tier = cache.lookup(query, fuzzy_threshold=0.90)
    assert skill_id == "skill_export_sales_pdf"
    assert "tier2_fuzzy_levenshtein" in tier
    print("Tier 2 Fuzzy Levenshtein: PASSED")

def test_tier3_vector_embedding_cache():
    cache = SemanticCacheManager()
    cache.add_entry("organize desktop files into folders", "skill_tidy_desktop")

    # Semantic variant
    query = "organize my desktop files into folders"
    skill_id, tier = cache.lookup(query, fuzzy_threshold=0.98, vector_threshold=0.85)
    assert skill_id == "skill_tidy_desktop"
    print(f"Tier 3 Vector Search ({tier}): PASSED")

def test_cache_miss():
    cache = SemanticCacheManager()
    cache.add_entry("open chrome and browse to news", "skill_browse_news")

    skill_id, tier = cache.lookup("format drive c and delete partitions")
    assert skill_id is None
    assert tier == "miss"
    print("Cache Miss handling: PASSED")

def test_triple_lock_snapping():
    # Test 1: All locks pass
    snapped, msg = TripleLockStateSnapper.verify_and_snap(
        current_process="excel.exe",
        current_window_title="Q3_Sales.xlsx - Excel",
        expected_process="excel.exe",
        expected_title_contains="Excel",
        anchor_matched=True,
        anchor_confidence=0.94,
        goal_intent_matched=True,
    )
    assert snapped
    assert "Triple-Lock Passed" in msg

    # Test 2: Anchor missing (Lock 2 fails)
    snapped, msg = TripleLockStateSnapper.verify_and_snap(
        current_process="excel.exe",
        current_window_title="Q3_Sales.xlsx - Excel",
        expected_process="excel.exe",
        expected_title_contains="Excel",
        anchor_matched=False,
        anchor_confidence=0.45,
        goal_intent_matched=True,
    )
    assert not snapped
    assert "Lock 2 Failed" in msg
    print("TripleLockStateSnapper: PASSED")

def test_trajectory_distillation():
    compiler = SkillCompiler()
    raw_steps = [
        {"thought": "Click search bar", "actions": [{"type": "click", "x": 100, "y": 200}], "success": True},
        # Dead-end exploration: clicked wrong menu then hit Escape to cancel
        {"thought": "Click View menu", "actions": [{"type": "click", "x": 400, "y": 50}], "success": True},
        {"thought": "Cancel View menu", "actions": [{"type": "key_press", "key": "esc"}], "success": True},
        # Failed step
        {"thought": "Click nonexistent button", "actions": [{"type": "click", "x": 999, "y": 999}], "success": False},
        # Legitimate step with shortcut optimization potential
        {"thought": "Save file to disk", "actions": [{"type": "click", "x": 50, "y": 50}], "success": True},
    ]

    shortcuts = {"save file": "Ctrl+S"}
    distilled = compiler.distill_trajectory(raw_steps, shortcut_optimizations=shortcuts)

    # Dead-end (View + Esc) stripped, failed step stripped, Save optimized to hotkey!
    assert len(distilled) == 2
    assert distilled[0]["actions"][0]["type"] == "click"
    assert distilled[1]["actions"][0]["type"] == "hotkey"
    assert distilled[1]["actions"][0]["keys"] == ["ctrl", "s"]
    print("SkillCompiler trajectory distillation: PASSED")

def test_visual_anchor_extraction():
    compiler = SkillCompiler()
    frame_w = 1920
    frame_h = 1080
    raw_frame = b"\x10\x20\x30\xFF" * (frame_w * frame_h)

    crop_bytes, crop_w, crop_h = compiler.extract_visual_anchor(
        raw_frame,
        frame_w,
        frame_h,
        target_x=500,
        target_y=400,
        anchor_size=64,
    )

    assert crop_w == 64
    assert crop_h == 64
    assert len(crop_bytes) == 64 * 64 * 4
    print("Visual anchor 64x64 extraction: PASSED")

if __name__ == "__main__":
    test_tier1_exact_hash_cache()
    test_tier2_fuzzy_levenshtein_cache()
    test_tier3_vector_embedding_cache()
    test_cache_miss()
    test_triple_lock_snapping()
    test_trajectory_distillation()
    test_visual_anchor_extraction()
    print("\nALL PHASE 5 TESTS PASSED SUCCESSFULLY!")
