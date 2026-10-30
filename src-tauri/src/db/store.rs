use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;
use std::path::Path;

pub const SCHEMA_SQL: &str = include_str!("schema.sql");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppShortcut {
    pub id: String,
    pub app_process: String,
    pub semantic_intent: String,
    pub hotkey_combo: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub id: String,
    pub prompt: String,
    pub normalized_intent: String,
    pub golden_path_json: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VisualAnchor {
    pub id: String,
    pub skill_id: String,
    pub step_index: i32,
    pub crop_blob: Vec<u8>,
    pub coords_x: i32,
    pub coords_y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guardrail {
    pub id: String,
    pub skill_id: String,
    pub faulty_step_index: i32,
    pub forbidden_action: String,
    pub correction_text: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PromptCacheEntry {
    pub sha256_hash: String,
    pub prompt_text: String,
    pub embedding_blob: Option<Vec<u8>>,
    pub skill_id: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRunRecord {
    pub id: String,
    pub prompt: String,
    pub normalized_intent: String,
    pub status: String,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrajectoryStepRecord {
    pub run_id: String,
    pub step_index: i32,
    pub proposal_json: String,
    pub verification_json: String,
    pub final_action_json: String,
    pub outcome: String,
    pub expected_outcome: String,
    #[serde(skip_serializing)]
    pub screenshot_webp: Option<Vec<u8>>,
    pub window_title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskStepAnchor {
    pub step_index: i32,
    pub crop_blob: Vec<u8>,
    pub coords_x: i32,
    pub coords_y: i32,
}

pub struct DbStore {
    conn: Connection,
}

impl DbStore {
    /// Open or create database at the given path and run migrations.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        let mut store = Self { conn };
        store.init_tables()?;
        Ok(store)
    }

    /// Open an in-memory database (useful for testing and zero-disk modes)
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut store = Self { conn };
        store.init_tables()?;
        Ok(store)
    }

    /// Initialize tables and indexes from schema.sql
    pub fn init_tables(&mut self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        Ok(())
    }

    /// Lookup shortcut for an application and semantic intent, falling back to 'global'
    pub fn get_shortcut(&self, app_process: &str, semantic_intent: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT hotkey_combo FROM AppShortcuts 
             WHERE (app_process = ?1 OR app_process = 'global') AND semantic_intent = ?2
             ORDER BY CASE WHEN app_process = ?1 THEN 0 ELSE 1 END
             LIMIT 1",
        )?;

        let mut rows = stmt.query(params![app_process, semantic_intent])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Insert or replace a learned application shortcut
    pub fn save_shortcut(
        &mut self,
        id: &str,
        app_process: &str,
        semantic_intent: &str,
        hotkey_combo: &str,
        source: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO AppShortcuts (id, app_process, semantic_intent, hotkey_combo, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, app_process, semantic_intent, hotkey_combo, source],
        )?;
        Ok(())
    }

    /// Insert or update a Golden Path skill
    pub fn save_skill(
        &mut self,
        id: &str,
        prompt: &str,
        normalized_intent: &str,
        golden_path_json: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO Skills (id, prompt, normalized_intent, golden_path_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, prompt, normalized_intent, golden_path_json],
        )?;
        Ok(())
    }

    /// Retrieve skill by ID
    pub fn get_skill(&self, id: &str) -> Result<Option<Skill>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, prompt, normalized_intent, golden_path_json, created_at 
             FROM Skills WHERE id = ?1",
        )?;

        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Skill {
                id: row.get(0)?,
                prompt: row.get(1)?,
                normalized_intent: row.get(2)?,
                golden_path_json: row.get(3)?,
                created_at: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Save a Visual Anchor crop
    pub fn save_visual_anchor(
        &mut self,
        id: &str,
        skill_id: &str,
        step_index: i32,
        crop_blob: &[u8],
        coords_x: i32,
        coords_y: i32,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO VisualAnchors (id, skill_id, step_index, crop_blob, coords_x, coords_y)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, skill_id, step_index, crop_blob, coords_x, coords_y],
        )?;
        Ok(())
    }

    /// Get all visual anchors for a skill
    pub fn get_visual_anchors(&self, skill_id: &str) -> Result<Vec<VisualAnchor>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, skill_id, step_index, crop_blob, coords_x, coords_y 
             FROM VisualAnchors WHERE skill_id = ?1 ORDER BY step_index ASC",
        )?;

        let rows = stmt.query_map(params![skill_id], |row| {
            Ok(VisualAnchor {
                id: row.get(0)?,
                skill_id: row.get(1)?,
                step_index: row.get(2)?,
                crop_blob: row.get(3)?,
                coords_x: row.get(4)?,
                coords_y: row.get(5)?,
            })
        })?;

        let mut anchors = Vec::new();
        for r in rows {
            anchors.push(r?);
        }
        Ok(anchors)
    }

    /// Save a Guardrail anti-pattern correction
    pub fn save_guardrail(
        &mut self,
        id: &str,
        skill_id: &str,
        faulty_step_index: i32,
        forbidden_action: &str,
        correction_text: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO Guardrails (id, skill_id, faulty_step_index, forbidden_action, correction_text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, skill_id, faulty_step_index, forbidden_action, correction_text],
        )?;
        Ok(())
    }

    /// Get all guardrails for a skill
    pub fn get_guardrails(&self, skill_id: &str) -> Result<Vec<Guardrail>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, skill_id, faulty_step_index, forbidden_action, correction_text, created_at 
             FROM Guardrails WHERE skill_id = ?1 ORDER BY faulty_step_index ASC",
        )?;

        let rows = stmt.query_map(params![skill_id], |row| {
            Ok(Guardrail {
                id: row.get(0)?,
                skill_id: row.get(1)?,
                faulty_step_index: row.get(2)?,
                forbidden_action: row.get(3)?,
                correction_text: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;

        let mut guardrails = Vec::new();
        for r in rows {
            guardrails.push(r?);
        }
        Ok(guardrails)
    }

    pub fn get_guardrails_for_intent(&self, normalized_intent: &str) -> Result<Vec<Guardrail>> {
        let mut stmt = self.conn.prepare(
            "SELECT g.id, g.skill_id, g.faulty_step_index, g.forbidden_action, g.correction_text, g.created_at
             FROM Guardrails g JOIN Skills s ON s.id = g.skill_id
             WHERE s.normalized_intent = ?1 ORDER BY g.faulty_step_index",
        )?;
        let rows = stmt.query_map(params![normalized_intent], |row| {
            Ok(Guardrail {
                id: row.get(0)?,
                skill_id: row.get(1)?,
                faulty_step_index: row.get(2)?,
                forbidden_action: row.get(3)?,
                correction_text: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// Save entry to PromptCache
    pub fn save_prompt_cache(
        &mut self,
        sha256_hash: &str,
        prompt_text: &str,
        embedding_blob: Option<&[u8]>,
        skill_id: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO PromptCache (sha256_hash, prompt_text, embedding_blob, skill_id)
             VALUES (?1, ?2, ?3, ?4)",
            params![sha256_hash, prompt_text, embedding_blob, skill_id],
        )?;
        Ok(())
    }

    /// Tier 1 Exact SHA-256 Prompt Lookup (<0.01ms)
    pub fn get_prompt_cache_by_hash(&self, sha256_hash: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT skill_id FROM PromptCache WHERE sha256_hash = ?1")?;
        let mut rows = stmt.query(params![sha256_hash])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn start_task_run(&mut self, id: &str, prompt: &str, normalized_intent: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO TaskRuns (id, prompt, normalized_intent, status) VALUES (?1, ?2, ?3, 'running')",
            params![id, prompt, normalized_intent],
        )?;
        Ok(())
    }

    pub fn finish_task_run(&mut self, id: &str, status: &str, summary: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE TaskRuns SET status = ?2, summary = ?3, completed_at = CURRENT_TIMESTAMP WHERE id = ?1",
            params![id, status, summary],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_trajectory_step(
        &mut self,
        run_id: &str,
        step_index: i32,
        proposal_json: &str,
        verification_json: &str,
        final_action_json: &str,
        outcome: &str,
        expected_outcome: &str,
        screenshot_webp: Option<&[u8]>,
        window_title: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO TrajectorySteps
             (run_id, step_index, proposal_json, verification_json, final_action_json, outcome, expected_outcome, screenshot_webp, window_title)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![run_id, step_index, proposal_json, verification_json, final_action_json, outcome, expected_outcome, screenshot_webp, window_title],
        )?;
        Ok(())
    }

    pub fn update_trajectory_outcome(
        &mut self,
        run_id: &str,
        step_index: i32,
        outcome: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE TrajectorySteps SET outcome = ?3 WHERE run_id = ?1 AND step_index = ?2",
            params![run_id, step_index, outcome],
        )?;
        Ok(())
    }

    pub fn save_task_step_anchor(
        &mut self,
        run_id: &str,
        step_index: i32,
        crop_blob: &[u8],
        coords_x: i32,
        coords_y: i32,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO TaskStepAnchors (run_id, step_index, crop_blob, coords_x, coords_y)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run_id, step_index, crop_blob, coords_x, coords_y],
        )?;
        Ok(())
    }

    pub fn get_task_step_anchors(&self, run_id: &str) -> Result<Vec<TaskStepAnchor>> {
        let mut stmt = self.conn.prepare(
            "SELECT step_index, crop_blob, coords_x, coords_y FROM TaskStepAnchors WHERE run_id = ?1 ORDER BY step_index",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            Ok(TaskStepAnchor {
                step_index: row.get(0)?,
                crop_blob: row.get(1)?,
                coords_x: row.get(2)?,
                coords_y: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_task_run(&self, id: &str) -> Result<Option<TaskRunRecord>> {
        self.conn
            .query_row(
                "SELECT id, prompt, normalized_intent, status, summary FROM TaskRuns WHERE id = ?1",
                params![id],
                |row| {
                    Ok(TaskRunRecord {
                        id: row.get(0)?,
                        prompt: row.get(1)?,
                        normalized_intent: row.get(2)?,
                        status: row.get(3)?,
                        summary: row.get(4)?,
                    })
                },
            )
            .optional()
    }

    pub fn get_trajectory_steps(&self, run_id: &str) -> Result<Vec<TrajectoryStepRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, step_index, proposal_json, verification_json, final_action_json,
                    outcome, expected_outcome, screenshot_webp, window_title
             FROM TrajectorySteps WHERE run_id = ?1 ORDER BY step_index",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            Ok(TrajectoryStepRecord {
                run_id: row.get(0)?,
                step_index: row.get(1)?,
                proposal_json: row.get(2)?,
                verification_json: row.get(3)?,
                final_action_json: row.get(4)?,
                outcome: row.get(5)?,
                expected_outcome: row.get(6)?,
                screenshot_webp: row.get(7)?,
                window_title: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    pub fn find_approved_skill(&self, normalized_intent: &str) -> Result<Option<Skill>> {
        let exact = self.conn
            .query_row(
                "SELECT id, prompt, normalized_intent, golden_path_json, created_at
                 FROM Skills WHERE normalized_intent = ?1 AND golden_path_json <> '[]' ORDER BY (id LIKE 'auto_%') ASC, created_at DESC LIMIT 1",
                params![normalized_intent],
                |row| {
                    Ok(Skill {
                        id: row.get(0)?,
                        prompt: row.get(1)?,
                        normalized_intent: row.get(2)?,
                        golden_path_json: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )
            .optional()?;
        if exact.is_some() {
            return Ok(exact);
        }

        let mut stmt = self.conn.prepare(
            "SELECT id, prompt, normalized_intent, golden_path_json, created_at FROM Skills WHERE golden_path_json <> '[]' ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Skill {
                id: row.get(0)?,
                prompt: row.get(1)?,
                normalized_intent: row.get(2)?,
                golden_path_json: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut best: Option<(Skill, f32)> = None;
        for row in rows {
            let skill = row?;
            let score = normalized_similarity(normalized_intent, &skill.normalized_intent);
            if score >= 0.88 && best.as_ref().is_none_or(|(_, best_score)| score > *best_score) {
                best = Some((skill, score));
            }
        }
        Ok(best.map(|(skill, _)| skill))
    }
}

fn normalized_similarity(left: &str, right: &str) -> f32 {
    if left == right {
        return 1.0;
    }
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (i, a) in left.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, b) in right.iter().enumerate() {
            current.push((current[j] + 1).min(previous[j + 1] + 1).min(previous[j] + usize::from(a != b)));
        }
        previous = current;
    }
    1.0 - previous[right.len()] as f32 / left.len().max(right.len()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_initialization_starts_empty() {
        let store = DbStore::open_in_memory().expect("Failed to initialize in-memory DB");
        assert_eq!(store.get_shortcut("chrome.exe", "focus_address_bar").unwrap(), None);
    }

    #[test]
    fn test_skills_and_visual_anchors() {
        let mut store = DbStore::open_in_memory().expect("Failed to initialize DB");

        // Save skill
        store
            .save_skill(
                "skill_1",
                "Export sales to PDF",
                "export_sales_pdf",
                r#"[{"action": "click", "x": 100, "y": 200}]"#,
            )
            .unwrap();

        let skill = store.get_skill("skill_1").unwrap().expect("Skill not found");
        assert_eq!(skill.prompt, "Export sales to PDF");
        assert_eq!(skill.normalized_intent, "export_sales_pdf");

        // Save visual anchor
        let fake_crop = vec![0u8; 64 * 64 * 4];
        store
            .save_visual_anchor("anchor_1", "skill_1", 1, &fake_crop, 100, 200)
            .unwrap();

        let anchors = store.get_visual_anchors("skill_1").unwrap();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].coords_x, 100);
        assert_eq!(anchors[0].coords_y, 200);
    }

    #[test]
    fn test_guardrails_and_prompt_cache() {
        let mut store = DbStore::open_in_memory().expect("Failed to initialize DB");

        store
            .save_skill(
                "skill_test",
                "Test Prompt",
                "test_intent",
                "[]",
            )
            .unwrap();

        // Save guardrail
        store
            .save_guardrail(
                "gr_1",
                "skill_test",
                2,
                "click_print",
                "Click Export -> Create PDF instead of Print",
            )
            .unwrap();

        let guardrails = store.get_guardrails("skill_test").unwrap();
        assert_eq!(guardrails.len(), 1);
        assert_eq!(guardrails[0].faulty_step_index, 2);
        assert_eq!(guardrails[0].forbidden_action, "click_print");

        // Save prompt cache
        let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        store
            .save_prompt_cache(hash, "Test Prompt", None, "skill_test")
            .unwrap();

        let cached_skill_id = store.get_prompt_cache_by_hash(hash).unwrap();
        assert_eq!(cached_skill_id, Some("skill_test".to_string()));
    }

    #[test]
    fn test_task_run_and_verified_trajectory_persistence() {
        let mut store = DbStore::open_in_memory().expect("Failed to initialize DB");
        store.start_task_run("run_1", "Open calculator", "open_calculator").unwrap();
        store
            .save_trajectory_step(
                "run_1",
                1,
                "{}",
                "{}",
                r#"{"type":"hotkey","keys":["win","r"]}"#,
                "uncertain",
                "Run dialog opens",
                Some(&[1, 2, 3]),
                "Desktop",
            )
            .unwrap();
        store.update_trajectory_outcome("run_1", 1, "succeeded").unwrap();
        store.finish_task_run("run_1", "completed", "done").unwrap();

        let run = store.get_task_run("run_1").unwrap().unwrap();
        assert_eq!(run.status, "completed");
        let steps = store.get_trajectory_steps("run_1").unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].outcome, "succeeded");
    }
}
