"""
Agent Aura - 3-Tier Semantic Cache Manager (Phase 5 implementation)
Tier 1: Exact SHA-256 hash (<0.01ms)
Tier 2: Levenshtein token-sort similarity ratio (>0.90, <0.5ms)
Tier 3: Local vector embedding cosine similarity (>0.88, <4ms)
Mid-Task State Snapping: Triple-Lock Verification
"""
import math
import struct
import hashlib
from typing import Optional, Tuple, List, Dict, Any

class LevenshteinUtils:
    @staticmethod
    def token_sort_normalize(text: str) -> str:
        tokens = text.lower().strip().split()
        tokens.sort()
        return " ".join(tokens)

    @staticmethod
    def distance(s1: str, s2: str) -> int:
        """Computes Levenshtein edit distance with O(min(N, M)) space."""
        if len(s1) < len(s2):
            return LevenshteinUtils.distance(s2, s1)
        if len(s2) == 0:
            return len(s1)

        previous_row = list(range(len(s2) + 1))
        for i, c1 in enumerate(s1):
            current_row = [i + 1]
            for j, c2 in enumerate(s2):
                insertions = previous_row[j + 1] + 1
                deletions = current_row[j] + 1
                substitutions = previous_row[j] + (c1 != c2)
                current_row.append(min(insertions, deletions, substitutions))
            previous_row = current_row

        return previous_row[-1]

    @staticmethod
    def token_sort_ratio(s1: str, s2: str) -> float:
        """Computes token-sort similarity ratio in range [0.0, 1.0]."""
        norm1 = LevenshteinUtils.token_sort_normalize(s1)
        norm2 = LevenshteinUtils.token_sort_normalize(s2)

        if norm1 == norm2:
            return 1.0

        total_len = len(norm1) + len(norm2)
        if total_len == 0:
            return 1.0

        dist = LevenshteinUtils.distance(norm1, norm2)
        return max(0.0, (total_len - dist) / float(total_len))

def cosine_similarity(vec1: List[float], vec2: List[float]) -> float:
    if len(vec1) != len(vec2) or not vec1:
        return 0.0

    dot = 0.0
    norm1 = 0.0
    norm2 = 0.0
    for a, b in zip(vec1, vec2):
        dot += a * b
        norm1 += a * a
        norm2 += b * b

    denom = math.sqrt(norm1) * math.sqrt(norm2)
    if denom < 1e-6:
        return 0.0
    return dot / denom

def pack_embedding(vec: List[float]) -> bytes:
    """Serializes float vector to little-endian IEEE-754 bytes."""
    return struct.pack(f"<{len(vec)}f", *vec)

def unpack_embedding(blob: bytes) -> List[float]:
    """Deserializes float vector from binary blob."""
    count = len(blob) // 4
    return list(struct.unpack(f"<{count}f", blob))

class FastEmbedder:
    """Embedder using fastembed if available, otherwise deterministic semantic projection."""

    def __init__(self):
        self._model = None
        try:
            from fastembed import TextEmbedding
            self._model = TextEmbedding(model_name="BAAI/bge-small-en-v1.5")
        except Exception:
            self._model = None

    def embed(self, text: str) -> List[float]:
        if self._model:
            embeddings = list(self._model.embed([text]))
            return embeddings[0].tolist()

        # Deterministic 128-dimensional character n-gram projection
        dim = 128
        vec = [0.0] * dim
        clean = text.lower().strip()
        for i in range(len(clean) - 2):
            trigram = clean[i:i+3]
            h = int(hashlib.md5(trigram.encode("utf-8")).hexdigest(), 16)
            idx = h % dim
            sign = 1.0 if (h // dim) % 2 == 0 else -1.0
            vec[idx] += sign

        # L2 normalize
        norm = math.sqrt(sum(x * x for x in vec))
        if norm > 1e-6:
            vec = [x / norm for x in vec]
        return vec

class SemanticCacheManager:
    """3-Tier Semantic Cache Controller implementing Feature 1.8 from docs/FEATURES.md."""

    def __init__(self, db_conn=None):
        self.db = db_conn
        self.in_memory_hash_cache: Dict[str, str] = {} # sha256 -> skill_id
        self.cached_prompts: List[Dict[str, Any]] = [] # list of {prompt, skill_id, embedding}
        self.embedder = FastEmbedder()

    def compute_sha256(self, prompt: str) -> str:
        clean = prompt.strip().lower()
        return hashlib.sha256(clean.encode("utf-8")).hexdigest()

    def add_entry(self, prompt: str, skill_id: str, embedding: Optional[List[float]] = None):
        h = self.compute_sha256(prompt)
        self.in_memory_hash_cache[h] = skill_id

        if embedding is None:
            embedding = self.embedder.embed(prompt)

        self.cached_prompts.append({
            "sha256": h,
            "prompt": prompt,
            "skill_id": skill_id,
            "embedding": embedding,
        })

    def lookup(
        self,
        prompt: str,
        fuzzy_threshold: float = 0.90,
        vector_threshold: float = 0.88,
    ) -> Tuple[Optional[str], str]:
        """
        Executes hierarchical 3-tier lookup:
        - Tier 1: Exact Hash (<0.01ms)
        - Tier 2: Levenshtein Fuzzy (<0.5ms)
        - Tier 3: Local Vector Embedding (<4ms)
        """
        clean_prompt = prompt.strip().lower()
        h = hashlib.sha256(clean_prompt.encode("utf-8")).hexdigest()

        # Tier 1: Exact SHA-256 Hash Match
        if h in self.in_memory_hash_cache:
            return self.in_memory_hash_cache[h], "tier1_exact_hash"

        # Tier 2: Fuzzy Levenshtein Token-Sort Ratio
        best_fuzzy_score = 0.0
        best_fuzzy_skill = None
        for entry in self.cached_prompts:
            ratio = LevenshteinUtils.token_sort_ratio(prompt, entry["prompt"])
            if ratio > best_fuzzy_score:
                best_fuzzy_score = ratio
                best_fuzzy_skill = entry["skill_id"]

        if best_fuzzy_score >= fuzzy_threshold and best_fuzzy_skill:
            return best_fuzzy_skill, f"tier2_fuzzy_levenshtein ({best_fuzzy_score:.3f})"

        # Tier 3: Local Vector Embedding Cosine Search
        query_vec = self.embedder.embed(prompt)
        best_vector_score = 0.0
        best_vector_skill = None

        for entry in self.cached_prompts:
            emb = entry.get("embedding")
            if emb:
                score = cosine_similarity(query_vec, emb)
                if score > best_vector_score:
                    best_vector_score = score
                    best_vector_skill = entry["skill_id"]

        if best_vector_score >= vector_threshold and best_vector_skill:
            return best_vector_skill, f"tier3_vector_embedding ({best_vector_score:.3f})"

        return None, "miss"

class TripleLockStateSnapper:
    """Mid-Task State Snapping with Triple-Lock verification (Feature 1.9)."""

    @staticmethod
    def verify_and_snap(
        current_process: str,
        current_window_title: str,
        expected_process: str,
        expected_title_contains: str,
        anchor_matched: bool,
        anchor_confidence: float,
        goal_intent_matched: bool,
    ) -> Tuple[bool, str]:
        """
        Verifies the Triple-Lock Gate:
        1. Active process & title match expected milestone.
        2. Landmark visual anchors confirmed on screen (via local Radar NCC >= 0.88).
        3. High-level goal compatibility matches.
        """
        # Lock 1: Process and window title check
        proc_match = current_process.lower() == expected_process.lower()
        title_match = expected_title_contains.lower() in current_window_title.lower()
        if not (proc_match and title_match):
            return False, f"Lock 1 Failed: Process/Title mismatch ({current_process} vs {expected_process})"

        # Lock 2: Visual Anchor Verification
        if not anchor_matched or anchor_confidence < 0.88:
            return False, f"Lock 2 Failed: Visual anchor missing (confidence: {anchor_confidence:.3f} < 0.88)"

        # Lock 3: High-level goal compatibility
        if not goal_intent_matched:
            return False, "Lock 3 Failed: High-level goal compatibility mismatch"

        return True, "Triple-Lock Passed: Snapping out of reasoning into muscle memory Golden Path"
