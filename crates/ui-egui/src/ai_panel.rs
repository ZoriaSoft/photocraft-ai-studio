//! AI-first command planning panel.
//!
//! The first implementation is deliberately local and deterministic: it turns a small set of
//! natural-language intents into the same engine commands the rest of the app uses. A remote or
//! local model can replace the planner later, while [`validate`] remains the boundary that decides
//! which commands an AI plan is allowed to execute.

use egui::{Color32, CornerRadius, RichText, Stroke, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::theme::{self, Tokens};

/// UI state persisted with the rest of the workspace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiPanelState {
    /// 0 = Assistant, 1 = Workflows.
    pub tab: usize,
    pub prompt: String,
    pub plan_title: String,
    pub plan: Vec<AiStep>,
    pub status: String,
    pub status_error: bool,
    pub recent_prompts: Vec<String>,
    /// Persisted separately from generic workspace/UI state.
    #[serde(skip)]
    pub saved_workflows: Vec<AiWorkflow>,
    #[serde(skip)]
    pub workflow_name: String,
}

impl Default for AiPanelState {
    fn default() -> Self {
        Self {
            tab: 0,
            prompt: String::new(),
            plan_title: String::new(),
            plan: Vec::new(),
            status: "Ready for a request".into(),
            status_error: false,
            recent_prompts: Vec::new(),
            saved_workflows: Vec::new(),
            workflow_name: String::new(),
        }
    }
}

/// One planned editor action. `command = None` means the step needs a capability that is not wired
/// yet (for example, vision-based subject isolation).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiStep {
    pub label: String,
    pub command: Option<String>,
    pub params: Value,
    pub note: String,
}

impl AiStep {
    fn command(label: &str, command: &str, params: Value) -> Self {
        Self { label: label.into(), command: Some(command.into()), params, note: String::new() }
    }

    fn pending(label: &str, note: &str) -> Self {
        Self { label: label.into(), command: None, params: Value::Null, note: note.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedRequest {
    pub title: String,
    pub steps: Vec<AiStep>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiWorkflow {
    pub name: String,
    pub steps: Vec<AiStep>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct WorkflowStore {
    version: u32,
    workflows: Vec<AiWorkflow>,
}

impl Default for WorkflowStore {
    fn default() -> Self {
        Self { version: WORKFLOW_STORE_VERSION, workflows: Vec::new() }
    }
}

const WORKFLOW_STORE_VERSION: u32 = 1;
const MAX_SAVED_WORKFLOWS: usize = 50;
const MAX_PLAN_STEPS: usize = 24;
const MAX_STEP_LABEL_CHARS: usize = 160;
const MAX_PENDING_NOTE_CHARS: usize = 500;

/// Provider-neutral request passed to the native model service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiPlannerRequest {
    pub prompt: String,
    /// Small, non-pixel document summary. Image data is never sent by this planning layer.
    pub context: Value,
}

/// Commands that an AI-generated plan may execute without adding a new review rule.
///
/// Keep this list intentionally small. New commands should be reviewed for destructive behaviour
/// before they are added here.
const SAFE_COMMANDS: &[&str] = &[
    "layer.new.layer",
    "layer.new.group",
    "layer.groupLayers",
    "layer.duplicate",
    "layer.setProps",
    "layer.arrange.bringForward",
    "layer.arrange.sendBackward",
    "layer.arrange.bringToFront",
    "layer.arrange.sendToBack",
    "layer.createClippingMask",
    "layer.releaseClippingMask",
    "layer.newAdjustmentLayer.brightnessContrast",
    "layer.newAdjustmentLayer.curves",
    "layer.newAdjustmentLayer.vibrance",
    "layer.newAdjustmentLayer.blackWhite",
    "layer.newAdjustmentLayer.invert",
];

fn param_object<'a>(command: &str, params: &'a Value) -> Result<Option<&'a serde_json::Map<String, Value>>, String> {
    match params {
        Value::Null => Ok(None),
        Value::Object(map) => Ok(Some(map)),
        _ => Err(format!("Parameters for `{command}` must be a JSON object.")),
    }
}

fn only_keys(command: &str, map: Option<&serde_json::Map<String, Value>>, allowed: &[&str]) -> Result<(), String> {
    let Some(map) = map else { return Ok(()) };
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("Parameter `{key}` is not allowed for AI command `{command}`."));
        }
    }
    Ok(())
}

fn no_params(command: &str, map: Option<&serde_json::Map<String, Value>>) -> Result<(), String> {
    if map.is_some_and(|map| !map.is_empty()) {
        return Err(format!("AI command `{command}` does not accept parameters."));
    }
    Ok(())
}

fn optional_name(command: &str, map: Option<&serde_json::Map<String, Value>>) -> Result<(), String> {
    let Some(value) = map.and_then(|map| map.get("name")) else { return Ok(()) };
    let Some(name) = value.as_str() else { return Err(format!("`{command}.name` must be a string.")) };
    let len = name.chars().count();
    if name.trim().is_empty() || len > 128 {
        return Err(format!("`{command}.name` must contain 1 to 128 characters."));
    }
    Ok(())
}

fn optional_bool(command: &str, map: Option<&serde_json::Map<String, Value>>, key: &str) -> Result<(), String> {
    if map.and_then(|map| map.get(key)).is_some_and(|value| !value.is_boolean()) {
        return Err(format!("`{command}.{key}` must be a boolean."));
    }
    Ok(())
}

fn optional_number(command: &str, map: Option<&serde_json::Map<String, Value>>, key: &str, min: f64, max: f64) -> Result<(), String> {
    let Some(value) = map.and_then(|map| map.get(key)) else { return Ok(()) };
    let Some(number) = value.as_f64() else { return Err(format!("`{command}.{key}` must be a number.")) };
    if !number.is_finite() || !(min..=max).contains(&number) {
        return Err(format!("`{command}.{key}` must be between {min} and {max}."));
    }
    Ok(())
}

fn optional_blend(command: &str, map: Option<&serde_json::Map<String, Value>>) -> Result<(), String> {
    let Some(value) = map.and_then(|map| map.get("blend")) else { return Ok(()) };
    let Some(blend) = value.as_str() else { return Err(format!("`{command}.blend` must be a string.")) };
    let norm = |value: &str| value.to_ascii_lowercase().replace([' ', '_', '-', '(', ')'], "");
    let want = norm(blend);
    let known = std::iter::once(photocraft_color::BlendMode::PassThrough)
        .chain(photocraft_color::BlendMode::LAYER_MODES)
        .any(|mode| norm(mode.label()) == want || norm(&format!("{mode:?}")) == want);
    if !known {
        return Err(format!("`{blend}` is not a supported blend mode."));
    }
    Ok(())
}

fn curve_points(command: &str, map: Option<&serde_json::Map<String, Value>>) -> Result<(), String> {
    let Some(value) = map.and_then(|map| map.get("points")) else { return Ok(()) };
    let Some(points) = value.as_array() else { return Err(format!("`{command}.points` must be an array.")) };
    if !(2..=32).contains(&points.len()) {
        return Err(format!("`{command}.points` must contain between 2 and 32 points."));
    }
    let mut previous_input = None;
    for point in points {
        let Some(pair) = point.as_array() else { return Err(format!("Every `{command}.points` entry must be [input, output].")) };
        if pair.len() != 2 {
            return Err(format!("Every `{command}.points` entry must contain exactly two numbers."));
        }
        let (Some(input), Some(output)) = (pair.first().and_then(Value::as_f64), pair.get(1).and_then(Value::as_f64)) else {
            return Err(format!("Every `{command}.points` entry must contain numbers."));
        };
        if !input.is_finite() || !output.is_finite() || !(0.0..=255.0).contains(&input) || !(0.0..=255.0).contains(&output) {
            return Err(format!("`{command}.points` values must be finite numbers from 0 to 255."));
        }
        if previous_input.is_some_and(|previous| input <= previous) {
            return Err(format!("`{command}.points` inputs must be strictly increasing."));
        }
        previous_input = Some(input);
    }
    Ok(())
}

fn validate_safe_params(command: &str, params: &Value) -> Result<(), String> {
    let map = param_object(command, params)?;
    match command {
        "layer.new.layer" | "layer.new.group" | "layer.groupLayers" => {
            only_keys(command, map, &["name"])?;
            optional_name(command, map)
        }
        "layer.duplicate"
        | "layer.arrange.bringForward"
        | "layer.arrange.sendBackward"
        | "layer.arrange.bringToFront"
        | "layer.arrange.sendToBack"
        | "layer.createClippingMask"
        | "layer.releaseClippingMask"
        | "layer.newAdjustmentLayer.blackWhite"
        | "layer.newAdjustmentLayer.invert" => no_params(command, map),
        "layer.setProps" => {
            only_keys(command, map, &["name", "visible", "opacity", "fill", "blend"])?;
            if map.is_none_or(serde_json::Map::is_empty) {
                return Err("AI command `layer.setProps` needs at least one editable property.".into());
            }
            optional_name(command, map)?;
            optional_bool(command, map, "visible")?;
            optional_number(command, map, "opacity", 0.0, 1.0)?;
            optional_number(command, map, "fill", 0.0, 1.0)?;
            optional_blend(command, map)
        }
        "layer.newAdjustmentLayer.brightnessContrast" => {
            only_keys(command, map, &["brightness", "contrast"])?;
            optional_number(command, map, "brightness", -150.0, 150.0)?;
            optional_number(command, map, "contrast", -50.0, 100.0)
        }
        "layer.newAdjustmentLayer.vibrance" => {
            only_keys(command, map, &["vibrance", "saturation"])?;
            optional_number(command, map, "vibrance", -100.0, 100.0)?;
            optional_number(command, map, "saturation", -100.0, 100.0)
        }
        "layer.newAdjustmentLayer.curves" => {
            only_keys(command, map, &["points"])?;
            curve_points(command, map)
        }
        _ => Err(format!("Command `{command}` has no reviewed AI parameter policy.")),
    }
}

/// Validate the trust boundary between a planner and the editor engine.
fn validate_steps(steps: &[AiStep], require_executable: bool) -> Result<(), String> {
    if steps.is_empty() {
        return Err("The plan has no steps.".into());
    }
    if steps.len() > MAX_PLAN_STEPS {
        return Err(format!("The plan has too many steps (maximum {MAX_PLAN_STEPS})."));
    }
    for step in steps {
        let label_len = step.label.trim().chars().count();
        if !(1..=MAX_STEP_LABEL_CHARS).contains(&label_len) {
            return Err(format!("Every plan step needs a label between 1 and {MAX_STEP_LABEL_CHARS} characters."));
        }
        let Some(command) = step.command.as_deref() else {
            if require_executable {
                return Err(format!("{} needs an AI capability that is not connected yet.", step.label));
            }
            let note_len = step.note.trim().chars().count();
            if !(1..=MAX_PENDING_NOTE_CHARS).contains(&note_len) {
                return Err(format!("{} needs a pending-note between 1 and {MAX_PENDING_NOTE_CHARS} characters.", step.label));
            }
            continue;
        };
        if !SAFE_COMMANDS.contains(&command) {
            return Err(format!("Command `{command}` is not in the AI safety allowlist."));
        }
        validate_safe_params(command, &step.params)?;
    }
    Ok(())
}

/// A plan may only run when every step maps to a reviewed editor command.
pub fn validate(steps: &[AiStep]) -> Result<(), String> {
    validate_steps(steps, true)
}

/// Validate model output before it is accepted into UI state. Pending steps are allowed so the
/// model can explicitly say a capability is unavailable; unknown commands are never accepted.
pub fn validate_model_plan(plan: &PlannedRequest) -> Result<(), String> {
    if plan.title.trim().is_empty() {
        return Err("The model returned a plan without a title.".into());
    }
    validate_steps(&plan.steps, false)
}

/// Contract sent to model providers. The provider can phrase the prompt however it wants, but this
/// is the complete command surface that remote planning is allowed to produce.
pub fn planner_contract() -> Value {
    json!({
        "result": {
            "title": "short descriptive string",
            "steps": [{
                "label": "human-readable action",
                "command": "one allowed command id, or null when the capability is unavailable",
                "params": {},
                "note": "empty for executable steps; explain why a null-command step is pending"
            }]
        },
        "allowedCommands": [
            {"id": "layer.new.layer", "params": {"name": "optional non-empty string, max 128 chars"}},
            {"id": "layer.new.group", "params": {"name": "optional non-empty string, max 128 chars"}},
            {"id": "layer.groupLayers", "params": {"name": "optional group name; groups the current selection"}},
            {"id": "layer.duplicate", "params": {}},
            {"id": "layer.setProps", "params": {"name": "optional string", "visible": "optional bool", "opacity": "optional number 0..1", "fill": "optional number 0..1", "blend": "optional supported blend-mode name"}},
            {"id": "layer.arrange.bringForward", "params": {}},
            {"id": "layer.arrange.sendBackward", "params": {}},
            {"id": "layer.arrange.bringToFront", "params": {}},
            {"id": "layer.arrange.sendToBack", "params": {}},
            {"id": "layer.createClippingMask", "params": {}},
            {"id": "layer.releaseClippingMask", "params": {}},
            {"id": "layer.newAdjustmentLayer.brightnessContrast", "params": {"brightness": "optional number -150..150", "contrast": "optional number -50..100"}},
            {"id": "layer.newAdjustmentLayer.curves", "params": {"points": "optional 2..32 [input, output] pairs; each value 0..255; inputs strictly increasing"}},
            {"id": "layer.newAdjustmentLayer.vibrance", "params": {"vibrance": "optional number -100..100", "saturation": "optional number -100..100"}},
            {"id": "layer.newAdjustmentLayer.blackWhite", "params": {}},
            {"id": "layer.newAdjustmentLayer.invert", "params": {}}
        ],
        "rules": [
            "Return JSON only, with exactly title and steps at the top level.",
            "Never invent a command id or parameter. Use null when the request needs a capability outside the allowed commands.",
            "Never send a layer id. All allowed layer commands intentionally operate on the active layer or current selection.",
            "Prefer non-destructive adjustment layers and keep the document editable.",
            "Do not claim a vision, selection, delete, export, filesystem, or network action happened when no allowed command can perform it.",
            "Keep plans short and directly related to the user's request."
        ]
    })
}

/// Local MVP planner. It intentionally recognizes a small vocabulary; the UI makes unsupported
/// intent explicit rather than pretending an edit can be completed.
pub fn plan(prompt: &str) -> PlannedRequest {
    let p = prompt.trim();
    if p.is_empty() {
        return PlannedRequest { title: "Nothing to plan".into(), steps: Vec::new() };
    }
    let q = p.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| q.contains(needle));

    if has(&["product photo", "product shot", "e-commerce", "ecommerce", "online store", "ürün foto", "urun foto", "mağaza", "magaza"]) {
        return PlannedRequest {
            title: "Product photo preparation".into(),
            steps: vec![
                AiStep::pending("Isolate the product", "Subject isolation will be enabled when the vision backend is connected."),
                AiStep::command("Lift tone and contrast", "layer.newAdjustmentLayer.brightnessContrast", json!({"brightness": 12, "contrast": 6})),
                AiStep::command("Add restrained vibrance", "layer.newAdjustmentLayer.vibrance", json!({"vibrance": 16, "saturation": 2})),
            ],
        };
    }

    let mut steps = Vec::new();

    if has(&["duplicate", "copy layer", "çoğalt", "cogalt", "katmanı kopyala", "katmani kopyala"]) {
        steps.push(AiStep::command("Duplicate the active layer", "layer.duplicate", json!({})));
    }
    if has(&["new layer", "empty layer", "yeni katman", "boş katman", "bos katman"]) {
        steps.push(AiStep::command("Create a new layer", "layer.new.layer", json!({"name": "AI Layer"})));
    }
    if has(&["new group", "yeni grup", "grup oluştur", "grup olustur"]) {
        steps.push(AiStep::command("Create a layer group", "layer.new.group", json!({"name": "AI Group"})));
    }
    if has(&["group layers", "group selected layers", "katmanları grupla", "katmanlari grupla"]) {
        steps.push(AiStep::command("Group the selected layers", "layer.groupLayers", json!({})));
    }
    if has(&["hide active layer", "hide layer", "katmanı gizle", "katmani gizle"]) {
        steps.push(AiStep::command("Hide the active layer", "layer.setProps", json!({"visible": false})));
    } else if has(&["show active layer", "show layer", "katmanı göster", "katmani goster"]) {
        steps.push(AiStep::command("Show the active layer", "layer.setProps", json!({"visible": true})));
    }
    if has(&["bring to front", "move to front", "en öne getir", "en one getir"]) {
        steps.push(AiStep::command("Bring the active layer to front", "layer.arrange.bringToFront", json!({})));
    } else if has(&["send to back", "move to back", "en arkaya gönder", "en arkaya gonder"]) {
        steps.push(AiStep::command("Send the active layer to back", "layer.arrange.sendToBack", json!({})));
    } else if has(&["bring forward", "move forward", "öne getir", "one getir"]) {
        steps.push(AiStep::command("Bring the active layer forward", "layer.arrange.bringForward", json!({})));
    } else if has(&["send backward", "move backward", "arkaya gönder", "arkaya gonder"]) {
        steps.push(AiStep::command("Send the active layer backward", "layer.arrange.sendBackward", json!({})));
    }
    if has(&["release clipping mask", "remove clipping mask", "kırpma maskesini kaldır", "kirpma maskesini kaldir"]) {
        steps.push(AiStep::command("Release the clipping mask", "layer.releaseClippingMask", json!({})));
    } else if has(&["create clipping mask", "make clipping mask", "kırpma maskesi oluştur", "kirpma maskesi olustur"]) {
        steps.push(AiStep::command("Create a clipping mask", "layer.createClippingMask", json!({})));
    }
    if has(&["black and white", "black & white", "monochrome", "siyah beyaz"]) {
        steps.push(AiStep::command("Add a Black & White adjustment", "layer.newAdjustmentLayer.blackWhite", json!({})));
    } else if has(&["invert", "negative", "negatif", "ters renk"]) {
        steps.push(AiStep::command("Add an Invert adjustment", "layer.newAdjustmentLayer.invert", json!({})));
    }
    if has(&["curve", "curves", "eğrid", "egri"]) {
        steps.push(AiStep::command(
            "Add a gentle contrast curve",
            "layer.newAdjustmentLayer.curves",
            json!({"points": [[0, 0], [64, 58], [128, 132], [192, 202], [255, 255]]}),
        ));
    }
    if has(&["bright", "brighter", "brightness", "parlak", "aydınlat", "aydinlat", "contrast", "kontrast"]) {
        steps.push(AiStep::command("Lift brightness and contrast", "layer.newAdjustmentLayer.brightnessContrast", json!({"brightness": 12, "contrast": 6})));
    }
    if has(&["vibrance", "vibrant", "more color", "canlı", "canli", "renkleri güç", "renkleri guc"]) {
        steps.push(AiStep::command("Increase vibrance", "layer.newAdjustmentLayer.vibrance", json!({"vibrance": 18, "saturation": 2})));
    }

    if steps.is_empty() {
        steps.push(AiStep::pending(
            "Interpret this creative request",
            "The local planner does not recognize this request yet. A model-backed planner will handle open-ended edits.",
        ));
    }

    PlannedRequest { title: "Edit plan".into(), steps }
}

fn workflow_name_ok(name: &str) -> bool {
    let len = name.trim().chars().count();
    (1..=64).contains(&len)
}

fn parse_saved_workflows(text: &str) -> Result<Vec<AiWorkflow>, String> {
    let store: WorkflowStore = serde_json::from_str(text).map_err(|error| format!("Couldn't parse saved AI workflows: {error}"))?;
    if store.version > WORKFLOW_STORE_VERSION {
        return Err(format!("Saved AI workflows use unsupported schema version {}.", store.version));
    }
    let mut valid = Vec::new();
    for workflow in store.workflows.into_iter().take(MAX_SAVED_WORKFLOWS) {
        if workflow_name_ok(&workflow.name) && validate(&workflow.steps).is_ok() {
            valid.push(workflow);
        }
    }
    Ok(valid)
}

fn serialize_saved_workflows(workflows: &[AiWorkflow]) -> Result<String, String> {
    let store = WorkflowStore { version: WORKFLOW_STORE_VERSION, workflows: workflows.iter().take(MAX_SAVED_WORKFLOWS).cloned().collect() };
    serde_json::to_string_pretty(&store).map_err(|error| format!("Couldn't serialize AI workflows: {error}"))
}

pub fn load_saved_workflows(app: &mut PhotocraftApp) {
    let Some(load) = app.services.load_ai_workflows.as_mut() else { return };
    let Some(text) = load() else { return };
    match parse_saved_workflows(&text) {
        Ok(workflows) => app.ui.ai.saved_workflows = workflows,
        Err(error) => {
            app.ui.ai.status = error;
            app.ui.ai.status_error = true;
        }
    }
}

fn persist_saved_workflows(app: &mut PhotocraftApp, workflows: &[AiWorkflow]) -> Result<(), String> {
    let text = serialize_saved_workflows(workflows)?;
    let Some(save) = app.services.save_ai_workflows.as_mut() else { return Ok(()) };
    save(&text)
}

fn save_current_workflow(app: &mut PhotocraftApp) -> Result<(), String> {
    validate(&app.ui.ai.plan)?;
    let name = app.ui.ai.workflow_name.trim().to_string();
    if !workflow_name_ok(&name) {
        return Err("Workflow name must contain 1 to 64 characters.".into());
    }
    let workflow = AiWorkflow { name: name.clone(), steps: app.ui.ai.plan.clone() };
    let mut next = app.ui.ai.saved_workflows.clone();
    if let Some(existing) = next.iter_mut().find(|workflow| workflow.name.eq_ignore_ascii_case(&name)) {
        *existing = workflow;
    } else {
        next.insert(0, workflow);
        next.truncate(MAX_SAVED_WORKFLOWS);
    }
    persist_saved_workflows(app, &next)?;
    app.ui.ai.saved_workflows = next;
    app.ui.ai.status = format!("Saved workflow: {name}");
    app.ui.ai.status_error = false;
    Ok(())
}

fn remove_saved_workflow(app: &mut PhotocraftApp, index: usize) -> Result<(), String> {
    if index >= app.ui.ai.saved_workflows.len() {
        return Err("Saved workflow no longer exists.".into());
    }
    let mut next = app.ui.ai.saved_workflows.clone();
    next.remove(index);
    persist_saved_workflows(app, &next)?;
    app.ui.ai.saved_workflows = next;
    app.ui.ai.status = "Removed saved workflow.".into();
    app.ui.ai.status_error = false;
    Ok(())
}

fn load_workflow(app: &mut PhotocraftApp, index: usize) -> Result<(), String> {
    let Some(workflow) = app.ui.ai.saved_workflows.get(index).cloned() else {
        return Err("Saved workflow no longer exists.".into());
    };
    validate(&workflow.steps)?;
    app.ui.ai.prompt.clear();
    app.ui.ai.plan_title = workflow.name.clone();
    app.ui.ai.plan = workflow.steps;
    app.ui.ai.workflow_name = workflow.name.clone();
    app.ui.ai.status = format!("Loaded workflow: {}", workflow.name);
    app.ui.ai.status_error = false;
    app.ui.ai.tab = 0;
    Ok(())
}

fn remember_prompt(app: &mut PhotocraftApp, prompt: &str) {
    let ai = &mut app.ui.ai;
    ai.prompt = prompt.trim().to_string();
    if !ai.prompt.is_empty() {
        ai.recent_prompts.retain(|p| p != &ai.prompt);
        ai.recent_prompts.insert(0, ai.prompt.clone());
        ai.recent_prompts.truncate(6);
    }
}

fn apply_plan(app: &mut PhotocraftApp, prompt: String, planned: PlannedRequest, source: &str) {
    remember_prompt(app, &prompt);
    let ai = &mut app.ui.ai;
    ai.plan_title = planned.title;
    ai.plan = planned.steps;
    ai.workflow_name = ai.plan_title.clone();
    ai.status_error = false;
    ai.status = match validate(&ai.plan) {
        Ok(()) => format!("{} step{} ready · {source}", ai.plan.len(), if ai.plan.len() == 1 { "" } else { "s" }),
        Err(reason) => reason,
    };
}

fn planner_request(app: &PhotocraftApp, prompt: &str) -> AiPlannerRequest {
    let context = app.session.active().map_or_else(
        || json!({"documentOpen": false}),
        |state| {
            json!({
                "documentOpen": true,
                "width": state.doc.size.width,
                "height": state.doc.size.height,
                "layers": state.doc.layers.len(),
                "colorMode": format!("{:?}", state.doc.mode),
                "depth": format!("{:?}", state.doc.depth)
            })
        },
    );
    AiPlannerRequest { prompt: prompt.trim().to_string(), context }
}

fn start_plan(app: &mut PhotocraftApp, prompt: String) {
    remember_prompt(app, &prompt);
    let request = planner_request(app, &prompt);
    if let Some(service) = app.services.ai_plan.as_ref() {
        app.ai_plan_rx = Some(service(request));
        app.ai_plan_prompt = Some(prompt);
        app.ui.ai.plan.clear();
        app.ui.ai.plan_title = "Planning…".into();
        app.ui.ai.status = "Connected model is building a safe editor plan…".into();
        app.ui.ai.status_error = false;
    } else {
        apply_plan(app, prompt.clone(), plan(&prompt), "local planner");
    }
}

fn poll_model_plan(app: &mut PhotocraftApp, ctx: &egui::Context) {
    use std::sync::mpsc::TryRecvError;
    let result = app.ai_plan_rx.as_ref().and_then(|rx| match rx.try_recv() {
        Ok(result) => Some(result),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(Err("AI planner disconnected before returning a result.".into())),
    });
    if app.ai_plan_rx.is_some() && result.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_millis(80));
    }
    let Some(result) = result else { return };
    app.ai_plan_rx = None;
    let prompt = app.ai_plan_prompt.take().unwrap_or_else(|| app.ui.ai.prompt.clone());
    match result {
        Ok(planned) => match validate_model_plan(&planned) {
            Ok(()) => apply_plan(app, prompt, planned, "connected model"),
            Err(error) => {
                app.ui.ai.plan.clear();
                app.ui.ai.plan_title = "Plan rejected".into();
                app.ui.ai.status = error;
                app.ui.ai.status_error = true;
            }
        },
        Err(error) => {
            // Network/model failures fall back to the deterministic planner when it can understand
            // the request, while still surfacing the provider error for unsupported requests.
            let fallback = plan(&prompt);
            if validate(&fallback.steps).is_ok() {
                apply_plan(app, prompt, fallback, "local fallback");
            } else {
                app.ui.ai.plan.clear();
                app.ui.ai.plan_title = "Model unavailable".into();
                app.ui.ai.status = error;
                app.ui.ai.status_error = true;
            }
        }
    }
}

fn run_plan(app: &mut PhotocraftApp) {
    let steps = app.ui.ai.plan.clone();
    if let Err(reason) = validate(&steps) {
        app.ui.ai.status = reason;
        app.ui.ai.status_error = true;
        return;
    }
    let Some((doc_id, revision, history_before)) = app.session.active().map(|state| (state.doc.id.0, state.revision, state.history.past_len())) else {
        app.ui.ai.status = "Open a document to run this plan.".into();
        app.ui.ai.status_error = true;
        return;
    };
    let journal_before = app.session.journal.len();
    // Injected only after validation, so neither a model nor a saved workflow can control history
    // grouping. PhotoCraft's command dispatcher treats `coalesce` as a shell-level parameter.
    let coalesce = format!("ai-plan:{doc_id}:{revision}");
    let mut completed = 0usize;
    for step in steps {
        let Some(command) = step.command else { continue };
        let mut params = match step.params {
            Value::Null => serde_json::Map::new(),
            Value::Object(map) => map,
            _ => {
                app.ui.ai.status = format!("Plan stopped: invalid parameters for `{command}`.");
                app.ui.ai.status_error = true;
                return;
            }
        };
        params.insert("coalesce".into(), Value::String(coalesce.clone()));
        match app.run(&command, Value::Object(params)) {
            Ok(_) => completed += 1,
            Err(error) => {
                let changed = app.session.active().is_some_and(|state| state.history.past_len() > history_before);
                let rolled_back = changed && app.session.undo();
                if rolled_back {
                    if let Some(state) = app.session.active_mut() {
                        state.history.clear_redo();
                    }
                    app.sync_views();
                }
                app.session.journal.truncate(journal_before);
                app.ui.ai.status = if rolled_back {
                    format!("Plan rolled back after {completed} step(s): {error}")
                } else {
                    format!("Plan stopped before making changes: {error}")
                };
                app.ui.ai.status_error = true;
                return;
            }
        }
    }
    if let Some(state) = app.session.active_mut() {
        let title = app.ui.ai.plan_title.trim();
        state.history.set_current_label(if title.is_empty() { "AI Plan".to_string() } else { format!("AI Plan: {title}") });
    }
    app.ui.ai.status = format!("Applied {completed} step{} ? one undo", if completed == 1 { "" } else { "s" });
    app.ui.ai.status_error = false;
}

pub fn panel(app: &mut PhotocraftApp, ui: &mut egui::Ui, workflows: bool) {
    poll_model_plan(app, ui.ctx());
    if workflows {
        workflows_panel(app, ui);
    } else {
        assistant_panel(app, ui);
    }
}

fn assistant_panel(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing.y = 9.0;
    ui.add_space(2.0);

    hero(app, ui, &t);
    prompt_box(app, ui, &t);
    quick_prompts(app, ui, &t);

    let planning = app.ai_plan_rx.is_some();
    let can_plan = !app.ui.ai.prompt.trim().is_empty() && !planning;
    let submit = ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
    let mut make_plan = false;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 8.0).max(100.0);
        let primary = crate::widgets::primary_button(ui, if planning { "Planning…" } else { "Create plan" }, width * 0.62);
        if primary.clicked() && can_plan {
            make_plan = true;
        }
        let clear = crate::widgets::secondary_button(ui, "Clear", width * 0.32);
        if clear.clicked() {
            app.ui.ai.prompt.clear();
            app.ui.ai.plan.clear();
            app.ui.ai.plan_title.clear();
            app.ui.ai.status = "Ready for a request".into();
            app.ui.ai.status_error = false;
            app.ai_plan_rx = None;
            app.ai_plan_prompt = None;
        }
    });
    if submit && can_plan {
        make_plan = true;
    }
    if make_plan {
        let prompt = app.ui.ai.prompt.clone();
        start_plan(app, prompt);
    }

    if !app.ui.ai.plan.is_empty() {
        plan_card(app, ui, &t);
    } else {
        empty_state(ui, &t);
    }
}

fn hero(app: &PhotocraftApp, ui: &mut egui::Ui, t: &Tokens) {
    egui::Frame::new()
        .fill(t.accent_soft)
        .stroke(Stroke::new(1.0, t.accent_border))
        .corner_radius(CornerRadius::same(t.radius_lg as u8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 16.0, t.accent);
                crate::icons::paint(ui, r, "sparkles", 16.0, Color32::WHITE);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Creative Copilot").font(theme::semibold(14.0)).color(t.text));
                    let detail = if app.services.ai_plan.is_some() {
                        "Connected model planner · commands validated locally"
                    } else {
                        "Local command planner · no upload"
                    };
                    ui.label(RichText::new(detail).size(10.5).color(t.text_dim));
                });
            });
        });
}

fn prompt_box(app: &mut PhotocraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.label(RichText::new("WHAT SHOULD CHANGE?").size(9.5).color(t.text_faint).strong());
    egui::Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(1.0, t.field_border))
        .corner_radius(CornerRadius::same(t.radius as u8))
        .inner_margin(egui::Margin::same(9))
        .show(ui, |ui| {
            let edit = egui::TextEdit::multiline(&mut app.ui.ai.prompt)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("e.g. Make this brighter, add contrast and keep it editable…");
            ui.add(edit);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("⌌↵").font(theme::mono(9.5)).color(t.text_faint));
            });
        });
}

fn quick_prompts(app: &mut PhotocraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
        for (label, prompt) in [
            ("Polish", "Make it brighter with a little more contrast and vibrance"),
            ("B&W", "Make this black and white"),
            ("Duplicate", "Duplicate the active layer"),
        ] {
            let button = egui::Button::new(RichText::new(label).size(10.5).color(t.text_dim))
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.card_border))
                .corner_radius(CornerRadius::same(t.radius_sm as u8));
            if ui.add(button).clicked() {
                app.ui.ai.prompt = prompt.into();
            }
        }
    });
}

fn percent(value: f64) -> String {
    format!("{}%", (value * 100.0).round() as i64)
}

fn signed(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    let number = if (rounded - rounded.round()).abs() < 1e-9 { format!("{}", rounded as i64) } else { format!("{rounded:.1}") };
    if rounded > 0.0 { format!("+{number}") } else { number }
}

fn step_summary(step: &AiStep) -> String {
    let Some(command) = step.command.as_deref() else { return step.note.clone() };
    let p = &step.params;
    match command {
        "layer.new.layer" => {
            p.get("name").and_then(Value::as_str).map_or_else(|| "New editable raster layer".into(), |name| format!("New raster layer ? {name}"))
        }
        "layer.new.group" => p.get("name").and_then(Value::as_str).map_or_else(|| "New empty layer group".into(), |name| format!("New group ? {name}")),
        "layer.groupLayers" => p
            .get("name")
            .and_then(Value::as_str)
            .map_or_else(|| "Group the current layer selection".into(), |name| format!("Current selection ? group ? {name}")),
        "layer.duplicate" => "Duplicate the active layer or current layer selection".into(),
        "layer.setProps" => {
            let mut parts = Vec::new();
            if let Some(name) = p.get("name").and_then(Value::as_str) {
                parts.push(format!("Rename ? {name}"));
            }
            if let Some(visible) = p.get("visible").and_then(Value::as_bool) {
                parts.push(if visible { "Show layer".into() } else { "Hide layer".into() });
            }
            if let Some(value) = p.get("opacity").and_then(Value::as_f64) {
                parts.push(format!("Opacity {}", percent(value)));
            }
            if let Some(value) = p.get("fill").and_then(Value::as_f64) {
                parts.push(format!("Fill {}", percent(value)));
            }
            if let Some(blend) = p.get("blend").and_then(Value::as_str) {
                parts.push(format!("Blend {blend}"));
            }
            if parts.is_empty() { "Update active layer properties".into() } else { parts.join(" ? ") }
        }
        "layer.arrange.bringForward" => "Active layer ? one position forward".into(),
        "layer.arrange.sendBackward" => "Active layer ? one position backward".into(),
        "layer.arrange.bringToFront" => "Active layer ? top of its stack".into(),
        "layer.arrange.sendToBack" => "Active layer ? bottom of its stack".into(),
        "layer.createClippingMask" => "Clip active layer to the layer below".into(),
        "layer.releaseClippingMask" => "Release active layer from its clipping mask".into(),
        "layer.newAdjustmentLayer.brightnessContrast" => {
            let b = p.get("brightness").and_then(Value::as_f64).unwrap_or(0.0);
            let c = p.get("contrast").and_then(Value::as_f64).unwrap_or(0.0);
            format!("Adjustment layer ? Brightness {} ? Contrast {}", signed(b), signed(c))
        }
        "layer.newAdjustmentLayer.vibrance" => {
            let v = p.get("vibrance").and_then(Value::as_f64).unwrap_or(0.0);
            let sat = p.get("saturation").and_then(Value::as_f64).unwrap_or(0.0);
            format!("Adjustment layer ? Vibrance {} ? Saturation {}", signed(v), signed(sat))
        }
        "layer.newAdjustmentLayer.curves" => {
            let points = p.get("points").and_then(Value::as_array).map_or(0, Vec::len);
            if points == 0 { "Adjustment layer ? Curves".into() } else { format!("Adjustment layer ? Curves ? {points} control points") }
        }
        "layer.newAdjustmentLayer.blackWhite" => "Adjustment layer ? Black & White".into(),
        "layer.newAdjustmentLayer.invert" => "Adjustment layer ? Invert".into(),
        _ => step.label.clone(),
    }
}

fn plan_badge(ui: &mut egui::Ui, t: &Tokens, text: &str, emphasized: bool) {
    egui::Frame::new()
        .fill(if emphasized { t.accent_soft } else { t.field })
        .stroke(Stroke::new(1.0, if emphasized { t.accent_border } else { t.field_border }))
        .corner_radius(CornerRadius::same(t.radius_sm as u8))
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(8.5).strong().color(if emphasized { t.accent_text } else { t.text_dim }));
        });
}

fn plan_card(app: &mut PhotocraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(2.0);
    let ready = validate(&app.ui.ai.plan).is_ok();
    ui.horizontal(|ui| {
        ui.label(RichText::new(&app.ui.ai.plan_title).font(theme::semibold(12.5)).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            plan_badge(ui, t, if ready { "READY" } else { "REVIEW" }, ready);
        });
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        plan_badge(ui, t, &format!("{} STEP{}", app.ui.ai.plan.len(), if app.ui.ai.plan.len() == 1 { "" } else { "S" }), false);
        plan_badge(ui, t, "EDITABLE", false);
        plan_badge(ui, t, "ONE UNDO", true);
    });

    egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.card_border))
        .corner_radius(CornerRadius::same(t.radius as u8))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            let steps = app.ui.ai.plan.clone();
            for (i, step) in steps.iter().enumerate() {
                if i > 0 {
                    ui.add_space(3.0);
                    ui.separator();
                    ui.add_space(3.0);
                }
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), egui::Sense::hover());
                    let ready = step.command.is_some();
                    ui.painter().circle_filled(r.center(), 10.0, if ready { t.accent_soft } else { t.field });
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        (i + 1).to_string(),
                        theme::medium(10.0),
                        if ready { t.accent_text } else { t.text_faint },
                    );
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&step.label).size(11.5).color(t.text));
                        if let Some(command) = &step.command {
                            ui.label(RichText::new(step_summary(step)).size(9.8).color(t.text_dim)).on_hover_text(command);
                        } else {
                            ui.label(RichText::new(&step.note).size(9.5).color(t.warning));
                        }
                    });
                });
            }
        });

    ui.add_space(5.0);
    ui.label(RichText::new("SAVE AS WORKFLOW").size(9.0).color(t.text_faint).strong());
    ui.horizontal(|ui| {
        let save_width = 82.0;
        let name_width = (ui.available_width() - save_width - 6.0).max(90.0);
        egui::Frame::new()
            .fill(t.field)
            .stroke(Stroke::new(1.0, t.field_border))
            .corner_radius(CornerRadius::same(t.radius_sm as u8))
            .inner_margin(egui::Margin::symmetric(7, 3))
            .show(ui, |ui| {
                ui.set_width(name_width);
                ui.add(
                    egui::TextEdit::singleline(&mut app.ui.ai.workflow_name).desired_width(f32::INFINITY).frame(egui::Frame::NONE).hint_text("Workflow name"),
                );
            });
        let can_save = validate(&app.ui.ai.plan).is_ok() && workflow_name_ok(&app.ui.ai.workflow_name);
        ui.add_enabled_ui(can_save, |ui| {
            if crate::widgets::secondary_button(ui, "Save", save_width).clicked()
                && let Err(error) = save_current_workflow(app)
            {
                app.ui.ai.status = error;
                app.ui.ai.status_error = true;
            }
        });
    });

    let valid = validate(&app.ui.ai.plan);
    let can_run = valid.is_ok() && app.session.active().is_some();
    ui.add_enabled_ui(can_run, |ui| {
        let label = format!("Apply plan ? {} step{}", app.ui.ai.plan.len(), if app.ui.ai.plan.len() == 1 { "" } else { "s" });
        if crate::widgets::primary_button(ui, &label, ui.available_width()).clicked() {
            run_plan(app);
        }
    });
    if app.session.active().is_none() {
        ui.label(RichText::new("Open a document to run this plan.").size(10.0).color(t.text_faint));
    } else if let Err(reason) = valid {
        ui.label(RichText::new(reason).size(10.0).color(t.warning));
    }

    let status_color = if app.ui.ai.status_error { t.danger } else { t.text_dim };
    ui.label(RichText::new(&app.ui.ai.status).size(10.0).color(status_color));
}

fn empty_state(ui: &mut egui::Ui, t: &Tokens) {
    egui::Frame::new()
        .fill(t.card.gamma_multiply(if t.dark() { 0.92 } else { 0.98 }))
        .corner_radius(CornerRadius::same(t.radius as u8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.label(RichText::new("Editable, not flattened").font(theme::medium(11.5)).color(t.text_dim));
            ui.label(
                RichText::new("Plans map to real layers and adjustment commands, so you can keep editing after AI changes.").size(10.5).color(t.text_faint),
            );
        });
}

fn workflows_panel(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing.y = 8.0;
    ui.add_space(2.0);
    ui.label(RichText::new("WORKFLOW STARTERS").size(9.5).color(t.text_faint).strong());
    ui.label(RichText::new("Reusable recipes built from safe editor commands.").size(10.5).color(t.text_dim));

    for (icon, title, subtitle, prompt) in [
        ("sparkles", "Product polish", "Tone + restrained vibrance", "Make it brighter with a little more contrast and vibrance"),
        ("contrast", "Tonal lift", "Brightness + contrast, non-destructive", "Make this brighter and add contrast"),
        ("layers", "Layer starter", "Duplicate the active layer", "Duplicate the active layer"),
    ] {
        if workflow_card(ui, &t, icon, title, subtitle) {
            let prompt = prompt.to_string();
            apply_plan(app, prompt.clone(), plan(&prompt), "workflow");
            app.ui.ai.tab = 0;
        }
    }

    ui.add_space(7.0);
    ui.label(RichText::new("SAVED WORKFLOWS").size(9.5).color(t.text_faint).strong());
    if app.ui.ai.saved_workflows.is_empty() {
        egui::Frame::new()
            .fill(t.field)
            .stroke(Stroke::new(1.0, t.field_border))
            .corner_radius(CornerRadius::same(t.radius as u8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.label(RichText::new("No saved workflows yet").font(theme::medium(11.5)).color(t.text));
                ui.label(RichText::new("Create a safe plan in Assistant, give it a name, then save it here for reuse.").size(10.0).color(t.text_faint));
            });
    } else {
        let workflows = app.ui.ai.saved_workflows.clone();
        let mut action = None;
        for (index, workflow) in workflows.iter().enumerate() {
            egui::Frame::new()
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.card_border))
                .corner_radius(CornerRadius::same(t.radius as u8))
                .inner_margin(egui::Margin::symmetric(9, 7))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&workflow.name).font(theme::medium(11.5)).color(t.text));
                            ui.label(
                                RichText::new(format!("{} step{}", workflow.steps.len(), if workflow.steps.len() == 1 { "" } else { "s" }))
                                    .size(9.5)
                                    .color(t.text_faint),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::icons::button(ui, "trash", 24.0, false, "Remove saved workflow").clicked() {
                                action = Some((index, false));
                            }
                            if crate::widgets::secondary_button(ui, "Use", 56.0).clicked() {
                                action = Some((index, true));
                            }
                        });
                    });
                });
        }
        if let Some((index, load)) = action {
            let result = if load { load_workflow(app, index) } else { remove_saved_workflow(app, index) };
            if let Err(error) = result {
                app.ui.ai.status = error;
                app.ui.ai.status_error = true;
            }
        }
    }
}

fn workflow_card(ui: &mut egui::Ui, t: &Tokens, icon: &str, title: &str, subtitle: &str) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 54.0), egui::Sense::click());
    let fill = if response.hovered() { t.hover } else { t.card };
    ui.painter().rect_filled(rect, t.radius, fill);
    ui.painter().rect_stroke(rect, t.radius, Stroke::new(1.0, t.card_border), egui::StrokeKind::Inside);

    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 27.0, rect.center().y), vec2(28.0, 28.0));
    ui.painter().rect_filled(icon_rect, t.radius_sm, t.accent_soft);
    crate::icons::paint(ui, icon_rect, icon, 14.0, t.accent_text);
    ui.painter().text(egui::pos2(rect.left() + 49.0, rect.center().y - 7.0), egui::Align2::LEFT_CENTER, title, theme::medium(11.5), t.text);
    ui.painter().text(egui::pos2(rect.left() + 49.0, rect.center().y + 9.0), egui::Align2::LEFT_CENTER, subtitle, egui::FontId::proportional(9.5), t.text_dim);
    crate::icons::paint(
        ui,
        egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), vec2(16.0, 16.0)),
        "chevron-right",
        11.0,
        t.text_faint,
    );
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_planner_builds_safe_editable_steps() {
        let p = plan("Make it brighter with more contrast and vibrance");
        assert!(p.steps.len() >= 2);
        assert!(validate(&p.steps).is_ok());
        assert!(p.steps.iter().all(|s| s.command.as_deref().is_some_and(|c| c.starts_with("layer."))));
    }

    #[test]
    fn product_workflow_waits_for_vision_instead_of_faking_it() {
        let p = plan("Prepare this product photo for an online store");
        assert!(p.steps.iter().any(|s| s.command.is_none()));
        assert!(validate(&p.steps).is_err());
    }

    #[test]
    fn validator_rejects_commands_outside_the_allowlist() {
        let steps = vec![AiStep::command("Export it", "file.save", json!({}))];
        assert!(validate(&steps).is_err());
    }

    #[test]
    fn set_props_allows_only_reviewed_editable_fields() {
        let safe = vec![AiStep::command(
            "Style active layer",
            "layer.setProps",
            json!({"name": "Hero", "opacity": 0.72, "fill": 0.9, "blend": "Multiply", "visible": true}),
        )];
        assert!(validate(&safe).is_ok());

        for params in [json!({"locked": true}), json!({"channels": [true, false, true]}), json!({"layer": 123}), json!({"opacity": 1.5})] {
            let plan = vec![AiStep::command("Unsafe property", "layer.setProps", params)];
            assert!(validate(&plan).is_err());
        }
    }

    #[test]
    fn provider_contract_and_runtime_allowlist_stay_in_sync() {
        let contract = planner_contract();
        let Some(commands) = contract.get("allowedCommands").and_then(Value::as_array) else {
            panic!("planner contract has no allowedCommands array");
        };
        let ids: Vec<&str> = commands.iter().filter_map(|command| command.get("id").and_then(Value::as_str)).collect();
        assert_eq!(ids.len(), SAFE_COMMANDS.len());
        for command in SAFE_COMMANDS {
            assert!(ids.contains(command), "provider contract is missing {command}");
        }
    }

    #[test]
    fn commands_without_params_reject_layer_ids() {
        for command in ["layer.duplicate", "layer.arrange.bringToFront", "layer.createClippingMask"] {
            let plan = vec![AiStep::command("Target arbitrary layer", command, json!({"layer": 42}))];
            assert!(validate(&plan).is_err(), "{command}");
        }
    }

    #[test]
    fn adjustment_params_are_bounded_and_unknown_fields_are_rejected() {
        assert!(validate(&[AiStep::command("Tone", "layer.newAdjustmentLayer.brightnessContrast", json!({"brightness": 25, "contrast": 12}),)]).is_ok());
        assert!(validate(&[AiStep::command("Legacy tone", "layer.newAdjustmentLayer.brightnessContrast", json!({"legacy": true}),)]).is_err());
        assert!(validate(&[AiStep::command("Too vibrant", "layer.newAdjustmentLayer.vibrance", json!({"vibrance": 101}))]).is_err());
    }

    #[test]
    fn curves_require_ordered_bounded_points() {
        assert!(
            validate(&[AiStep::command("Curve", "layer.newAdjustmentLayer.curves", json!({"points": [[0, 0], [96, 88], [180, 194], [255, 255]]}),)]).is_ok()
        );
        for points in [json!([[0, 0]]), json!([[0, 0], [0, 5]]), json!([[0, 0], [256, 255]])] {
            assert!(validate(&[AiStep::command("Bad curve", "layer.newAdjustmentLayer.curves", json!({"points": points}))]).is_err());
        }
    }

    #[test]
    fn local_planner_knows_safe_layer_structure_actions() {
        let grouped = plan("Katmanları grupla");
        assert_eq!(grouped.steps.first().and_then(|step| step.command.as_deref()), Some("layer.groupLayers"));
        assert!(validate(&grouped.steps).is_ok());

        let hidden = plan("Hide active layer");
        assert_eq!(hidden.steps.first().and_then(|step| step.command.as_deref()), Some("layer.setProps"));
        assert_eq!(hidden.steps.first().and_then(|step| step.params.get("visible")).and_then(Value::as_bool), Some(false));
        assert!(validate(&hidden.steps).is_ok());
    }

    #[test]
    fn saved_workflow_round_trip_filters_unsafe_entries() {
        let safe = AiWorkflow {
            name: "Polish".into(),
            steps: vec![AiStep::command("Lift contrast", "layer.newAdjustmentLayer.brightnessContrast", json!({"brightness": 8, "contrast": 5}))],
        };
        let unsafe_workflow = AiWorkflow { name: "Unsafe".into(), steps: vec![AiStep::command("Save", "file.save", json!({}))] };
        let text = serialize_saved_workflows(&[safe.clone(), unsafe_workflow]).unwrap();
        assert_eq!(parse_saved_workflows(&text).unwrap(), vec![safe]);
    }

    #[test]
    fn workflow_store_is_versioned_and_does_not_persist_original_prompt() {
        let workflow = AiWorkflow { name: "Polish".into(), steps: vec![AiStep::command("Duplicate", "layer.duplicate", json!({}))] };
        let text = serialize_saved_workflows(&[workflow]).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["version"], WORKFLOW_STORE_VERSION);
        assert!(value.get("workflows").is_some());
        assert!(!text.contains("prompt"));
    }

    #[test]
    fn ui_state_does_not_serialize_saved_workflow_archive() {
        let mut state = AiPanelState::default();
        state.saved_workflows.push(AiWorkflow { name: "Private recipe".into(), steps: vec![AiStep::command("Duplicate", "layer.duplicate", json!({}))] });
        let value = serde_json::to_value(state).unwrap();
        assert!(value.get("saved_workflows").is_none());
        assert!(value.get("workflow_name").is_none());
    }

    #[test]
    fn saving_workflow_replaces_same_name_and_persists() {
        use std::sync::{Arc, Mutex};
        let written = Arc::new(Mutex::new(String::new()));
        let out = Arc::clone(&written);
        let services = crate::Services {
            save_ai_workflows: Some(Box::new(move |text| {
                *out.lock().unwrap() = text.to_string();
                Ok(())
            })),
            ..Default::default()
        };
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), services);
        app.ui.ai.workflow_name = "Polish".into();
        app.ui.ai.prompt = "brighten".into();
        app.ui.ai.plan_title = "Edit plan".into();
        app.ui.ai.plan = vec![AiStep::command("Lift", "layer.newAdjustmentLayer.brightnessContrast", json!({"brightness": 5}))];
        save_current_workflow(&mut app).unwrap();
        app.ui.ai.plan = vec![AiStep::command("Vibrance", "layer.newAdjustmentLayer.vibrance", json!({"vibrance": 10}))];
        save_current_workflow(&mut app).unwrap();
        assert_eq!(app.ui.ai.saved_workflows.len(), 1);
        assert_eq!(app.ui.ai.saved_workflows[0].steps[0].command.as_deref(), Some("layer.newAdjustmentLayer.vibrance"));
        assert!(written.lock().unwrap().contains("Polish"));
    }

    #[test]
    fn saved_workflow_survives_a_new_app_instance() {
        use std::sync::{Arc, Mutex};
        let store = Arc::new(Mutex::new(String::new()));
        let writer = Arc::clone(&store);
        let services = crate::Services {
            save_ai_workflows: Some(Box::new(move |text| {
                *writer.lock().unwrap() = text.to_string();
                Ok(())
            })),
            ..Default::default()
        };
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), services);
        app.ui.ai.workflow_name = "Reusable polish".into();
        app.ui.ai.plan = vec![AiStep::command("Duplicate", "layer.duplicate", json!({}))];
        save_current_workflow(&mut app).unwrap();

        let reader = Arc::clone(&store);
        let services = crate::Services { load_ai_workflows: Some(Box::new(move || Some(reader.lock().unwrap().clone()))), ..Default::default() };
        let restarted = PhotocraftApp::new(photocraft_engine::Session::new(), services);
        assert_eq!(restarted.ui.ai.saved_workflows.len(), 1);
        assert_eq!(restarted.ui.ai.saved_workflows[0].name, "Reusable polish");
        assert_eq!(restarted.ui.ai.saved_workflows[0].steps[0].command.as_deref(), Some("layer.duplicate"));
    }

    #[test]
    fn plans_and_saved_workflows_are_bounded() {
        let steps: Vec<_> = (0..=MAX_PLAN_STEPS).map(|index| AiStep::command(&format!("Step {index}"), "layer.duplicate", json!({}))).collect();
        assert!(validate(&steps).is_err());

        let too_long = "x".repeat(MAX_STEP_LABEL_CHARS + 1);
        assert!(validate(&[AiStep::command(&too_long, "layer.duplicate", json!({}))]).is_err());
    }

    #[test]
    fn failed_workflow_save_does_not_mutate_archive() {
        let services = crate::Services { save_ai_workflows: Some(Box::new(|_| Err("disk full".into()))), ..Default::default() };
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), services);
        app.ui.ai.workflow_name = "Polish".into();
        app.ui.ai.plan = vec![AiStep::command("Duplicate", "layer.duplicate", json!({}))];
        assert!(save_current_workflow(&mut app).is_err());
        assert!(app.ui.ai.saved_workflows.is_empty());
    }

    #[test]
    fn model_plan_is_revalidated_after_the_service_boundary() {
        let services = crate::Services {
            ai_plan: Some(Box::new(|_| {
                let (tx, rx) = std::sync::mpsc::channel();
                let _ =
                    tx.send(Ok(PlannedRequest { title: "Unsafe".into(), steps: vec![AiStep::command("Save behind the user's back", "file.save", json!({}))] }));
                rx
            })),
            ..Default::default()
        };
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), services);
        start_plan(&mut app, "do something unsafe".into());
        poll_model_plan(&mut app, &egui::Context::default());
        assert!(app.ui.ai.plan.is_empty());
        assert!(app.ui.ai.status_error);
        assert!(app.ui.ai.status.contains("allowlist"));
    }

    #[test]
    fn reviewed_layer_property_plan_executes_on_the_active_layer() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 32, "height": 32})).unwrap();
        app.run("layer.new.layer", json!({"name": "Before"})).unwrap();
        app.ui.ai.plan = vec![AiStep::command("Style layer", "layer.setProps", json!({"name": "Hero", "opacity": 0.5, "blend": "Multiply"}))];
        run_plan(&mut app);
        let state = app.session.active().expect("document");
        let layer = state.active_layer.and_then(|id| state.doc.layer(id)).expect("active layer");
        assert_eq!(layer.name, "Hero");
        assert!((layer.opacity - 0.5).abs() < 1e-6);
        assert_eq!(layer.blend, photocraft_color::BlendMode::Multiply);
        assert!(!app.ui.ai.status_error);
    }

    #[test]
    fn step_summaries_are_human_readable_and_hide_command_ids() {
        let props = AiStep::command("Style", "layer.setProps", json!({"opacity": 0.55, "blend": "Multiply", "visible": true}));
        let summary = step_summary(&props);
        assert!(summary.contains("Opacity 55%"));
        assert!(summary.contains("Blend Multiply"));
        assert!(summary.contains("Show layer"));
        assert!(!summary.contains("layer.setProps"));

        let tone = AiStep::command("Tone", "layer.newAdjustmentLayer.brightnessContrast", json!({"brightness": 12, "contrast": -6}));
        let summary = step_summary(&tone);
        assert!(summary.contains("Brightness +12"));
        assert!(summary.contains("Contrast -6"));
    }

    #[test]
    fn plan_review_surface_exposes_human_summary_and_atomic_badge() {
        use egui_kittest::{Harness, kittest::Queryable};
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.ui.panels.ai = true;
        app.ui.ai.plan_title = "Product polish".into();
        app.ui.ai.workflow_name = "Product polish".into();
        app.ui.ai.plan = vec![
            AiStep::command("Style product", "layer.setProps", json!({"opacity": 0.55, "blend": "Multiply"})),
            AiStep::command("Lift tone", "layer.newAdjustmentLayer.brightnessContrast", json!({"brightness": 12, "contrast": 6})),
        ];
        let mut h = Harness::builder().with_size(vec2(620.0, 820.0)).build_ui_state(
            |ui, app: &mut PhotocraftApp| {
                let ctx = ui.ctx().clone();
                if !ctx.fonts(|fonts| fonts.families().contains(&egui::FontFamily::Name("medium".into()))) {
                    return;
                }
                crate::panels::right_dock(app, ui);
                egui::CentralPanel::default().show(ui, |_| {});
            },
            app,
        );
        PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::Studio);
        h.state_mut().ui.theme = crate::theme::ThemeKind::Studio;
        h.run_steps(4);
        assert!(h.query_by_label("ONE UNDO").is_some());
        assert!(h.query_by_label("2 STEPS").is_some());
        assert!(h.query_by_label_contains("Opacity 55%").is_some());
        assert!(h.query_by_label_contains("Brightness +12").is_some());
        assert!(h.query_by_label("Apply plan ? 2 steps").is_some());
        assert!(h.query_by_label_contains("layer.setProps").is_none(), "technical command ids stay out of the visible review surface");
    }

    #[test]
    fn multi_step_plan_is_one_undo_step() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 32, "height": 32})).unwrap();
        let (layers_before, history_before) = {
            let state = app.session.active().unwrap();
            (state.doc.layers.len(), state.history.past_len())
        };
        app.ui.ai.plan_title = "Atomic polish".into();
        app.ui.ai.plan = vec![
            AiStep::command("Create working layer", "layer.new.layer", json!({"name": "AI Working"})),
            AiStep::command("Add monochrome look", "layer.newAdjustmentLayer.blackWhite", json!({})),
        ];
        run_plan(&mut app);
        let state = app.session.active().unwrap();
        assert_eq!(state.history.past_len(), history_before + 1);
        assert_eq!(state.doc.layers.len(), layers_before + 2);
        assert_eq!(state.history.undo_label(), Some("AI Plan: Atomic polish"));
        assert!(app.ui.ai.status.contains("one undo"));

        assert!(app.session.undo());
        let state = app.session.active().unwrap();
        assert_eq!(state.doc.layers.len(), layers_before);
        assert_eq!(state.history.past_len(), history_before);
    }

    #[test]
    fn failed_multi_step_plan_rolls_back_and_cannot_be_redone() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 32, "height": 32})).unwrap();
        let (layers_before, history_before, journal_before) = {
            let state = app.session.active().unwrap();
            (state.doc.layers.len(), state.history.past_len(), app.session.journal.len())
        };
        app.ui.ai.plan_title = "Should rollback".into();
        app.ui.ai.plan = vec![
            AiStep::command("Create working layer", "layer.new.layer", json!({"name": "Temporary"})),
            // A newly-created layer is already topmost, so this reviewed command fails at runtime.
            AiStep::command("Move it further forward", "layer.arrange.bringForward", json!({})),
        ];
        run_plan(&mut app);
        let state = app.session.active().unwrap();
        assert_eq!(state.doc.layers.len(), layers_before);
        assert_eq!(state.history.past_len(), history_before);
        assert!(!state.history.can_redo());
        assert_eq!(app.session.journal.len(), journal_before);
        assert!(app.ui.ai.status_error);
        assert!(app.ui.ai.status.contains("rolled back"));
    }

    #[test]
    fn ai_plans_cannot_supply_their_own_coalesce_key() {
        let plan = vec![AiStep::command("Try to control history", "layer.new.layer", json!({"name": "x", "coalesce": "attacker"}))];
        assert!(validate(&plan).is_err());
    }

    #[test]
    fn connected_model_plan_can_cross_the_boundary_when_safe() {
        let services = crate::Services {
            ai_plan: Some(Box::new(|_| {
                let (tx, rx) = std::sync::mpsc::channel();
                let _ = tx.send(Ok(PlannedRequest {
                    title: "Polish".into(),
                    steps: vec![AiStep::command("Lift contrast", "layer.newAdjustmentLayer.brightnessContrast", json!({"brightness": 8, "contrast": 5}))],
                }));
                rx
            })),
            ..Default::default()
        };
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), services);
        start_plan(&mut app, "polish this".into());
        poll_model_plan(&mut app, &egui::Context::default());
        assert_eq!(app.ui.ai.plan_title, "Polish");
        assert!(!app.ui.ai.status_error);
        assert!(app.ui.ai.status.contains("connected model"));
    }

    #[test]
    fn turkish_prompt_is_recognized() {
        let p = plan("Biraz daha parlak yap ve renkleri canlılaştır");
        assert!(validate(&p.steps).is_ok());
        assert!(p.steps.iter().any(|s| s.command.as_deref() == Some("layer.newAdjustmentLayer.brightnessContrast")));
    }
}
