// promptmaxxer UI: the one bridge to the backend.
// In the Tauri app it uses window.__TAURI__.core.invoke and window.__TAURI__.event.listen
// (needs "withGlobalTauri": true). In a normal browser it answers from mock data,
// so every page can be opened and screenshotted. Command and event names match app/src-tauri/src/commands.rs.
(function () {
  "use strict";
  const T = window.__TAURI__;
  const LIVE = !!(T && T.core && T.core.invoke && T.event && T.event.listen);
  const q = new URLSearchParams(location.search);
  document.documentElement.classList.add(LIVE ? "tauri" : "mock");
  // Windows or macOS. First guess from the web view (WebView2 says "Windows NT"); the app's `app_platform`
  // answer below confirms it. Browser preview: ?os=windows.
  let WIN = q.has("os") ? q.get("os") === "windows" : /Windows/i.test(navigator.userAgent);

  // Engine ids and labels; the backend label wins when present.
  const ENGINES = [
    { id: "custom-api", label: "Any model", fast: "", max: "" },
    { id: "anthropic-key", label: "Anthropic key", fast: "claude-haiku-5-5", max: "claude-sonnet-5-5" },
    { id: "claude-plan", label: "Claude plan", fast: "haiku", max: "sonnet" },
    { id: "chatgpt-plan", label: "ChatGPT plan", fast: "", max: "" },
    { id: "gemini-plan", label: "Gemini", fast: "", max: "" },
  ];
  const engineLabel = (id) => (ENGINES.find((e) => e.id === id) || { label: id || "" }).label;

  // ---------- mock backend (browser only) ----------
  const mock = (() => {
    const now = Date.now();
    const onlyClaude = q.get("engines") === "claude";
    const noneReady = q.get("engines") === "none";
    let keySaved = !onlyClaude && !noneReady;
    let settings = {
      fast_key: "Alt+Space", max_key: "Ctrl+Shift+Space",
      engine: "auto", engine_fast: null, engine_max: null,
      models: {}, custom_api: { base_url: "", fast_model: "", max_model: "" },
      about_me: "I run a small web studio on a Mac. Plain English, short steps, exact commands. Never push to main without asking.",
      sources: { claude_user: true, codex_user: false, project_files: true },
      blocked_projects: ["~/Clients/bank-portal"], pinned_project: null,
      history: q.get("history") !== "off", start_at_login: true, sound: false, onboarded: false,
    };
    const DEF = { "anthropic-key": ["claude-haiku-5-5", "claude-sonnet-5-5"], "claude-plan": ["haiku", "sonnet"],
      "chatgpt-plan": ["gpt-6.2-sol", "gpt-6.2-astra"], "gemini-plan": ["flash", "pro"], "custom-api": ["", ""] };
    const withModels = (s) => {
      const d = DEF[s.id], c = settings.custom_api || {}, m = (settings.models || {})[s.id] || {};
      const fast = s.id === "custom-api" ? c.fast_model || "" : m.fast || d[0];
      const max = s.id === "custom-api" ? c.max_model || fast : m.max || d[1];
      return { ...s, fast_model: fast, max_model: max, default_fast: d[0], default_max: d[1] };
    };
    const statuses = () => {
      const c = settings.custom_api || {}, cSet = !!(c.base_url && c.fast_model);
      return [
        { id: "custom-api", label: "Any model (OpenAI-compatible API)", ready: cSet, detail: cSet ? (c.base_url.split("://")[1] || c.base_url).split("/")[0] : "Not set up" },
        { id: "anthropic-key", label: "Anthropic key", ready: keySaved, detail: keySaved ? "Key in Keychain" : "No key saved" },
        { id: "claude-plan", label: "Claude plan", ready: !noneReady, detail: noneReady ? "Not signed in" : "Signed in" },
        { id: "chatgpt-plan", label: "ChatGPT plan", ready: !onlyClaude && !noneReady, detail: onlyClaude || noneReady ? "CLI not found" : "Signed in" },
        { id: "gemini-plan", label: "Gemini", ready: !onlyClaude && !noneReady, detail: onlyClaude || noneReady ? "Not signed in" : "Signed in" },
      ].map(withModels);
    };
    const min = 60e3, hr = 36e5;
    let history = q.get("empty") ? [] : [
      { id: "h1", ts: new Date(now - 2 * min).toISOString(), app: "Ghostty", level: "standard", engine: "claude-plan", model: "haiku", ms: 1840, ok: true, draft: "fix the login bug on staging, users get kicked out", output: "Goal: find and fix why users are signed out of staging after about a minute.\nContext: shop repo. Run `pnpm test` before you say done. Do not push to main.\nSteps:\n1. Reproduce on staging with the test account and time the sign-out.\n2. Read the session code in app/auth/ and check the cookie max-age.\n3. Fix it, add a test that holds a session for 10 minutes.", target: "Claude Code", project: "shop", thumb: true },
      { id: "h2", ts: new Date(now - 18 * min).toISOString(), app: "Ghostty", level: "max", engine: "chatgpt-plan", model: "gpt-5.5", ms: 6120, ok: true, draft: "add tests for checkout discount codes", output: "Write unit tests for the discount code path in checkout. Cover expired, single use, stacked and wrong-currency codes. Use the existing vitest setup.", target: "Codex", project: "shop", thumb: null },
      { id: "h3", ts: new Date(now - 1 * hr).toISOString(), app: "Claude", level: "standard", engine: "anthropic-key", model: "claude-haiku-5-5", ms: 1530, ok: true, draft: "email to client about the delay, keep it short", output: "Write a 5-line email to a client. The launch moves one week. Say why in one line, give the new date, and offer a 15-minute call.", target: "Claude", project: null, thumb: null },
      { id: "h4", ts: new Date(now - 3 * hr).toISOString(), app: "Brave Browser", level: "standard", engine: "claude-plan", model: "haiku", ms: 2210, ok: true, draft: "moody hero image for a sauna brand", output: "Cedar sauna interior at dusk, low warm light through steam, wide shot, 16:9, quiet and premium, no people, no text.", target: "Midjourney", project: null, thumb: false },
      { id: "h5", ts: new Date(now - 26 * hr).toISOString(), app: "Cursor", level: "max", engine: "claude-plan", model: "sonnet", ms: 7480, ok: true, draft: "make the menu bar icon sharper at 16 px", output: "The menu bar icon blurs at 16 and 18 px. Draw a simpler statue for small sizes: fewer curls, a thicker profile line, pixel-aligned at 1x and 2x.", target: "Cursor", project: "promptmaxxer", thumb: null },
      { id: "h6", ts: new Date(now - 29 * hr).toISOString(), app: "Brave Browser", level: "standard", engine: "gemini-plan", model: "gemini-flash", ms: 2950, ok: false, draft: "summarise what prompt caching costs", output: "", target: "Perplexity", project: null, thumb: null },
    ];
    let ax = q.get("ax") === "granted";
    const wait = (ms) => new Promise((r) => setTimeout(r, ms));
    const handlers = {
      get_settings: () => JSON.parse(JSON.stringify(settings)),
      save_settings: ({ settings: s }) => { settings = JSON.parse(JSON.stringify(s)); return null; },
      list_engines: () => statuses(),
      test_engine: async ({ engine }) => {
        await wait(700);
        const st = statuses().find((e) => e.id === engine);
        if (!st || !st.ready) return { ok: false, ms: 0, model: "", error: st ? st.detail : "Unknown engine" };
        return { ok: true, ms: { "custom-api": 900, "anthropic-key": 610, "claude-plan": 1840, "chatgpt-plan": 2390, "gemini-plan": 2710 }[engine], model: st.fast_model, text: "Ready." };
      },
      save_api_key: async ({ provider, key }) => {
        await wait(400);
        if (provider === "custom-api") return { ok: true };
        if (!/^sk-ant-/.test(key || "")) return { ok: false, error: "That doesn't look like an Anthropic key. It starts with sk-ant-." };
        keySaved = true; return { ok: true };
      },
      delete_api_key: () => { keySaved = false; return null; },
      history_list: ({ query }) => {
        const t = (query || "").toLowerCase().trim();
        return history.filter((h) => !t || [h.draft, h.output, h.target, h.project, h.app].join(" ").toLowerCase().includes(t));
      },
      history_delete: ({ id }) => { history = id == null ? [] : history.filter((h) => h.id !== id); return null; },
      history_copy: () => null,
      history_rerun: () => null,
      history_thumb: ({ id, up }) => { const h = history.find((x) => x.id === id); if (h) h.thumb = up; return null; },
      app_platform: () => (WIN ? "windows" : "macos"),
      check_accessibility: () => ax,
      open_accessibility_settings: () => { setTimeout(() => { ax = true; }, 2500); return null; },
      finish_onboarding: () => { settings.onboarded = true; return null; },
      bar_cancel: () => null, bar_undo: () => null, bar_thumb: () => null,
      open_settings: () => null, open_history: () => null,
      context_preview: () => ({
        chip: "shop · CLAUDE.md + 2 memories",
        brief: "Next.js 15 shop on Vercel, pnpm. Tests: pnpm test (vitest), pnpm e2e (Playwright). Never push to main; open a PR from a branch. Auth lives in app/auth/ (cookie sessions). Staging: staging.shop.example. Payments through Stripe test mode only.",
        sources: [
          { path: "~/.claude/CLAUDE.md", label: "CLAUDE.md", level: "User" },
          { path: "~/Code/shop/CLAUDE.md", label: "CLAUDE.md", level: "Project" },
          { path: "~/.claude/projects/-Users-me-Code-shop/memory/MEMORY.md", label: "memory", level: "Project" },
        ],
      }),
    };
    const listeners = {};
    return {
      async invoke(cmd, args) {
        const h = handlers[cmd];
        if (!h) throw new Error("mock: unknown command " + cmd);
        console.debug("[mock invoke]", cmd, args || {});
        return h(args || {});
      },
      listen(evt, cb) { (listeners[evt] = listeners[evt] || []).push(cb); return () => { listeners[evt] = listeners[evt].filter((f) => f !== cb); }; },
      emit(evt, payload) { (listeners[evt] || []).forEach((cb) => cb(payload)); },
    };
  })();

  // ---------- public API ----------
  async function invoke(cmd, args) {
    return LIVE ? T.core.invoke(cmd, args || {}) : mock.invoke(cmd, args);
  }
  async function listen(evt, cb) {
    if (LIVE) return T.event.listen(evt, (e) => cb(e.payload));
    return mock.listen(evt, cb);
  }

  // ---------- small helpers shared by pages ----------
  const $ = (s, el) => (el || document).querySelector(s);
  const $$ = (s, el) => Array.from((el || document).querySelectorAll(s));
  const esc = (s) => String(s == null ? "" : s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  // "Ctrl+Alt+Space" -> ["⌃","⌥","Space"]; on Windows -> ["Ctrl","Alt","Space"]
  const KEYMAP = { ctrl: "⌃", control: "⌃", alt: "⌥", option: "⌥", shift: "⇧", cmd: "⌘", command: "⌘", super: "⌘", commandorcontrol: "⌘" };
  const KEYMAP_WIN = { ctrl: "Ctrl", control: "Ctrl", alt: "Alt", option: "Alt", shift: "Shift", cmd: "Win", command: "Win", super: "Win", commandorcontrol: "Ctrl" };
  const keyParts = (combo) => String(combo || "").split("+").filter(Boolean).map((p) => (WIN ? KEYMAP_WIN : KEYMAP)[p.toLowerCase()] || p);
  const keyCaps = (combo, cls) => keyParts(combo).map((k) => `<span class="kc ${cls || "sm"}">${esc(k)}</span>`).join("");
  // keydown -> "Ctrl+Alt+Space" (Tauri global-shortcut format), or null while only modifiers are down
  function comboFromEvent(e) {
    const code = e.code || "";
    if (/^(Shift|Control|Alt|Meta)(Left|Right)$/.test(code)) return null;
    let key = code.replace(/^Key/, "").replace(/^Digit/, "");
    if (!key) return null;
    const mods = [];
    if (e.ctrlKey) mods.push("Ctrl");
    if (e.altKey) mods.push("Alt");
    if (e.shiftKey) mods.push("Shift");
    if (e.metaKey) mods.push("Cmd");
    if (!mods.length) return null; // a global key needs a modifier
    return mods.concat(key).join("+");
  }

  function ago(ts) {
    const d = (Date.now() - new Date(ts).getTime()) / 1000;
    if (d < 60) return "Just now";
    if (d < 3600) return Math.round(d / 60) + " min ago";
    if (d < 86400) return Math.round(d / 3600) + " hr ago";
    if (d < 172800) return "Yesterday";
    return new Date(ts).toLocaleDateString(undefined, { day: "numeric", month: "short" });
  }

  let toastTimer;
  function toast(text, dot) {
    let t = $(".toast");
    if (!t) { t = document.createElement("div"); t.className = "toast"; document.body.appendChild(t); }
    t.innerHTML = (dot ? `<span class="dot ${dot}"></span>` : "") + esc(text);
    t.classList.add("show");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => t.classList.remove("show"), 2200);
  }

  const ICON = {
    search: '<svg viewBox="0 0 16 16"><circle cx="7" cy="7" r="5"/><path d="M11 11l3.5 3.5"/></svg>',
    up: '<svg viewBox="0 0 16 16"><path d="M5 7.5v6.5H2.5V7.5zM5 7.5l2.8-5.5c1.1 0 1.8.8 1.6 1.9L9 6.3h4c.9 0 1.6.9 1.3 1.8l-1.1 4.5c-.2.8-.9 1.4-1.7 1.4H5"/></svg>',
    down: '<svg viewBox="0 0 16 16" style="transform:rotate(180deg)"><path d="M5 7.5v6.5H2.5V7.5zM5 7.5l2.8-5.5c1.1 0 1.8.8 1.6 1.9L9 6.3h4c.9 0 1.6.9 1.3 1.8l-1.1 4.5c-.2.8-.9 1.4-1.7 1.4H5"/></svg>',
    copy: '<svg viewBox="0 0 16 16"><rect x="5" y="5" width="8" height="9" rx="2"/><path d="M3 11V4a2 2 0 012-2h5"/></svg>',
    rerun: '<svg viewBox="0 0 16 16"><path d="M13 8a5 5 0 11-1.5-3.6M13 2.5v2.8h-2.8"/></svg>',
    trash: '<svg viewBox="0 0 16 16"><path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 9h5.8l.6-9"/></svg>',
    x: '<svg viewBox="0 0 16 16"><path d="M4 4l8 8M12 4l-8 8"/></svg>',
  };

  // "macos" or "windows", from the app (mock: the guess above).
  async function platform() {
    try { WIN = (await invoke("app_platform")) === "windows"; } catch (e) {}
    return WIN ? "windows" : "macos";
  }
  // Where API keys are kept, for messages.
  const keyStore = () => (WIN ? "Windows Credential Manager" : "the macOS Keychain");
  // Windows: the page's Mac words become PC words (text and placeholders), once at load.
  function windowsWords(root) {
    const swap = (s) => s.replace(/the macOS Keychain/g, "Windows Credential Manager").replace(/\bthis Mac\b/g, "this PC")
      .replace(/\byour Mac\b/g, "your PC").replace(/\bon a Mac\b/g, "on a PC").replace(/\bthe Mac\b/g, "the PC");
    const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let n = w.nextNode(); n; n = w.nextNode()) { const t = swap(n.nodeValue); if (t !== n.nodeValue) n.nodeValue = t; }
    root.querySelectorAll("[placeholder]").forEach((el) => { el.placeholder = swap(el.placeholder); });
  }
  const platformReady = platform().then((p) => {
    if (p === "windows") {
      document.documentElement.classList.add("win");
      if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", () => windowsWords(document.body));
      else windowsWords(document.body);
    }
    return p;
  });

  window.PMX = { LIVE, query: q, invoke, listen, platform: () => platformReady, keyStore, mockEmit: mock.emit, ENGINES, engineLabel, $, $$, esc, keyParts, keyCaps, comboFromEvent, ago, toast, ICON };
})();
