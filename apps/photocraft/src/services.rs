//! Native platform services: file dialogs (rfd), filesystem, codecs.

use photocraft_codecs::{ChannelLayout, EncodeOptions, Image, SampleType as CS};
use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Document, Layer, Size};
use photocraft_format::RecoveryStore;
use photocraft_geom::Rect;
use photocraft_ui_egui::{AiPlanFn, Recovered, Services};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

/// Everything File › Open reads: PhotoCraft and Photoshop documents, flat images, and Photoshop
/// brushes (.abr) and gradients (.grd), which go to the preset libraries.
const OPEN_EXTS: &[&str] = &[
    "pcraft", "psd", "psb", "png", "jpg", "jpeg", "tif", "tiff", "webp", "gif", "bmp", "tga", "ico", "qoi", "exr", "hdr", "pbm", "pgm", "ppm", "pam", "pfm",
    "dng", "cr2", "cr3", "nef", "nrw", "arw", "pef", "orf", "rw2", "raf", "abr", "grd",
];

/// Folder-batch inputs. Preset files are intentionally excluded: V1 processes image/document files
/// non-recursively and always writes new PNG copies.
const BATCH_INPUT_EXTS: &[&str] = &[
    "pcraft", "psd", "psb", "png", "jpg", "jpeg", "tif", "tiff", "webp", "gif", "bmp", "tga", "ico", "qoi", "exr", "hdr", "pbm", "pgm", "ppm", "pam", "pfm",
    "dng", "cr2", "cr3", "nef", "nrw", "arw", "pef", "orf", "rw2", "raf",
];
const MAX_BATCH_FAILURES: usize = 20;

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

fn is_batch_input(path: &Path) -> bool {
    let ext = path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase);
    ext.as_deref().is_some_and(|ext| BATCH_INPUT_EXTS.contains(&ext))
}

fn batch_output_path(input: &Path, output_dir: &Path) -> PathBuf {
    let stem = input.file_stem().and_then(|stem| stem.to_str()).filter(|stem| !stem.is_empty()).unwrap_or("image");
    output_dir.join(format!("{stem}-ai.png"))
}

fn is_generated_batch_output(path: &Path) -> bool {
    path.file_stem().and_then(|stem| stem.to_str()).is_some_and(|stem| stem.to_ascii_lowercase().ends_with("-ai"))
}

fn process_batch_file(path: &Path, output_dir: &Path, workflow: &photocraft_ui_egui::ai_panel::AiWorkflow) -> Result<(), String> {
    let name = path.file_name().and_then(|name| name.to_str()).ok_or_else(|| "input file name is not valid UTF-8".to_string())?;
    let bytes = std::fs::read(path).map_err(|error| format!("read failed: {error}"))?;
    let imported = photocraft_io::import(name, &bytes).map_err(|error| format!("import failed: {error}"))?;

    let mut session = photocraft_engine::Session::new();
    session.add_document(imported.document, None);
    photocraft_ui_egui::ai_panel::execute_batch_steps(&mut session, &workflow.steps).map_err(|error| format!("workflow failed: {error}"))?;
    let doc = session.active().map(|state| state.doc.clone()).ok_or_else(|| "workflow produced no active document".to_string())?;

    let output = batch_output_path(path, output_dir);
    if output.exists() {
        return Err("output already exists".into());
    }
    let output_name = output.to_string_lossy();
    let exported =
        photocraft_io::export(doc.as_ref(), &output_name, &photocraft_io::ExportOptions::default()).map_err(|error| format!("export failed: {error}"))?;
    write_atomic(&output, &exported.bytes).map_err(|error| format!("write failed: {error}"))
}

fn run_ai_batch(request: photocraft_ui_egui::ai_panel::AiBatchRequest) -> Result<photocraft_ui_egui::ai_panel::AiBatchResult, String> {
    photocraft_ui_egui::ai_panel::validate(&request.workflow.steps)?;
    let input_dir = PathBuf::from(&request.input_dir);
    let output_dir = PathBuf::from(&request.output_dir);
    if !input_dir.is_dir() {
        return Err("Batch source is not a readable directory.".into());
    }
    if !output_dir.is_dir() {
        return Err("Batch output is not a writable directory.".into());
    }

    let mut files = Vec::new();
    for entry in std::fs::read_dir(&input_dir).map_err(|error| format!("Couldn't read batch source folder: {error}"))? {
        let entry = entry.map_err(|error| format!("Couldn't inspect batch source folder: {error}"))?;
        if entry.file_type().map_err(|error| format!("Couldn't inspect batch input: {error}"))?.is_file() {
            files.push(entry.path());
        }
    }
    files.sort();

    let mut result = photocraft_ui_egui::ai_panel::AiBatchResult { discovered: files.len(), ..Default::default() };
    for path in files {
        if !is_batch_input(&path) || is_generated_batch_output(&path) {
            result.skipped += 1;
            continue;
        }
        let output = batch_output_path(&path, &output_dir);
        if output.exists() {
            result.skipped += 1;
            continue;
        }
        match process_batch_file(&path, &output_dir, &request.workflow) {
            Ok(()) => result.succeeded += 1,
            Err(error) => {
                result.failed += 1;
                if result.failures.len() < MAX_BATCH_FAILURES {
                    let name = path.file_name().map(|name| name.to_string_lossy()).unwrap_or_else(|| path.as_os_str().to_string_lossy());
                    result.failures.push(format!("{name}: {error}"));
                }
            }
        }
    }
    Ok(result)
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

type SharedRecovery = Rc<RefCell<Option<RecoveryStore>>>;

/// Run `f` on the recovery store (`Err` without a config directory). The service closures never
/// call each other, so the store is never borrowed twice.
fn with_store<R>(store: &SharedRecovery, f: impl FnOnce(&mut RecoveryStore) -> R) -> Result<R, String> {
    let mut slot = store.try_borrow_mut().map_err(|_| "crash recovery is busy".to_string())?;
    Ok(f(slot.as_mut().ok_or("no config directory")?))
}

/// Crash recovery: background incremental .pcraft autosaves into `dir` (`None`: no config
/// directory, so autosaves fail and nothing is recovered). Recovered documents keep their entries
/// until a newer autosave replaces them or they're saved or closed (see [`RecoveryStore`]).
fn recovery_services(dir: Option<PathBuf>) -> Services {
    let store: SharedRecovery = Rc::new(RefCell::new(dir.map(RecoveryStore::new)));
    let (s1, s2, s3) = (store.clone(), store.clone(), store.clone());
    Services {
        autosave: Some(Box::new(move |doc: &Arc<Document>, revision: u64, path: Option<&str>| {
            with_store(&s1, |s| s.autosave(doc, revision, path.map(str::to_string)))
        })),
        discard_autosave: Some(Box::new(move |id: u64| {
            let _ = with_store(&s2, |s| s.discard(id));
        })),
        recover: Some(Box::new(move || {
            let found = with_store(&s3, |s| s.recover()).unwrap_or_default();
            found.into_iter().map(|(e, doc)| Recovered { key: e.info.key, path: e.info.original_path, doc }).collect()
        })),
        adopt_autosave: Some(Box::new(move |id: u64, key: &str| {
            let _ = with_store(&store, |s| s.adopt(id, key));
        })),
        ..Default::default()
    }
}

/// The pixels to paste when the clipboard holds copied files rather than an image (Copy in
/// Files, Finder or Explorer puts paths on the clipboard, #338): the first file that decodes,
/// upright, as RGBA8. Files that aren't images are skipped after reading only their header.
fn image_from_files(paths: &[PathBuf]) -> Option<(u32, u32, Vec<u8>)> {
    use std::io::Read;
    paths.iter().find_map(|path| {
        // text/uri-list lines end in CRLF (RFC 2483, and GTK writes them so), but arboard splits
        // on LF only, so a path copied in GNOME Files arrives with a trailing '\r'.
        let path = path.to_str().map_or_else(|| path.clone(), |s| PathBuf::from(s.trim_end_matches('\r')));
        let mut head = Vec::with_capacity(256);
        std::fs::File::open(&path).ok()?.take(256).read_to_end(&mut head).ok()?;
        // TGA has no magic number and is only guessed from the header, so trust the extension
        // too before reading what may be a large non-image file.
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match photocraft_codecs::detect(&head)? {
            photocraft_codecs::Format::Tga if photocraft_codecs::from_extension(ext) != Some(photocraft_codecs::Format::Tga) => return None,
            _ => {}
        }
        let img = photocraft_codecs::decode(&std::fs::read(&path).ok()?).ok()?;
        Some((img.width(), img.height(), img.to_rgba8()))
    })
}

pub fn native(automation: Option<photocraft_automation::AuthorizedWorkspace>) -> Services {
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
            // A read failure goes back to the app, which reports it like any other open failure.
            let bytes = photocraft_format::read_file(&path).map_err(|e| e.to_string());
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
        pick_folder: Some(Box::new(|title: &str| rfd::FileDialog::new().set_title(title).pick_folder().map(|path| path.to_string_lossy().to_string()))),
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
                // Copied files first: Finder also puts the file's icon on the clipboard as an
                // image, which would otherwise paste instead of the file.
                if let Some(img) = cb.get().file_list().ok().and_then(|paths| image_from_files(&paths)) {
                    return Some(img);
                }
                let img = cb.get_image().ok()?;
                Some((img.width as u32, img.height as u32, img.bytes.into_owned()))
            })
        }),
        load_prefs: Some(Box::new(|| std::fs::read_to_string(prefs_file()?).ok())),
        save_prefs: Some(Box::new(|text: &str| write_atomic(&prefs_file().ok_or("no config directory")?, text.as_bytes()))),
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
        ai_batch: Some(Box::new(|request| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(run_ai_batch(request));
            });
            rx
        })),
        // Set by main, which starts loading the store before the window opens.
        preset_store: None,
        is_wayland: false,
        ..recovery_services(recovery_dir())
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

    #[test]
    fn batch_output_names_are_safe_and_generated_outputs_are_skipped() {
        let out = batch_output_path(Path::new("hero.photo.JPG"), Path::new("out"));
        assert_eq!(out, Path::new("out").join("hero.photo-ai.png"));
        assert!(is_batch_input(Path::new("input.PSD")));
        assert!(!is_batch_input(Path::new("notes.txt")));
        assert!(is_generated_batch_output(Path::new("hero-ai.png")));
    }

    #[test]
    fn folder_batch_writes_a_new_png_without_mutating_the_source() {
        use photocraft_ui_egui::ai_panel::{AiBatchRequest, AiStep, AiWorkflow};
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("photocraft-ai-batch-{}-{unique}", std::process::id()));
        let input = root.join("input");
        let output = root.join("output");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::create_dir_all(&output).unwrap();

        let mut doc = Document::new("source", Size::new(8, 8), ColorMode::Rgb, SampleType::U8);
        doc.layers.push(Layer::raster("Background", doc.pixel_format()));
        let source = input.join("source.png");
        let source_bytes = export_flat(&doc, source.to_str().unwrap()).unwrap();
        std::fs::write(&source, &source_bytes).unwrap();

        let workflow = AiWorkflow {
            name: "Invert".into(),
            steps: vec![AiStep {
                label: "Invert".into(),
                command: Some("layer.newAdjustmentLayer.invert".into()),
                params: serde_json::json!({}),
                note: String::new(),
            }],
        };
        let result =
            run_ai_batch(AiBatchRequest { workflow, input_dir: input.to_string_lossy().to_string(), output_dir: output.to_string_lossy().to_string() })
                .unwrap();

        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert!(output.join("source-ai.png").is_file());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_engine::Session;
    use photocraft_format::list_recovery;
    use photocraft_ui_egui::{PhotocraftApp, prefs_ui};
    use serde_json::json;

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

    /// One app launch with crash recovery in `dir` (dropping it is the crash: background writes
    /// already queued finish, nothing else runs).
    fn launch(dir: &Path) -> PhotocraftApp {
        PhotocraftApp::new(Session::new(), recovery_services(Some(dir.to_path_buf())))
    }

    /// The first pixel of each open document, and whether it's unsaved.
    fn open_docs(app: &PhotocraftApp) -> Vec<([f32; 4], bool)> {
        app.session.documents().iter().map(|d| (photocraft_compose::flatten(&d.doc).px.first().copied().unwrap_or_default(), d.is_dirty())).collect()
    }

    fn index_of(app: &PhotocraftApp, color: [f32; 4]) -> usize {
        open_docs(app).iter().position(|(c, _)| *c == color).unwrap()
    }

    fn new_doc(app: &mut PhotocraftApp, color: &str) {
        app.run("file.new", json!({"width": 4, "height": 4})).unwrap();
        app.run("edit.fill", json!({"color": color})).unwrap();
    }

    fn autosave(app: &mut PhotocraftApp, ctx: &egui::Context) {
        prefs_ui::autosave_now(app);
        prefs_ui::tick(app, ctx);
    }

    #[test]
    fn recovered_documents_survive_a_second_crash_until_saved_or_closed() {
        let dir = std::env::temp_dir().join(format!("photocraft-recovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        // Launch 1: two unsaved documents are autosaved, then the app crashes.
        {
            let mut app = launch(&dir);
            new_doc(&mut app, "#ff0000");
            new_doc(&mut app, "#0000ff");
            autosave(&mut app, &ctx);
        }
        assert_eq!(list_recovery(&dir).len(), 2);
        // Launch 2 recovers both and crashes again before its first autosave (#444).
        {
            let mut app = launch(&dir);
            let mut docs = open_docs(&app);
            docs.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
            assert_eq!(docs, [(BLUE, true), (RED, true)], "recovered and still unsaved");
            prefs_ui::tick(&mut app, &ctx);
        }
        assert_eq!(list_recovery(&dir).len(), 2, "recovering keeps the entries on disk");
        // Launch 3 has both again; autosaving unchanged documents adds nothing, a new document
        // (whose id may repeat one from an earlier launch) gets an entry of its own.
        {
            let mut app = launch(&dir);
            assert_eq!(app.session.documents().len(), 2);
            autosave(&mut app, &ctx);
            new_doc(&mut app, "#00ff00");
            autosave(&mut app, &ctx);
        }
        assert_eq!(list_recovery(&dir).len(), 3);
        // Launch 4: closing and saving recovered documents removes their entries, no duplicates.
        let mut app = launch(&dir);
        assert_eq!(app.session.documents().len(), 3);
        let red = index_of(&app, RED);
        app.run("file.close", json!({"document": red})).unwrap();
        // Saving marks the revision saved (the file write itself is the save service's job).
        assert!(app.session.set_active(index_of(&app, BLUE)));
        let st = app.session.active_mut().unwrap();
        st.saved_revision = st.revision;
        prefs_ui::tick(&mut app, &ctx);
        let left = list_recovery(&dir);
        assert_eq!(left.len(), 1);
        assert_eq!(photocraft_compose::flatten(&photocraft_format::recover(&left[0]).unwrap()).px.first().copied(), Some(GREEN));
        let green = index_of(&app, GREEN);
        app.run("file.close", json!({"document": green})).unwrap();
        prefs_ui::tick(&mut app, &ctx);
        assert!(list_recovery(&dir).is_empty());
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("photocraft-services-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn red_png(w: u32, h: u32) -> Vec<u8> {
        let img = Image::from_u8(w, h, ChannelLayout::Rgba, [255u8, 0, 0, 255].repeat((w * h) as usize)).unwrap();
        photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &EncodeOptions::default()).unwrap()
    }

    /// A 1×1 uncompressed 32-bit TGA (TGA has no magic number).
    fn tga_1x1() -> Vec<u8> {
        let mut b = vec![0u8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 32, 8];
        b.extend_from_slice(&[0, 0, 255, 255]); // BGRA red
        b
    }

    #[test]
    fn copied_image_file_pastes_its_pixels() {
        let dir = temp("png");
        let png = dir.join("red.png");
        std::fs::write(&png, red_png(3, 2)).unwrap();
        let (w, h, px) = image_from_files(&[png]).unwrap();
        assert_eq!((w, h), (3, 2));
        assert_eq!(px, [255u8, 0, 0, 255].repeat(6));
    }

    #[test]
    fn crlf_from_a_uri_list_is_ignored() {
        // arboard splits text/uri-list on LF, so GNOME Files' CRLF list leaves a '\r' behind.
        let dir = temp("crlf");
        let png = dir.join("red image.png");
        std::fs::write(&png, red_png(2, 2)).unwrap();
        let with_cr = PathBuf::from(format!("{}\r", png.display()));
        assert_eq!(image_from_files(&[with_cr]).map(|(w, h, _)| (w, h)), Some((2, 2)));
    }

    #[test]
    fn non_images_are_skipped_for_the_first_image() {
        let dir = temp("mixed");
        let (txt, png) = (dir.join("notes.txt"), dir.join("red.png"));
        std::fs::write(&txt, b"not an image").unwrap();
        std::fs::write(&png, red_png(4, 1)).unwrap();
        let missing = dir.join("gone.png");
        assert_eq!(image_from_files(&[missing.clone(), txt.clone(), dir.clone(), png]).map(|(w, h, _)| (w, h)), Some((4, 1)));
        assert!(image_from_files(&[missing, txt, dir]).is_none());
        assert!(image_from_files(&[]).is_none());
    }

    #[test]
    fn a_damaged_image_is_skipped_not_a_panic() {
        let dir = temp("damaged");
        let bad = dir.join("cut.png");
        std::fs::write(&bad, &red_png(8, 8)[..20]).unwrap();
        assert!(image_from_files(&[bad]).is_none());
    }

    #[test]
    fn tga_is_trusted_only_with_its_extension() {
        let dir = temp("tga");
        let (tga, bin) = (dir.join("red.tga"), dir.join("red.bin"));
        std::fs::write(&tga, tga_1x1()).unwrap();
        std::fs::write(&bin, tga_1x1()).unwrap();
        assert_eq!(image_from_files(&[tga]), Some((1, 1, vec![255, 0, 0, 255])));
        assert!(image_from_files(&[bin]).is_none());
    }
}
