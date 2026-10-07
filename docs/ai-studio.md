# AI Studio model planner

AI Studio always has a deterministic local planner. The desktop build can optionally use an OpenAI Responses API model for broader natural-language planning.

## Enable the model planner

Set the API key in the environment **before launching the app**. Never commit API keys to this repository or put them in PhotoCraft preferences.

PowerShell:

```powershell
$env:OPENAI_API_KEY = "your-key"
# Optional; defaults to gpt-5.5
$env:PHOTOCRAFT_AI_MODEL = "gpt-5.5"
```

Optional provider endpoint override:

```powershell
$env:PHOTOCRAFT_AI_ENDPOINT = "https://api.openai.com/v1/responses"
```

When `OPENAI_API_KEY` is absent, the model service is not created and AI Studio remains local-only.

## Privacy boundary

The command planner does **not** upload document pixels. It currently sends only the user's prompt plus a small document summary: whether a document is open, canvas dimensions, layer count, color mode and bit depth.

## Safety boundary

Model output is treated as untrusted data. The response must parse into a structured plan, and every executable command must be present in AI Studio's reviewed allowlist. Unknown commands are rejected before entering the plan UI. Plans containing capabilities the editor cannot execute can use a pending step with no command; pending steps cannot run.

If the model request fails and the local planner understands the request, AI Studio falls back to the local plan. Otherwise the provider error is surfaced without executing anything.


## Saved workflows

A validated plan can be saved as a reusable workflow from the Assistant tab. Desktop builds persist these recipes separately as `ai-workflows.json` in the application config directory. The file stores only the workflow name and validated editor steps; the original natural-language prompt is not persisted with the recipe.

Saved workflows are treated as untrusted input when reloaded: every recipe is passed through the same command and parameter validator again, invalid entries are discarded, and future schema versions that this build does not understand are rejected.

## Batch Workflows V1

Desktop builds can apply any validated saved workflow to a folder from the Workflows tab. V1 is intentionally conservative:

- Processing is non-recursive and each input file runs in a fresh engine session.
- The saved workflow is revalidated before the batch starts and again by the execution boundary.
- Source files are never modified. Outputs are new PNG files named `<stem>-ai.png` in a separately selected output folder.
- Existing output files are skipped rather than overwritten, and prior `*-ai.*` outputs in the source folder are ignored.
- Unsupported files are skipped. A failure in one image does not stop the rest of the folder; the UI reports written, skipped and failed counts plus the first per-file error.
- The batch runner works off the UI thread so planning and editor interaction do not need to own the temporary documents.

This first version deliberately omits recursive traversal, overwrite mode, format selection and destructive in-place processing. Those can be added later without weakening the default safety boundary.
## Plan review surface

Before execution, AI Studio presents each step in editor language rather than exposing raw command IDs as the primary UI. The review card shows the number of steps, `EDITABLE`, and `ONE UNDO`; layer property changes are summarized as values such as opacity and blend mode, while tonal adjustments show their effective settings. Raw command IDs remain available only as hover detail for diagnostics.

## Atomic plan execution

Executable AI plans run as one history transaction. The shell injects an internal coalescing key only after the plan has passed validation, so model output and saved workflows cannot control history grouping. A successful multi-step plan appears as one `AI Plan: ?` undo state. If any later step fails, AI Studio automatically undoes the already-applied steps, removes the failed plan from redo history, and restores the command journal to its pre-plan state.
