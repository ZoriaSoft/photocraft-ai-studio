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
## Atomic plan execution

Executable AI plans run as one history transaction. The shell injects an internal coalescing key only after the plan has passed validation, so model output and saved workflows cannot control history grouping. A successful multi-step plan appears as one `AI Plan: ?` undo state. If any later step fails, AI Studio automatically undoes the already-applied steps, removes the failed plan from redo history, and restores the command journal to its pre-plan state.
