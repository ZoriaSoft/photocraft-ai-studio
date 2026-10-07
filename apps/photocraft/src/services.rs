//! Native platform services: file dialogs (rfd), filesystem, codecs.

use photocraft_codecs::{ChannelLayout, EncodeOptions, Image, SampleType as CS};
use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Document, Layer, Size};
use photocraft_format::Autosaver;
use photocraft_geom::Rect;
use photocraft_ui_egui::{AiPlanFn, Services};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

/// Everything File › Open reads: PhotoCraft and Photoshop documents, flat images, and Photoshop
/// brushes (.abr) and gradients (.grd), which go to the preset libraries.
const OPEN_EXTS: &[&str] = &[
    "pcraft", "psd", "psb", "png", "jpg", "jpeg", "tif", "tiff", "webp", "gif", "bmp", "tga", "ico", "qoi", "exr", "hdr", "pbm", "pgm", "ppm", "pam", "pfm",
    "dng", "cr2", "cr3", "nef", "nrw", "arw", "pef", "orf", "rw2", "raf", "abr", "grd",
];

/// File › Save As formats: (filter name, extensions). The filter matching the suggested name's
/// extension comes first, so a .pcraft document saves as .pcraft by default and everything else
/// keeps defaulting to Photoshop.
const SAVE_FILTERS: &[(&str, &[&str])] = &[
    ("Photoshop", &["psd", "psb"]),
    ("PhotoCraft", &["pcraft"]),
    ("PNG", &["png"]),
    ("JPEG", &["jpg"]),
    ("TIFF", &["tif"]),
    ("Targa", &["tga"]),
    ("OpenEXR", &["exr"]),
];

/// [`SAVE_FILTERS`] with the one for `suggested`'s extension first.
fn save_filters(suggested: &str) -> Vec<(&'static str, &'static [&'static str])> {
    let ext = Path::new(suggested).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut v = SAVE_FILTERS.to_vec();
    if let Some(i) = v.iter().position(|(_, exts)| exts.contains(&ext.as_str())) {
        let f = v.remove(i);
        v.insert(0, f);
    }
    v
}

/// Per-user settings directory: `PHOTOCRAFT_CONFIG_DIR`, else `<exe dir>/PhotoCraftData` in
/// portable mode, else the platform convention. Everything the app persists lives under it; see
/// [`crate::app_dirs`].
pub fn config_dir() -> Option<PathBuf> {
    crate::app_dirs::config_dir()
}

pub fn prefs_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("preferences.json"))
}

fn ai_workflows_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("ai-workflows.json"))
}

/// The brush preset store (one file per preset group plus tip bitmaps; see
/// `photocraft_engine::preset_store`).
pub fn presets_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("Presets"))
}

fn recovery_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("Recovery"))
}

/// Write `bytes` crash-safely (temp file beside the target, fsync, rename, directory fsync; see
/// [`photocraft_format::atomic`]). Every document write (Save, Save As, Export, Save for Web) and
/// the preferences go through here, so a failed or interrupted save never destroys the old file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    photocraft_format::atomic_write(path, bytes).map_err(|e| e.to_string())
}

/// Model-backed planning is opt-in: no key means the UI stays entirely local. Secrets are read
/// from the process environment and are never copied into PhotoCraft preferences or UI state.
fn ai_planner() -> Option<AiPlanFn> {
    let api_key = std::env::var("OPENAI_API_KEY").ok().filter(|key| !key.trim().is_empty())?;
    let model = std::env::var("PHOTOCRAFT_AI_MODEL").unwrap_or_else(|_| "gpt-5.5".into());
    let endpoint = std::env::var("PHOTOCRAFT_AI_ENDPOINT").unwrap_or_else(|_| "https://api.openai.com/v1/responses".into());
    let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(45)).build().ok()?;
    Some(Box::new(move |request| {
        let (tx, rx) = std::sync::mpsc::channel();
        let (client, api_key, model, endpoint) = (client.clone(), api_key.clone(), model.clone(), endpoint.clone());
        std::thread::spawn(move || {
            let result = openai_plan(&client, &endpoint, &api_key, &model, request);
            let _ = tx.send(result);
        });
        rx
    }))
}

fn openai_plan(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    api_key: &str,
    model: &str,
    request: photocraft_ui_egui::ai_panel::AiPlannerRequest,
) -> Result<photocraft_ui_egui::ai_panel::PlannedRequest, String> {
    let contract = photocraft_ui_egui::ai_panel::planner_contract();
    let instructions = format!(
        "You are the planning layer of an editable image editor. Convert the user's intent into a short JSON editor plan. \
Only use the command contract below. Return JSON only; no markdown or prose outside the JSON. \
A null command is allowed only to honestly represent a capability the editor cannot execute yet.\n\nCONTRACT:\n{}",
        serde_json::to_string_pretty(&contract).map_err(|error| error.to_string())?
    );
    let input = format!(
        "User request:\n{}\n\nDocument context (metadata only, no pixels):\n{}",
        request.prompt,
        serde_json::to_string_pretty(&request.context).map_err(|error| error.to_string())?
    );
    let body = serde_json::json!({
        "model": model,
        "instructions": instructions,
        "input": input,
        "store": false
    });
    let response = client.post(endpoint).bearer_auth(api_key).json(&body).send().map_err(|error| format!("AI request failed: {error}"))?;
    let status = response.status();
    let value: serde_json::Value = response.json().map_err(|error| format!("AI response was not valid JSON: {error}"))?;
    if !status.is_success() {
        let message = value.pointer("/error/message").and_then(serde_json::Value::as_str).unwrap_or("model provider returned an error");
        return Err(format!("AI provider error ({status}): {message}"));
    }
    let text = response_output_text(&value).ok_or_else(|| "AI response did not contain output text.".to_string())?;
    let plan = parse_plan_json(text)?;
    photocraft_ui_egui::ai_panel::validate_model_plan(&plan)?;
    Ok(plan)
}

fn response_output_text(value: &serde_json::Value) -> Option<&str> {
    value.get("output")?.as_array()?.iter().find_map(|item| {
        item.get("content")?
            .as_array()?
            .iter()
            .find_map(|content| (content.get("type")?.as_str()? == "output_text").then(|| content.get("text")?.as_str()).flatten())
    })
}

fn parse_plan_json(text: &str) -> Result<photocraft_ui_egui::ai_panel::PlannedRequest, String> {
    let trimmed = text.trim();
    if let Ok(plan) = serde_json::from_str(trimmed) {
        return Ok(plan);
    }
    // Be tolerant of a provider wrapping otherwise-valid JSON in markdown despite the instruction.
    let start = trimmed.find('{').ok_or_else(|| "AI plan did not contain a JSON object.".to_string())?;
    let end = trimmed.rfind('}').ok_or_else(|| "AI plan JSON was incomplete.".to_string())?;
    serde_json::from_str(&trimmed[start..=end]).map_err(|error| format!("AI plan JSON was invalid: {error}"))
}

pub fn native(automation: Option<photocraft_automation::AuthorizedWorkspace>) -> Services {
    let savers: Rc<RefCell<HashMap<u64, Autosaver>>> = Rc::default();
    let savers2 = savers.clone();
    let clip: Rc<RefCell<Option<arboard::Clipboard>>> = Rc::default();
    let automation_read = automation.clone().map(|workspace| {
        Box::new(move |path: &str| {
            let bytes = workspace.read(path).map_err(|error| error.to_string())?;
            let name = Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or(path).to_string();
            Ok((name, bytes))
        }) as photocraft_ui_egui::AutomationReadFn
    });
    let automation_write = automation.clone().map(|workspace| {
        Box::new(move |path: &str, bytes: &[u8]| workspace.write(path, bytes).map_err(|error| error.to_string())) as photocraft_ui_egui::AutomationWriteFn
    });
    let automation_command = automation.map(|_| {
        Box::new(|id: &str, params: &serde_json::Value| {
            photocraft_automation::workspace::authorize_desktop_engine_command(id, params).map_err(|error| error.to_string())
        }) as photocraft_ui_egui::AutomationCommandFn
    });
    Services {
        import: Some(Box::new(|name: &str, bytes: &[u8]| {
            crate::crash_guard::guard("Open", || photocraft_io::import(name, bytes).map(|r| (r.document, r.warnings)).map_err(|e| e.to_string()))
        })),
        export: Some(Box::new(|doc: &Document, path: &str, settings: &photocraft_ui_egui::ExportSettings| {
            let mut opts = photocraft_io::ExportOptions::default();
            if let Some(q) = settings.jpeg_quality {
                opts.encode.jpeg_quality = q;
            }
            crate::crash_guard::guard("Export", || photocraft_io::export(doc, path, &opts).map(|r| (r.bytes, r.warnings)).map_err(|e| e.to_string()))
        })),
        pick_open: Some(Box::new(|| {
            let path = rfd::FileDialog::new().add_filter("All Formats", OPEN_EXTS).add_filter("PhotoCraft", &["pcraft"]).pick_file()?;
            let bytes = std::fs::read(&path).ok()?;
            Some((path.to_string_lossy().to_string(), bytes))
        })),
        pick_save: Some(Box::new(|suggested: &str| {
            let p = std::path::Path::new(suggested);
            let mut d = rfd::FileDialog::new();
            for (name, exts) in save_filters(suggested) {
                d = d.add_filter(name, exts);
            }
            if let Some(name) = p.file_name() {
                d = d.set_file_name(name.to_string_lossy());
            }
            Some(d.save_file()?.to_string_lossy().to_string())
        })),
        write: Some(Box::new(|path: &str, bytes: &[u8]| write_atomic(Path::new(path), bytes))),
        automation_read,
        automation_write,
        automation_command,
        encode_png: Some(Box::new(|w, h, rgba| {
            let img = Image::from_u8(w, h, ChannelLayout::Rgba, rgba.to_vec()).map_err(|e| e.to_string())?;
            photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &EncodeOptions::default()).map_err(|e| e.to_string())
        })),
        inbox: None,
        open_url: Some(Box::new(|url: &str| open::that(url).map_err(|e| e.to_string()))),
        clipboard_set_image: Some({
            let clip = clip.clone();
            Box::new(move |w: u32, h: u32, px: &[u8]| {
                let mut slot = clip.try_borrow_mut().map_err(|_| "clipboard is busy".to_string())?;
                let cb = match slot.as_mut() {
                    Some(c) => c,
                    None => slot.insert(arboard::Clipboard::new().map_err(|e| e.to_string())?),
                };
                cb.set_image(arboard::ImageData { width: w as usize, height: h as usize, bytes: std::borrow::Cow::Borrowed(px) }).map_err(|e| e.to_string())
            })
        }),
        clipboard_get_image: Some({
            let clip = clip.clone();
            Box::new(move || {
                let mut slot = clip.try_borrow_mut().ok()?;
                let cb = match slot.as_mut() {
                    Some(c) => c,
                    None => slot.insert(arboard::Clipboard::new().ok()?),
                };
                let img = cb.get_image().ok()?;
                Some((img.width as u32, img.height as u32, img.bytes.into_owned()))
            })
        }),
        load_prefs: Some(Box::new(|| std::fs::read_to_string(prefs_file()?).ok())),
        save_prefs: Some(Box::new(|text: &str| write_atomic(&prefs_file().ok_or("no config directory")?, text.as_bytes()))),
        // Crash recovery: background incremental .pcraft saves into the recovery directory.
        autosave: Some(Box::new(move |doc: &Arc<Document>, revision: u64, path: Option<&str>| {
            let dir = recovery_dir().ok_or("no config directory")?;
            let mut map = savers.borrow_mut();
            let saver = map.entry(doc.id.0).or_insert_with(|| Autosaver::new(&dir, &format!("doc-{}", doc.id.0)));
            saver.request(doc.clone(), revision, path.map(str::to_string), Default::default());
            Ok(())
        })),
        discard_autosave: Some(Box::new(move |id: u64| {
            if let Some(s) = savers2.borrow_mut().remove(&id) {
                let _ = s.discard();
            }
        })),
        recover: Some(Box::new(|| {
            let Some(dir) = recovery_dir() else { return Vec::new() };
            let mut out = Vec::new();
            for entry in photocraft_format::list_recovery(&dir) {
                if let Ok(doc) = photocraft_format::recover(&entry) {
                    out.push((entry.info.original_path.clone(), doc));
                }
                // Recovered documents autosave again under their new ids.
                let _ = photocraft_format::discard_recovery(&dir, &entry);
            }
            out
        })),
        append_text: Some(Box::new(|path: &str, text: &str| {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| e.to_string())?;
            f.write_all(text.as_bytes()).map_err(|e| e.to_string())
        })),
        load_ai_workflows: Some(Box::new(|| std::fs::read_to_string(ai_workflows_file()?).ok())),
        save_ai_workflows: Some(Box::new(|text: &str| write_atomic(&ai_workflows_file().ok_or("no config directory")?, text.as_bytes()))),
        // Set by main once the Apple-event handlers are connected (macOS).
        os_events: None,
        ai_plan: ai_planner(),
        // Set by main, which starts loading the store before the window opens.
        preset_store: None,
    }
}

/// Flat-image import via photocraft-codecs (kept for reference/tests; the app uses photocraft-io).
#[allow(dead_code)]
pub fn import_flat(name: &str, bytes: &[u8]) -> Result<Document, String> {
    let img = photocraft_codecs::decode(bytes).map_err(|e| e.to_string())?;
    let (w, h) = (img.width(), img.height());
    let depth = match img.sample_type() {
        CS::U8 => SampleType::U8,
        CS::U16 => SampleType::U16,
        _ => SampleType::F32,
    };
    let gray = matches!(img.layout(), ChannelLayout::Gray | ChannelLayout::GrayA);
    let cmyk = matches!(img.layout(), ChannelLayout::Cmyk | ChannelLayout::CmykA);
    let mode = if gray {
        ColorMode::Grayscale
    } else if cmyk {
        ColorMode::Cmyk
    } else {
        ColorMode::Rgb
    };
    let target = match mode {
        ColorMode::Grayscale => ChannelLayout::GrayA,
        ColorMode::Cmyk => ChannelLayout::CmykA,
        _ => ChannelLayout::Rgba,
    };
    let sample = match depth {
        SampleType::U8 => CS::U8,
        SampleType::U16 => CS::U16,
        SampleType::F32 => CS::F32,
    };
    let conv = img.convert(target, sample);
    let stem = std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(name.to_string());
    let mut doc = Document::new(stem, Size::new(w, h), mode, depth);
    doc.icc_profile = img.icc.clone().map(std::sync::Arc::new);
    if let Some((x, _)) = img.meta.dpi {
        doc.resolution_dpi = x;
    }
    let mut layer = Layer::raster("Background", doc.pixel_format());
    let data = conv.to_normalized();
    layer.surface_mut().ok_or("new raster layer has no pixels")?.write_region(Rect::from_xywh(0, 0, w, h), &data);
    doc.layers.push(layer);
    Ok(doc)
}

#[allow(dead_code)]
pub fn export_flat(doc: &Document, path: &str) -> Result<Vec<u8>, String> {
    let format = photocraft_codecs::from_extension(path).ok_or_else(|| format!("unknown file type for {path}"))?;
    let buf = photocraft_compose::flatten(doc);
    let (w, h) = (buf.rect.width(), buf.rect.height());
    let data: Vec<f32> = buf.px.iter().flat_map(|p| *p).collect();
    let img = match doc.depth {
        SampleType::U8 => Image::from_u8(w, h, ChannelLayout::Rgba, buf.to_rgba8().pixels),
        SampleType::U16 => Image::from_u16(w, h, ChannelLayout::Rgba, &data.iter().map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).collect::<Vec<_>>()),
        SampleType::F32 => Image::from_f32(w, h, ChannelLayout::Rgba, &data),
    }
    .map_err(|e| e.to_string())?;
    let img = match &doc.icc_profile {
        Some(icc) if doc.mode == ColorMode::Rgb => img.with_icc(Some((**icc).clone())),
        _ => img,
    };
    photocraft_codecs::encode(&img, format, &EncodeOptions::default()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod ai_tests {
    use super::*;

    #[test]
    fn parses_model_plan_from_response_text() {
        let plan = parse_plan_json(
            r#"```json
{"title":"Polish","steps":[{"label":"Lift tone","command":"layer.newAdjustmentLayer.brightnessContrast","params":{"brightness":8,"contrast":4},"note":""}]}
```"#,
        )
        .unwrap();
        assert_eq!(plan.title, "Polish");
        assert!(photocraft_ui_egui::ai_panel::validate_model_plan(&plan).is_ok());
    }

    #[test]
    fn extracts_output_text_from_responses_payload() {
        let value = serde_json::json!({"output":[{"type":"message","content":[{"type":"output_text","text":"{\"title\":\"x\",\"steps\":[]}"}]}]});
        assert_eq!(response_output_text(&value), Some(r#"{"title":"x","steps":[]}"#));
    }
}
