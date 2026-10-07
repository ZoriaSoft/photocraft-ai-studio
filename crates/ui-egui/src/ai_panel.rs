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
    "layer.duplicate",
    "layer.newAdjustmentLayer.brightnessContrast",
    "layer.newAdjustmentLayer.curves",
    "layer.newAdjustmentLayer.vibrance",
    "layer.newAdjustmentLayer.blackWhite",
    "layer.newAdjustmentLayer.invert",
];

/// Validate the trust boundary between a planner and the editor engine.
fn validate_steps(steps: &[AiStep], require_executable: bool) -> Result<(), String> {
    if steps.is_empty() {
        return Err("The plan has no steps.".into());
    }
    for step in steps {
        let Some(command) = step.command.as_deref() else {
            if require_executable {
                return Err(format!("{} needs an AI capability that is not connected yet.", step.label));
            }
            if step.note.trim().is_empty() {
                return Err(format!("{} is pending but has no explanation.", step.label));
            }
            continue;
        };
        if !SAFE_COMMANDS.contains(&command) {
            return Err(format!("Command `{command}` is not in the AI safety allowlist."));
        }
        if !step.params.is_object() && !step.params.is_null() {
            return Err(format!("Parameters for `{command}` must be a JSON object."));
        }
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
            {"id": "layer.new.layer", "params": {"name": "optional string"}},
            {"id": "layer.new.group", "params": {"name": "optional string"}},
            {"id": "layer.duplicate", "params": {}},
            {"id": "layer.newAdjustmentLayer.brightnessContrast", "params": {"brightness": "number", "contrast": "number"}},
            {"id": "layer.newAdjustmentLayer.curves", "params": {"points": "optional array of [input, output] points"}},
            {"id": "layer.newAdjustmentLayer.vibrance", "params": {"vibrance": "number", "saturation": "number"}},
            {"id": "layer.newAdjustmentLayer.blackWhite", "params": {}},
            {"id": "layer.newAdjustmentLayer.invert", "params": {}}
        ],
        "rules": [
            "Return JSON only, with exactly title and steps at the top level.",
            "Never invent a command id. Use null when the request needs a capability outside the allowed commands.",
            "Prefer non-destructive adjustment layers and keep the document editable.",
            "Do not claim a vision, selection, export, filesystem, or network action happened when no allowed command can perform it.",
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
    if has(&["new group", "group layers", "yeni grup", "grup oluştur", "grup olustur"]) {
        steps.push(AiStep::command("Create a layer group", "layer.new.group", json!({"name": "AI Group"})));
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
    let mut completed = 0usize;
    for step in steps {
        let Some(command) = step.command else { continue };
        match app.run(&command, step.params) {
            Ok(_) => completed += 1,
            Err(error) => {
                app.ui.ai.status = format!("Stopped after {completed} step(s): {error}");
                app.ui.ai.status_error = true;
                return;
            }
        }
    }
    app.ui.ai.status = format!("Applied {completed} step{} · each action is undoable", if completed == 1 { "" } else { "s" });
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

fn plan_card(app: &mut PhotocraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(&app.ui.ai.plan_title).font(theme::semibold(12.5)).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ready = validate(&app.ui.ai.plan).is_ok();
            ui.label(RichText::new(if ready { "READY" } else { "REVIEW" }).size(9.0).color(if ready { t.accent_text } else { t.warning }));
        });
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
                            ui.label(RichText::new(command).font(theme::mono(8.8)).color(t.text_faint));
                        } else {
                            ui.label(RichText::new(&step.note).size(9.5).color(t.warning));
                        }
                    });
                });
            }
        });

    let valid = validate(&app.ui.ai.plan);
    let can_run = valid.is_ok() && app.session.active().is_some();
    ui.add_enabled_ui(can_run, |ui| {
        if crate::widgets::primary_button(ui, "Run plan", ui.available_width()).clicked() {
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

    ui.add_space(4.0);
    egui::Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(1.0, t.field_border))
        .corner_radius(CornerRadius::same(t.radius as u8))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.label(RichText::new("Saved workflows").font(theme::medium(11.5)).color(t.text));
            ui.label(RichText::new("Saving custom multi-step recipes is the next milestone.").size(10.0).color(t.text_faint));
        });
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
