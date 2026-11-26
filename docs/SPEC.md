# promptmaxxer v1: product spec

The original design spec for v1. Decisions: no server in v1; built with Tauri. Brand: `brand/BRAND.md`.
Where the built app differs: it also runs on a Claude plan (Claude Code CLI) a ChatGPT plan (Codex CLI) or any OpenAI-compatible API (cloud or local), not only an Anthropic API key; every model name can be changed; and the default max key is Ctrl+Shift+Space.

## What it is
A small Mac menu bar app. Highlight text in any app, press one key, and the text is replaced by a strong prompt that fits the AI it is going to and the project you are in.

## 1. Core use
- Fast key: Option+Space (changeable). Max key: Ctrl+Option+Space (changeable). Pressing the fast key again while the bar is open upgrades to max.
- Works in any app with editable text. Where text cannot be replaced (terminals, some web apps), the result goes to the clipboard and the bar says so.
- Copies the selection itself and puts the person's old clipboard back afterwards.
- Pastes only into the app that was in front when the key was pressed.
- Floating bar beside the selection: target AI, context chip, the prompt streaming in, progress line. Esc cancels.
- Undo: one key in the bar (or Cmd+Z in the app) restores the original.
- Fail-safe: any error, timeout or refusal leaves the text unchanged and shows why in the bar.

## 2. Context engine (project memory)
Read in this order. Later, closer files win over earlier ones, the same as Claude Code and Codex.
1. User level: the app's own "About me" notes, `~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`. Each source has an on/off switch.
2. Project detection, from the front app:
   - Terminals (Terminal, iTerm2, Ghostty, Warp): the working folder of the shell under the front window.
   - Code editors (VS Code, Cursor, Zed, Xcode): the open folder from the window title or the app's own files.
   - Then up to the Git root to find the project top.
   - Other apps: no project; the user level only. A pinned project in the menu overrides detection.
3. Project level: `CLAUDE.md`, `CLAUDE.local.md`, `AGENTS.md`, `.claude/CLAUDE.md` from the project top down to the working folder, plus Claude's project memory index `~/.claude/projects/<project>/memory/MEMORY.md`.
- Project brief: when any of these files change (checked by fingerprint), the app writes one brief of max 300 words with the facts that change prompts (stack, commands, rules, live and risky areas). Each rewrite reads only the brief, never the full files.
- Rules for the brief: facts only, kept word for word where exact (paths, commands, names); no keys or passwords (a secret scanner removes them before anything is sent).
- The context chip shows what was used. Clicking it shows the brief and the source files.
- Per-project switch: "never read this project".

## 3. Target awareness
- Knows the receiving AI from the app: Claude Code, Codex, Cursor, ChatGPT, Claude, Gemini, Perplexity, Midjourney, image and video tools in the browser, or "unknown, decide from the text".
- A playbook per target type (coding agent, chat, research, writing, image, video, data, building a bot), as in today's framework, plus per-tool notes for the most-used tools.

## 4. Speed and quality
| | Fast key | Max key |
|---|---|---|
| Model | Claude Haiku (latest) | Claude Sonnet (latest) |
| Size | compact (2 to 25 lines) | full brief |
| Target time to finished text | p50 under 2.5 s | p50 under 8 s |
| First words in the bar | under 1 s | under 1.5 s |
- Direct Anthropic API calls (no Claude Code tool in between), streaming.
- The rulebook and the project brief are cached at Anthropic (prompt caching), so each press sends only the new text.
- Local work (copy, detect project, read cached brief) under 100 ms.
- Guard: a refusal or a comment instead of a prompt is caught by rules in code and retried once.
- Quality bar to ship: on the eval set, win rate over a plain one-line improver at least 15 of 18, hard fails at most 1 of 18, in 2 runs in a row.

## 5. Features as modules
- Each feature is a module with a manifest: name, key, input (selection, clipboard, voice), what it does, output (replace, clipboard, bar only).
- v1 modules: Maxx (fast), Maxx (max).
- Ready for later modules without changes to the core: Shorten, Explain, Translate, Voice-to-prompt (dictate), Prompt library.
- Settings list modules with an on/off switch and their key.

## 6. History and learning
- Local history: original, result, target, project, time. Search, copy, re-run as max, delete one or all, or turn history off.
- Thumbs up or down in the bar. Thumbs-down examples stay local and feed the person's own eval set.

## 7. Settings
- Keys, model per key, About me notes, context sources on or off, per-project rules, history on or off, start at login, sound on or off.

## 8. First run (4 screens)
1. Welcome and what it does (the floating bar animation).
2. Add your Anthropic key (link to get one; key checked with one test call; saved in the macOS Keychain).
3. Allow Accessibility (needed to copy and paste for you), with a live check that turns green.
4. Try it: a sample rough prompt to highlight and maxx.

## 9. Privacy and safety
- Everything stays on the Mac except the call to Anthropic. No account, no tracking, no analytics in v1.
- The key is kept in the macOS Keychain, never in a file.
- A secret scanner strips keys and passwords from the selection and the brief before sending.
- The app never sends, posts or presses Enter: it only replaces the highlighted text.

## 10. Platform and delivery
- macOS 12 or later, Apple silicon and Intel. Windows later (Tauri keeps that open).
- Signed and notarised with an Apple Developer ID ($99 a year, needed before sale). Until then: local signing for your own use.
- Auto-update off until a release channel exists.
- Size target: under 25 MB.

## Not in v1
- Accounts, payments and a hosted server (v2: monthly plan).
- Windows and Linux builds.
- Voice dictation (planned module).
- Team or shared project memory.

## Done means for v1
1. On a real Mac, Option+Space in Notes, Claude, a terminal and VS Code replaces the selection, and in Ghostty fills the clipboard.
2. Measured p50 for the fast key under 2.5 s over 20 real presses.
3. In a project with a CLAUDE.md, the context chip shows it and the prompt uses at least one fact from it; in a project with none, no project facts appear.
4. The eval quality bar in section 4 passes 2 runs in a row.
5. A fresh macOS user account completes the 4 first-run screens without help.
