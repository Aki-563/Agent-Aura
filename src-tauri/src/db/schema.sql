-- Agent Aura Database Schema (SQLite)

-- 1. Skills: Distilled Golden Path workflows for known user goals
CREATE TABLE IF NOT EXISTS Skills (
    id TEXT PRIMARY KEY,
    prompt TEXT NOT NULL,
    normalized_intent TEXT NOT NULL,
    golden_path_json TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- 2. VisualAnchors: 64x64 pixel crops and coordinates for robust muscle memory replay
CREATE TABLE IF NOT EXISTS VisualAnchors (
    id TEXT PRIMARY KEY,
    skill_id TEXT NOT NULL,
    step_index INTEGER NOT NULL,
    crop_blob BLOB NOT NULL,
    coords_x INTEGER NOT NULL,
    coords_y INTEGER NOT NULL,
    FOREIGN KEY(skill_id) REFERENCES Skills(id) ON DELETE CASCADE
);

-- 3. AppShortcuts: Pre-loaded and learned keyboard shortcuts by application
CREATE TABLE IF NOT EXISTS AppShortcuts (
    id TEXT PRIMARY KEY,
    app_process TEXT NOT NULL,
    semantic_intent TEXT NOT NULL,
    hotkey_combo TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'learned'
);

-- 4. Guardrails: Anti-pattern mistake memory to prevent repeating past failures
CREATE TABLE IF NOT EXISTS Guardrails (
    id TEXT PRIMARY KEY,
    skill_id TEXT NOT NULL,
    faulty_step_index INTEGER NOT NULL,
    forbidden_action TEXT NOT NULL,
    correction_text TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(skill_id) REFERENCES Skills(id) ON DELETE CASCADE
);

-- 5. PromptCache: 3-Tier semantic prompt lookup (SHA-256 and ONNX embeddings)
CREATE TABLE IF NOT EXISTS PromptCache (
    sha256_hash TEXT PRIMARY KEY,
    prompt_text TEXT NOT NULL,
    embedding_blob BLOB,
    skill_id TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(skill_id) REFERENCES Skills(id) ON DELETE CASCADE
);

-- Indexes for lightning fast queries
CREATE INDEX IF NOT EXISTS idx_skills_normalized_intent ON Skills(normalized_intent);
CREATE INDEX IF NOT EXISTS idx_visual_anchors_skill ON VisualAnchors(skill_id, step_index);
CREATE INDEX IF NOT EXISTS idx_app_shortcuts_lookup ON AppShortcuts(app_process, semantic_intent);
CREATE INDEX IF NOT EXISTS idx_guardrails_skill ON Guardrails(skill_id);

-- Versioned production task history. Only real executions are recorded.
CREATE TABLE IF NOT EXISTS TaskRuns (
    id TEXT PRIMARY KEY,
    prompt TEXT NOT NULL,
    normalized_intent TEXT NOT NULL,
    status TEXT NOT NULL,
    summary TEXT,
    started_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    completed_at TIMESTAMP
);

CREATE TABLE IF NOT EXISTS TrajectorySteps (
    run_id TEXT NOT NULL,
    step_index INTEGER NOT NULL,
    proposal_json TEXT NOT NULL,
    verification_json TEXT NOT NULL,
    final_action_json TEXT NOT NULL,
    outcome TEXT NOT NULL,
    expected_outcome TEXT NOT NULL,
    screenshot_webp BLOB,
    window_title TEXT,
    PRIMARY KEY(run_id, step_index),
    FOREIGN KEY(run_id) REFERENCES TaskRuns(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS SchemaMigrations (
    version INTEGER PRIMARY KEY,
    applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS TaskStepAnchors (
    run_id TEXT NOT NULL,
    step_index INTEGER NOT NULL,
    crop_blob BLOB NOT NULL,
    coords_x INTEGER NOT NULL,
    coords_y INTEGER NOT NULL,
    PRIMARY KEY(run_id, step_index),
    FOREIGN KEY(run_id) REFERENCES TaskRuns(id) ON DELETE CASCADE
);

INSERT OR IGNORE INTO SchemaMigrations(version) VALUES (2);
CREATE INDEX IF NOT EXISTS idx_task_runs_intent ON TaskRuns(normalized_intent, status);
CREATE INDEX IF NOT EXISTS idx_trajectory_run ON TrajectorySteps(run_id, step_index);

-- Saved model connections (presets). The API key is DPAPI-encrypted (see vault.rs), never plain.
CREATE TABLE IF NOT EXISTS ModelPresets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    protocol TEXT NOT NULL,
    base_url TEXT NOT NULL,
    model TEXT NOT NULL,
    api_key_encrypted TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_used_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_model_presets_identity ON ModelPresets(protocol, base_url, model);
