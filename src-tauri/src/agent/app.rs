//! Commands behind the app's home screen and settings sheet: saved model connections (presets),
//! a cheap connection check for the status pill, automatic coordinate calibration of a newly saved
//! model, the step reader switch, dragging the mini player, and the run history the activity view
//! shows.

use super::*;
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetView {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    pub model: String,
    pub has_api_key: bool,
    pub active: bool,
    /// "pixels", "norm1000", "norm1" once measured; None before calibration.
    pub coordinates: Option<String>,
}

fn protocol_name(protocol: ApiProtocol) -> &'static str {
    match protocol {
        ApiProtocol::OpenAiResponses => "open_ai_responses",
        ApiProtocol::OpenAiCompatibleChat => "open_ai_compatible_chat",
    }
}

/// A readable default name: the model id without its vendor prefix, plus the server's host for
/// local servers ("qwen3-vl-4b · 10.103.7.32").
pub fn default_preset_name(protocol: ApiProtocol, base_url: &str, model: &str) -> String {
    let short = model.rsplit('/').next().unwrap_or(model);
    if protocol == ApiProtocol::OpenAiResponses {
        return short.to_owned();
    }
    let host = base_url.split("://").nth(1).unwrap_or(base_url).split(['/', ':']).next().unwrap_or("");
    if host.is_empty() { short.to_owned() } else { format!("{short} · {host}") }
}

fn view(preset: crate::db::ModelPreset, active: Option<&ProviderConfig>) -> PresetView {
    let is_active = active.is_some_and(|c| protocol_name(c.protocol) == preset.protocol && c.base_url == preset.base_url && c.model == preset.model);
    PresetView {
        coordinates: crate::coords::model_scale(&preset.model),
        has_api_key: !preset.api_key_encrypted.is_empty(),
        active: is_active,
        id: preset.id,
        name: preset.name,
        protocol: preset.protocol,
        base_url: preset.base_url,
        model: preset.model,
    }
}

/// Stores the active connection as a preset (key encrypted the same way as `provider.json`).
fn save_active_as_preset(state: &AgentRuntime, name: Option<&str>) -> Result<String, String> {
    let config = cloned_config(state)?;
    let encrypted = crate::vault::encrypt(&config.api_key)?;
    let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).unwrap_or_else(|| default_preset_name(config.protocol, &config.base_url, &config.model));
    let mut db = state.db.lock().map_err(|_| "Database lock was poisoned.".to_owned())?;
    db.save_model_preset(&name, protocol_name(config.protocol), &config.base_url, &config.model, &encrypted).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_model_presets(state: State<'_, AgentRuntime>) -> Result<Vec<PresetView>, String> {
    let active = state.config.lock().map_err(|_| "Provider configuration lock was poisoned.".to_owned())?.clone();
    let mut presets = state.db.lock().map_err(|_| "Database lock was poisoned.".to_owned())?.list_model_presets().map_err(|e| e.to_string())?;
    // A connection saved before presets existed becomes the first preset.
    if presets.is_empty() && active.is_some() {
        save_active_as_preset(&state, None)?;
        presets = state.db.lock().map_err(|_| "Database lock was poisoned.".to_owned())?.list_model_presets().map_err(|e| e.to_string())?;
    }
    Ok(presets.into_iter().map(|p| view(p, active.as_ref())).collect())
}

/// Saves the connection, checks it with a real model request, and keeps it as a preset. A model
/// whose coordinate convention is unknown is calibrated in the background afterwards.
#[tauri::command]
pub async fn connect_model(app: AppHandle, state: State<'_, AgentRuntime>, input: ProviderConfigInput, name: Option<String>) -> Result<PresetView, String> {
    configure_provider_inner(&state, input)?;
    check_connection(&state).await?;
    let id = save_active_as_preset(&state, name.as_deref())?;
    let config = cloned_config(&state)?;
    calibrate_in_background(app, config.model.clone());
    preset_view(&state, &id)
}

/// Makes a saved preset the active connection and checks it.
#[tauri::command]
pub async fn use_model_preset(app: AppHandle, state: State<'_, AgentRuntime>, id: String) -> Result<PresetView, String> {
    let preset = state.db.lock().map_err(|_| "Database lock was poisoned.".to_owned())?.get_model_preset(&id).map_err(|e| e.to_string())?.ok_or("That model is no longer saved.")?;
    let protocol: ApiProtocol = serde_json::from_value(Value::String(preset.protocol.clone())).map_err(|_| "The saved model has an unknown API type.".to_owned())?;
    let mut api_key = crate::vault::decrypt(&preset.api_key_encrypted)?;
    let result = configure_provider_inner(&state, ProviderConfigInput { protocol, base_url: preset.base_url.clone(), model: preset.model.clone(), api_key: api_key.clone() });
    api_key.zeroize();
    result?;
    if let Ok(mut db) = state.db.lock() {
        let _ = db.touch_model_preset(&id);
    }
    calibrate_in_background(app, preset.model.clone());
    preset_view(&state, &id)
}

#[tauri::command]
pub fn delete_model_preset(state: State<'_, AgentRuntime>, id: String) -> Result<ProviderStatus, String> {
    let preset = state.db.lock().map_err(|_| "Database lock was poisoned.".to_owned())?.get_model_preset(&id).map_err(|e| e.to_string())?;
    state.db.lock().map_err(|_| "Database lock was poisoned.".to_owned())?.delete_model_preset(&id).map_err(|e| e.to_string())?;
    // Deleting the model in use disconnects it too (its saved key goes with it).
    let mut config = state.config.lock().map_err(|_| "Provider configuration lock was poisoned.".to_owned())?;
    if let (Some(preset), Some(active)) = (preset, config.as_ref()) {
        if protocol_name(active.protocol) == preset.protocol && active.base_url == preset.base_url && active.model == preset.model {
            crate::vault::clear(&state.data_root);
            *config = None;
        }
    }
    Ok(status_from_config(config.as_ref()))
}

fn preset_view(state: &AgentRuntime, id: &str) -> Result<PresetView, String> {
    let active = state.config.lock().map_err(|_| "Provider configuration lock was poisoned.".to_owned())?.clone();
    let preset = state.db.lock().map_err(|_| "Database lock was poisoned.".to_owned())?.get_model_preset(id).map_err(|e| e.to_string())?.ok_or("The model was not saved.")?;
    Ok(view(preset, active.as_ref()))
}

/// The server answers and the model takes an image request.
async fn check_connection(state: &AgentRuntime) -> Result<(), String> {
    ping(state).await?;
    probe_model(state).await.map(|_| ())
}

/// Only asks the server for its model list (no model call): used for the status pill.
async fn ping(state: &AgentRuntime) -> Result<(), String> {
    let config = cloned_config(state)?;
    let response = state
        .client
        .get(endpoint(&config.base_url, "models"))
        .headers(auth_headers(&config)?)
        .timeout(Duration::from_secs(6))
        .send()
        .await
        .map_err(|error| format!("Cannot reach {}: {}", config.base_url, explain_request_error(&error)))?;
    if !response.status().is_success() {
        return Err(format!("{} answered HTTP {}", config.base_url, response.status().as_u16()));
    }
    Ok(())
}

#[tauri::command]
pub async fn ping_provider(state: State<'_, AgentRuntime>) -> Result<Value, String> {
    let model = cloned_config(&state).map(|c| c.model.clone()).ok();
    let Some(model) = model else {
        return Ok(json!({"state": "none"}));
    };
    Ok(match ping(&state).await {
        Ok(()) => json!({"state": "online", "model": model}),
        Err(error) => json!({"state": "offline", "model": model, "message": error}),
    })
}

// ---- Coordinate calibration on a generated picture -----------------------------------------

/// The labelled buttons drawn on the calibration picture: label and centre in pixels.
pub const CALIBRATION_SIZE: (u32, u32) = (1280, 800);
pub fn calibration_targets() -> Vec<(String, (i32, i32))> {
    vec![("Alpha".into(), (214, 156)), ("Beta".into(), (1046, 238)), ("Gamma".into(), (452, 622)), ("Delta".into(), (928, 690))]
}

/// A plain app-like picture with four labelled buttons at known places, drawn with GDI (so the
/// labels are real text a vision model can read). PNG bytes.
pub fn calibration_image() -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Graphics::Gdi::*;
    let (w, h) = (CALIBRATION_SIZE.0 as i32, CALIBRATION_SIZE.1 as i32);
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return Err("no drawing context".into());
        }
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB as u32, ..std::mem::zeroed() };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(dc);
            return Err("no bitmap".into());
        }
        let old_bitmap = SelectObject(dc, bitmap);
        let fill = |rect: windows_sys::Win32::Foundation::RECT, rgb: u32| {
            let brush = CreateSolidBrush(rgb);
            FillRect(dc, &rect, brush);
            DeleteObject(brush);
        };
        let rect = |l: i32, t: i32, r: i32, b: i32| windows_sys::Win32::Foundation::RECT { left: l, top: t, right: r, bottom: b };
        // COLORREF is 0x00BBGGRR.
        fill(rect(0, 0, w, h), 0x00F4F2F0);
        fill(rect(0, 0, w, 56), 0x00E6E2DE);
        let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
        let font = CreateFontW(-26, 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr());
        let old_font = SelectObject(dc, font);
        SetBkMode(dc, 1);
        let colours = [0x00D9822B, 0x0047A33A, 0x002E61D9, 0x00A0469B];
        for ((label, (cx, cy)), colour) in calibration_targets().into_iter().zip(colours) {
            let mut r = rect(cx - 90, cy - 30, cx + 90, cy + 30);
            fill(r, colour);
            SetTextColor(dc, 0x00FFFFFF);
            let text: Vec<u16> = label.encode_utf16().collect();
            DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
        SelectObject(dc, old_font);
        DeleteObject(font);
        let pixels = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
        let mut rgba = Vec::with_capacity(pixels.len());
        for px in pixels.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        SelectObject(dc, old_bitmap);
        DeleteObject(bitmap);
        DeleteDC(dc);
        let mut png = Vec::new();
        PngEncoder::new(&mut png).write_image(&rgba, w as u32, h as u32, ExtendedColorType::Rgba8).map_err(|e| e.to_string())?;
        Ok(png)
    }
}

/// Asks the active model to point at each target on `image` and decides its coordinate
/// convention. Returns (scale, mean miss in pixels, a line per answer).
pub async fn calibrate_on_image(state: &AgentRuntime, image: &str, size: (u32, u32), targets: &[(String, (i32, i32))]) -> Result<(&'static str, f64, Vec<String>), String> {
    let config = cloned_config(state)?;
    let mut answers = Vec::new();
    let mut lines = Vec::new();
    for (label, truth) in targets {
        let prompt = "You point at things on a screenshot. Reply with JSON only: {\"found\": true, \"x\": X, \"y\": Y} for the centre of the named thing, using your usual coordinate convention.";
        let input = format!("Point at the centre of: {label}");
        let value = request_plan(&state.client, &config, prompt, &input, image, model_schema::GROUNDING).await?;
        let text = extract_model_text(&value, config.protocol)?;
        let reply: Value = serde_json::from_str(model_schema::extract_json_object(&text).unwrap_or("{}")).unwrap_or_default();
        let number = |k: &str| reply.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())));
        match (number("x"), number("y")) {
            (Some(x), Some(y)) => {
                lines.push(format!("{label}: model said ({x:.0}, {y:.0}), true centre ({}, {})", truth.0, truth.1));
                answers.push(((x, y), *truth));
            }
            _ => lines.push(format!("{label}: no usable point in {text:?}")),
        }
    }
    if answers.len() < 2 {
        return Err(format!("too few usable answers to decide ({} of {})", answers.len(), targets.len()));
    }
    let (scale, error) = crate::coords::best_scale(&answers, size);
    Ok((scale, error, lines))
}

/// Calibrates the active model on the generated picture and saves the result.
pub async fn calibrate_active_model(state: &AgentRuntime) -> Result<(String, f64), String> {
    let model = cloned_config(state)?.model.clone();
    let png = tokio::task::spawn_blocking(calibration_image).await.map_err(|e| e.to_string())??;
    let image = format!("data:image/png;base64,{}", BASE64.encode(&png));
    let (scale, error, _) = calibrate_on_image(state, &image, CALIBRATION_SIZE, &calibration_targets()).await?;
    // A model that cannot point at big labelled buttons at all is not given a convention.
    if error > 120.0 {
        return Err(format!("the model's answers were too far off to tell (mean miss {error:.0} px)"));
    }
    crate::coords::save_model_scale(&model, scale)?;
    Ok((scale.to_owned(), error))
}

fn calibrate_in_background(app: AppHandle, model: String) {
    if crate::coords::model_scale(&model).is_some() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AgentRuntime>();
        queue_event("model-calibration", &json!({"model": model, "state": "running"}));
        let outcome = calibrate_active_model(&state).await;
        let payload = match outcome {
            Ok((scale, error)) => json!({"model": model, "state": "done", "coordinates": scale, "missPx": error.round()}),
            Err(error) => json!({"model": model, "state": "failed", "message": error}),
        };
        queue_event("model-calibration", &payload);
    });
}

/// Runs the calibration again for the active model (settings sheet).
#[tauri::command]
pub async fn calibrate_model(state: State<'_, AgentRuntime>) -> Result<Value, String> {
    let (scale, error) = calibrate_active_model(&state).await?;
    Ok(json!({"coordinates": scale, "missPx": error.round()}))
}

// ---- Step reader, mini player --------------------------------------------------------------

#[tauri::command]
pub fn get_voice_enabled() -> bool {
    crate::settings::get().voice.enabled
}

#[tauri::command]
pub fn set_voice_enabled(enabled: bool) -> Result<bool, String> {
    let mut settings = crate::settings::get();
    settings.voice.enabled = enabled;
    crate::settings::set(settings)?;
    if !enabled {
        crate::voice::stop_speaking();
    }
    Ok(enabled)
}

/// The mini player is dragged by its bar. When the button is let go, the place is remembered and
/// used for the next run.
#[tauri::command]
pub fn start_window_drag(window: WebviewWindow) -> Result<(), String> {
    window.start_dragging().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
        let started = Instant::now();
        // Wait for the release (the drag loop runs until then), at most a minute.
        while started.elapsed() < Duration::from_secs(60) && unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } as u16 & 0x8000 != 0 {
            std::thread::sleep(Duration::from_millis(30));
        }
        std::thread::sleep(Duration::from_millis(60));
        panel::remember_position(&window);
    });
    Ok(())
}

// ---- Activity ----------------------------------------------------------------------------------

#[tauri::command]
pub fn list_task_runs(state: State<'_, AgentRuntime>, limit: Option<u32>) -> Result<Vec<crate::db::TaskRunSummary>, String> {
    let db = state.db.lock().map_err(|_| "Database lock was poisoned.".to_owned())?;
    db.recent_task_runs(limit.unwrap_or(30).clamp(1, 200)).map_err(|e| e.to_string())
}

/// One run with its steps in order: what the model meant, what was done, how it ended, and the
/// screenshot it looked at.
#[tauri::command]
pub fn get_task_run_detail(state: State<'_, AgentRuntime>, id: String) -> Result<Value, String> {
    let db = state.db.lock().map_err(|_| "Database lock was poisoned.".to_owned())?;
    let summary = db.recent_task_runs(500).map_err(|e| e.to_string())?.into_iter().find(|r| r.id == id).ok_or("That run is no longer stored.")?;
    let steps: Vec<Value> = db
        .get_trajectory_steps(&id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|step| {
            let proposal: Value = serde_json::from_str(&step.proposal_json).unwrap_or(Value::Null);
            let action: Value = serde_json::from_str(&step.final_action_json).unwrap_or(Value::Null);
            json!({
                "index": step.step_index,
                "thought": proposal.get("thought").and_then(Value::as_str).unwrap_or(""),
                "target": proposal.pointer("/target/description").and_then(Value::as_str).unwrap_or(""),
                "expected": step.expected_outcome,
                "action": action,
                "outcome": step.outcome,
                "window": step.window_title.unwrap_or_default(),
                "screenshot": step.screenshot_webp.map(|bytes| format!("data:{};base64,{}", image_mime(&bytes), BASE64.encode(bytes))),
            })
        })
        .collect();
    Ok(json!({"run": summary, "steps": steps}))
}

fn image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.starts_with(b"RIFF") {
        "image/webp"
    } else {
        "image/png"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_names_are_short_and_say_where() {
        assert_eq!(default_preset_name(ApiProtocol::OpenAiCompatibleChat, "http://10.103.7.32:1234/v1", "qwen/qwen3-vl-4b"), "qwen3-vl-4b · 10.103.7.32");
        assert_eq!(default_preset_name(ApiProtocol::OpenAiResponses, "https://api.openai.com/v1", "gpt-x"), "gpt-x");
    }

    #[test]
    fn the_calibration_picture_has_its_buttons_where_the_targets_say() {
        let png = calibration_image().unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), CALIBRATION_SIZE);
        for (label, (x, y)) in calibration_targets() {
            // Just inside the left edge of the button: the button colour, not the background.
            let px = image.get_pixel((x - 80) as u32, y as u32);
            assert!(px[0] < 220 || px[1] < 220 || px[2] < 220, "{label} button is drawn: {px:?}");
            // Somewhere in the middle there is white text.
            let white = (x - 60..x + 60).any(|tx| image.get_pixel(tx as u32, y as u32).0[..3].iter().all(|c| *c > 230));
            assert!(white, "{label} has a readable label");
        }
        // The background far from any button is the light page colour.
        assert!(image.get_pixel(640, 420).0[..3].iter().all(|c| *c > 230));
    }
}
