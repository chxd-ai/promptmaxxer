//! Map the front app (and, for browsers and terminals, its window title) to the receiving AI.
//! Order: bundle id first, then app name, then window title.
//! On Windows the "bundle id" is the program file name in lower case (`windowsterminal.exe`).

use crate::{AppInfo, Target, TargetKind};

/// What sort of app this is, for target mapping and project detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppClass {
    /// Terminal or agent host: the project is the shell's working folder.
    Terminal,
    /// Code editor. The `&str` is the app folder name under `~/Library/Application Support`
    /// for VS Code style editors ("Code", "Cursor"), or "" for others.
    Editor(&'static str),
    Browser,
    Other,
}

struct Known {
    bundle: &'static str,
    name: &'static str,
    class: AppClass,
    label: &'static str,
}

const KNOWN: &[Known] = &[
    // Terminals and agent hosts.
    Known { bundle: "com.apple.terminal", name: "terminal", class: AppClass::Terminal, label: "Terminal" },
    Known { bundle: "com.googlecode.iterm2", name: "iterm2", class: AppClass::Terminal, label: "iTerm2" },
    Known { bundle: "com.mitchellh.ghostty", name: "ghostty", class: AppClass::Terminal, label: "Ghostty" },
    Known { bundle: "dev.warp.warp-stable", name: "warp", class: AppClass::Terminal, label: "Warp" },
    Known { bundle: "dev.warp.warp", name: "warp", class: AppClass::Terminal, label: "Warp" },
    Known { bundle: "com.github.wez.wezterm", name: "wezterm", class: AppClass::Terminal, label: "WezTerm" },
    Known { bundle: "org.alacritty", name: "alacritty", class: AppClass::Terminal, label: "Alacritty" },
    Known { bundle: "io.alacritty", name: "alacritty", class: AppClass::Terminal, label: "Alacritty" },
    Known { bundle: "net.kovidgoyal.kitty", name: "kitty", class: AppClass::Terminal, label: "kitty" },
    Known { bundle: "com.stablyai.orca", name: "orca", class: AppClass::Terminal, label: "Orca" },
    Known { bundle: "co.zeit.hyper", name: "hyper", class: AppClass::Terminal, label: "Hyper" },
    Known { bundle: "org.tabby", name: "tabby", class: AppClass::Terminal, label: "Tabby" },
    // Code editors.
    Known { bundle: "com.microsoft.vscode", name: "code", class: AppClass::Editor("Code"), label: "VS Code" },
    Known { bundle: "com.microsoft.vscodeinsiders", name: "code - insiders", class: AppClass::Editor("Code - Insiders"), label: "VS Code" },
    Known { bundle: "com.vscodium", name: "vscodium", class: AppClass::Editor("VSCodium"), label: "VSCodium" },
    Known { bundle: "com.todesktop.230313mzl4w4u92", name: "cursor", class: AppClass::Editor("Cursor"), label: "Cursor" },
    Known { bundle: "com.exafunction.windsurf", name: "windsurf", class: AppClass::Editor("Windsurf"), label: "Windsurf" },
    Known { bundle: "dev.zed.zed", name: "zed", class: AppClass::Editor(""), label: "Zed" },
    Known { bundle: "dev.zed.zed-preview", name: "zed preview", class: AppClass::Editor(""), label: "Zed" },
    Known { bundle: "com.apple.dt.xcode", name: "xcode", class: AppClass::Editor(""), label: "Xcode" },
    // Browsers.
    Known { bundle: "com.apple.safari", name: "safari", class: AppClass::Browser, label: "Safari" },
    Known { bundle: "com.apple.safaritechnologypreview", name: "safari technology preview", class: AppClass::Browser, label: "Safari" },
    Known { bundle: "com.google.chrome", name: "google chrome", class: AppClass::Browser, label: "Chrome" },
    Known { bundle: "com.google.chrome.canary", name: "google chrome canary", class: AppClass::Browser, label: "Chrome" },
    Known { bundle: "com.brave.browser", name: "brave browser", class: AppClass::Browser, label: "Brave" },
    Known { bundle: "company.thebrowser.browser", name: "arc", class: AppClass::Browser, label: "Arc" },
    Known { bundle: "company.thebrowser.dia", name: "dia", class: AppClass::Browser, label: "Dia" },
    Known { bundle: "org.mozilla.firefox", name: "firefox", class: AppClass::Browser, label: "Firefox" },
    Known { bundle: "com.microsoft.edgemac", name: "microsoft edge", class: AppClass::Browser, label: "Edge" },
    Known { bundle: "com.operasoftware.opera", name: "opera", class: AppClass::Browser, label: "Opera" },
    Known { bundle: "com.vivaldi.vivaldi", name: "vivaldi", class: AppClass::Browser, label: "Vivaldi" },
    Known { bundle: "app.zen-browser.zen", name: "zen", class: AppClass::Browser, label: "Zen" },
    Known { bundle: "ai.perplexity.comet", name: "comet", class: AppClass::Browser, label: "Comet" },
    Known { bundle: "com.openai.atlas", name: "chatgpt atlas", class: AppClass::Browser, label: "ChatGPT Atlas" },
    // Windows, by program file name. Names are left empty: Windows matches on the file name only.
    Known { bundle: "windowsterminal.exe", name: "", class: AppClass::Terminal, label: "Windows Terminal" },
    Known { bundle: "powershell.exe", name: "", class: AppClass::Terminal, label: "PowerShell" },
    Known { bundle: "pwsh.exe", name: "", class: AppClass::Terminal, label: "PowerShell" },
    Known { bundle: "cmd.exe", name: "", class: AppClass::Terminal, label: "Command Prompt" },
    Known { bundle: "conhost.exe", name: "", class: AppClass::Terminal, label: "Console" },
    Known { bundle: "openconsole.exe", name: "", class: AppClass::Terminal, label: "Console" },
    Known { bundle: "mintty.exe", name: "", class: AppClass::Terminal, label: "Git Bash" },
    Known { bundle: "wezterm-gui.exe", name: "", class: AppClass::Terminal, label: "WezTerm" },
    Known { bundle: "alacritty.exe", name: "", class: AppClass::Terminal, label: "Alacritty" },
    Known { bundle: "tabby.exe", name: "", class: AppClass::Terminal, label: "Tabby" },
    Known { bundle: "hyper.exe", name: "", class: AppClass::Terminal, label: "Hyper" },
    Known { bundle: "warp.exe", name: "", class: AppClass::Terminal, label: "Warp" },
    Known { bundle: "code.exe", name: "", class: AppClass::Editor("Code"), label: "VS Code" },
    Known { bundle: "code - insiders.exe", name: "", class: AppClass::Editor("Code - Insiders"), label: "VS Code" },
    Known { bundle: "vscodium.exe", name: "", class: AppClass::Editor("VSCodium"), label: "VSCodium" },
    Known { bundle: "cursor.exe", name: "", class: AppClass::Editor("Cursor"), label: "Cursor" },
    Known { bundle: "windsurf.exe", name: "", class: AppClass::Editor("Windsurf"), label: "Windsurf" },
    Known { bundle: "zed.exe", name: "", class: AppClass::Editor(""), label: "Zed" },
    Known { bundle: "chrome.exe", name: "", class: AppClass::Browser, label: "Chrome" },
    Known { bundle: "msedge.exe", name: "", class: AppClass::Browser, label: "Edge" },
    Known { bundle: "firefox.exe", name: "", class: AppClass::Browser, label: "Firefox" },
    Known { bundle: "brave.exe", name: "", class: AppClass::Browser, label: "Brave" },
    Known { bundle: "opera.exe", name: "", class: AppClass::Browser, label: "Opera" },
    Known { bundle: "vivaldi.exe", name: "", class: AppClass::Browser, label: "Vivaldi" },
    Known { bundle: "arc.exe", name: "", class: AppClass::Browser, label: "Arc" },
    Known { bundle: "zen.exe", name: "", class: AppClass::Browser, label: "Zen" },
    Known { bundle: "comet.exe", name: "", class: AppClass::Browser, label: "Comet" },
];

/// Windows JetBrains IDEs (`idea64.exe`, `pycharm64.exe`, ...) and Android Studio (`studio64.exe`).
const JETBRAINS_EXE: &[&str] = &["idea", "pycharm", "webstorm", "phpstorm", "rider", "goland", "clion", "rubymine", "rustrover", "datagrip", "studio"];

fn known(app: &AppInfo) -> Option<&'static Known> {
    let bid = app.bundle_id.to_lowercase();
    if !bid.is_empty() {
        if let Some(k) = KNOWN.iter().find(|k| k.bundle == bid) {
            return Some(k);
        }
        // JetBrains IDEs: com.jetbrains.intellij, com.jetbrains.pycharm, ...
        let jetbrains_exe = bid.strip_suffix(".exe").map(|s| s.trim_end_matches("64")).is_some_and(|s| JETBRAINS_EXE.contains(&s));
        if bid.starts_with("com.jetbrains.") || bid.starts_with("com.google.android.studio") || jetbrains_exe {
            return Some(&Known { bundle: "", name: "", class: AppClass::Editor(""), label: "" });
        }
    }
    let name = app.name.to_lowercase();
    if name.is_empty() {
        return None;
    }
    KNOWN.iter().find(|k| k.name == name)
}

pub(crate) fn app_class(app: &AppInfo) -> AppClass {
    known(app).map(|k| k.class).unwrap_or(AppClass::Other)
}

fn t(kind: TargetKind, label: &str) -> Target {
    Target { kind, label: label.to_string() }
}

/// Map an app and its window title to the receiving AI.
pub fn target_for(app: &AppInfo, window_title: Option<&str>) -> Target {
    let title = window_title.unwrap_or("").trim();
    if let Some(k) = known(app) {
        let label = if k.label.is_empty() { app.name.as_str() } else { k.label };
        match k.class {
            AppClass::Terminal => {
                return t(TargetKind::CodingAgent, terminal_agent(title).unwrap_or(label));
            }
            AppClass::Editor(_) => return t(TargetKind::CodingAgent, label),
            AppClass::Browser => {
                return match title_target(title) {
                    Some(tg) => tg,
                    None => t(TargetKind::Unknown, "Unknown"),
                };
            }
            AppClass::Other => {}
        }
    }
    // Native AI apps, by bundle id.
    let bid = app.bundle_id.to_lowercase();
    let name = app.name.to_lowercase();
    match bid.as_str() {
        "com.anthropic.claudefordesktop" | "com.anthropic.claude" => {
            if contains_word(title, "Claude Code") {
                return t(TargetKind::CodingAgent, "Claude Code");
            }
            return t(TargetKind::Chat, "Claude");
        }
        "com.openai.chat" | "com.openai.codex" => {
            // Some ChatGPT.app builds ship with the bundle id com.openai.codex (ChatGPT with Codex inside).
            if name == "codex" || contains_word(title, "Codex") {
                return t(TargetKind::CodingAgent, "Codex");
            }
            return t(TargetKind::Chat, "ChatGPT");
        }
        "ai.perplexity.mac" | "ai.perplexity.app" => return t(TargetKind::Research, "Perplexity"),
        "com.google.geminimacos" | "com.google.gemini" => return t(TargetKind::Chat, "Gemini"),
        "com.microsoft.copilot-mac" | "com.microsoft.copilot" => return t(TargetKind::Chat, "Copilot"),
        "com.liuliu.draw-things" => return t(TargetKind::Image, "Draw Things"),
        "com.divamgupta.diffusionbee" => return t(TargetKind::Image, "DiffusionBee"),
        "com.microsoft.excel" | "excel.exe" => return t(TargetKind::Data, "Excel"),
        // Windows desktop apps, by program file name.
        "claude.exe" => {
            if contains_word(title, "Claude Code") {
                return t(TargetKind::CodingAgent, "Claude Code");
            }
            return t(TargetKind::Chat, "Claude");
        }
        "chatgpt.exe" => return t(TargetKind::Chat, "ChatGPT"),
        "perplexity.exe" => return t(TargetKind::Research, "Perplexity"),
        "com.apple.iwork.numbers" => return t(TargetKind::Data, "Numbers"),
        _ => {}
    }
    // By app name, for builds with other bundle ids.
    match name.as_str() {
        "claude" => return t(TargetKind::Chat, "Claude"),
        "chatgpt" => return t(TargetKind::Chat, "ChatGPT"),
        "codex" => return t(TargetKind::CodingAgent, "Codex"),
        "perplexity" => return t(TargetKind::Research, "Perplexity"),
        "gemini" => return t(TargetKind::Chat, "Gemini"),
        "microsoft excel" => return t(TargetKind::Data, "Excel"),
        _ => {}
    }
    t(TargetKind::Unknown, "Unknown")
}

/// A terminal title that names the agent running in it.
fn terminal_agent(title: &str) -> Option<&'static str> {
    if title.is_empty() {
        return None;
    }
    let low = title.to_lowercase();
    // Claude Code sets the terminal title with a leading "✳" while it runs.
    if low.contains("claude code") || title.trim_start().starts_with('\u{2733}') {
        return Some("Claude Code");
    }
    if word_in(&low, "codex") {
        return Some("Codex");
    }
    if word_in(&low, "claude") {
        return Some("Claude Code");
    }
    if word_in(&low, "gemini") {
        return Some("Gemini CLI");
    }
    if word_in(&low, "aider") {
        return Some("Aider");
    }
    None
}

/// Browser tab titles. Each rule: words to find (case-sensitive, whole words), kind, label.
const TITLE_RULES: &[(&[&str], TargetKind, &str)] = &[
    (&["Claude Code"], TargetKind::CodingAgent, "Claude Code"),
    (&["Codex"], TargetKind::CodingAgent, "Codex"),
    (&["ChatGPT"], TargetKind::Chat, "ChatGPT"),
    (&["Claude"], TargetKind::Chat, "Claude"),
    (&["Google AI Studio"], TargetKind::Chat, "Google AI Studio"),
    (&["Gemini"], TargetKind::Chat, "Gemini"),
    (&["NotebookLM"], TargetKind::Research, "NotebookLM"),
    (&["Perplexity"], TargetKind::Research, "Perplexity"),
    (&["Grok"], TargetKind::Chat, "Grok"),
    (&["DeepSeek"], TargetKind::Chat, "DeepSeek"),
    (&["Le Chat", "Mistral AI"], TargetKind::Chat, "Le Chat"),
    (&["Copilot Studio"], TargetKind::BotBuilding, "Copilot Studio"),
    (&["Copilot"], TargetKind::Chat, "Copilot"),
    (&["Midjourney"], TargetKind::Image, "Midjourney"),
    (&["Ideogram"], TargetKind::Image, "Ideogram"),
    (&["Leonardo.Ai", "Leonardo AI"], TargetKind::Image, "Leonardo"),
    (&["Krea"], TargetKind::Image, "Krea"),
    (&["Adobe Firefly", "Firefly"], TargetKind::Image, "Firefly"),
    (&["Recraft"], TargetKind::Image, "Recraft"),
    (&["DALL·E", "DALL-E"], TargetKind::Image, "DALL·E"),
    (&["Higgsfield"], TargetKind::Video, "Higgsfield"),
    (&["Runway"], TargetKind::Video, "Runway"),
    (&["Sora"], TargetKind::Video, "Sora"),
    (&["Kling", "KLING"], TargetKind::Video, "Kling"),
    (&["Pika"], TargetKind::Video, "Pika"),
    (&["Dream Machine", "Luma AI"], TargetKind::Video, "Luma"),
    (&["Hailuo"], TargetKind::Video, "Hailuo"),
    (&["HeyGen"], TargetKind::Video, "HeyGen"),
    (&["Synthesia"], TargetKind::Video, "Synthesia"),
    (&["Lovable"], TargetKind::CodingAgent, "Lovable"),
    (&["v0"], TargetKind::CodingAgent, "v0"),
    (&["bolt.new", "Bolt"], TargetKind::CodingAgent, "Bolt"),
    (&["Replit"], TargetKind::CodingAgent, "Replit"),
    (&["Base44"], TargetKind::CodingAgent, "Base44"),
    (&["Voiceflow"], TargetKind::BotBuilding, "Voiceflow"),
    (&["Botpress"], TargetKind::BotBuilding, "Botpress"),
    (&["GPT Builder", "Agent Builder"], TargetKind::BotBuilding, "GPT Builder"),
    (&["n8n"], TargetKind::BotBuilding, "n8n"),
    (&["Relevance AI"], TargetKind::BotBuilding, "Relevance AI"),
    (&["Flowise"], TargetKind::BotBuilding, "Flowise"),
    (&["Google Sheets"], TargetKind::Data, "Google Sheets"),
    (&["Airtable"], TargetKind::Data, "Airtable"),
    (&["Google Docs"], TargetKind::Writing, "Google Docs"),
    (&["Jasper"], TargetKind::Writing, "Jasper"),
    (&["Copy.ai"], TargetKind::Writing, "Copy.ai"),
    (&["Grammarly"], TargetKind::Writing, "Grammarly"),
];

/// Match a browser title. The site name is usually the last part ("My chat - Claude"),
/// so the parts are checked from last to first.
fn title_target(title: &str) -> Option<Target> {
    if title.is_empty() {
        return None;
    }
    let parts = split_title(title);
    for part in parts.iter().rev() {
        for (words, kind, label) in TITLE_RULES {
            if words.iter().any(|w| contains_word(part, w)) {
                return Some(t(*kind, label));
            }
        }
    }
    None
}

pub(crate) fn split_title(title: &str) -> Vec<&str> {
    let mut parts = vec![title];
    for sep in [" \u{2014} ", " \u{2013} ", " - ", " | ", " \u{00b7} "] {
        parts = parts.into_iter().flat_map(|p| p.split(sep)).collect();
    }
    parts.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect()
}

/// Case-sensitive whole-word find: the characters around the match must not be letters or digits.
pub(crate) fn contains_word(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut start = 0;
    while let Some(i) = hay[start..].find(needle) {
        let a = start + i;
        let b = a + needle.len();
        let before_ok = hay[..a].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after_ok = hay[b..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        start = a + needle.len().max(1);
        while start < hay.len() && !hay.is_char_boundary(start) {
            start += 1;
        }
        if start >= hay.len() {
            break;
        }
    }
    false
}

fn word_in(low: &str, needle: &str) -> bool {
    contains_word(low, needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, bid: &str) -> AppInfo {
        AppInfo { name: name.into(), bundle_id: bid.into(), pid: 1 }
    }

    #[test]
    fn terminals_are_coding_agents() {
        for (n, b, label) in [
            ("Terminal", "com.apple.Terminal", "Terminal"),
            ("iTerm2", "com.googlecode.iterm2", "iTerm2"),
            ("Ghostty", "com.mitchellh.ghostty", "Ghostty"),
            ("Warp", "dev.warp.Warp-Stable", "Warp"),
            ("WezTerm", "com.github.wez.wezterm", "WezTerm"),
            ("Alacritty", "org.alacritty", "Alacritty"),
            ("kitty", "net.kovidgoyal.kitty", "kitty"),
            ("Orca", "com.stablyai.orca", "Orca"),
        ] {
            let tg = target_for(&app(n, b), Some("zsh"));
            assert_eq!(tg, t(TargetKind::CodingAgent, label), "{n}");
        }
    }

    #[test]
    fn terminal_title_sharpens_label() {
        let term = app("Terminal", "com.apple.Terminal");
        assert_eq!(target_for(&term, Some("promptmaxxer \u{2014} claude \u{2014} 80×24")).label, "Claude Code");
        assert_eq!(target_for(&term, Some("Claude Code")).label, "Claude Code");
        assert_eq!(target_for(&term, Some("\u{2733} Fix the login bug")).label, "Claude Code");
        assert_eq!(target_for(&term, Some("app \u{2014} codex \u{2014} 120×40")).label, "Codex");
        assert_eq!(target_for(&term, Some("app \u{2014} gemini")).label, "Gemini CLI");
        assert_eq!(target_for(&term, Some("app \u{2014} -zsh \u{2014} 80×24")).label, "Terminal");
        assert_eq!(target_for(&term, None).label, "Terminal");
        // "claudette" is not "claude".
        assert_eq!(target_for(&term, Some("claudette")).label, "Terminal");
    }

    #[test]
    fn editors_are_coding_agents() {
        for (n, b, label) in [
            ("Code", "com.microsoft.VSCode", "VS Code"),
            ("Cursor", "com.todesktop.230313mzl4w4u92", "Cursor"),
            ("Zed", "dev.zed.Zed", "Zed"),
            ("Xcode", "com.apple.dt.Xcode", "Xcode"),
            ("Windsurf", "com.exafunction.windsurf", "Windsurf"),
        ] {
            assert_eq!(target_for(&app(n, b), Some("main.rs \u{2014} shop")), t(TargetKind::CodingAgent, label), "{n}");
        }
        // Name only (unknown bundle id) still works.
        assert_eq!(target_for(&app("Cursor", ""), None).kind, TargetKind::CodingAgent);
        // JetBrains.
        assert_eq!(target_for(&app("PyCharm", "com.jetbrains.pycharm"), None), t(TargetKind::CodingAgent, "PyCharm"));
    }

    #[test]
    fn windows_programs() {
        for (n, b, label) in [
            ("WindowsTerminal", "windowsterminal.exe", "Windows Terminal"),
            ("powershell", "powershell.exe", "PowerShell"),
            ("pwsh", "pwsh.exe", "PowerShell"),
            ("cmd", "cmd.exe", "Command Prompt"),
        ] {
            assert_eq!(target_for(&app(n, b), Some("C:\\Users\\x")), t(TargetKind::CodingAgent, label), "{n}");
        }
        assert_eq!(target_for(&app("WindowsTerminal", "windowsterminal.exe"), Some("\u{2733} Claude Code")).label, "Claude Code");
        assert_eq!(target_for(&app("Code", "code.exe"), None), t(TargetKind::CodingAgent, "VS Code"));
        assert_eq!(target_for(&app("Cursor", "cursor.exe"), None), t(TargetKind::CodingAgent, "Cursor"));
        assert_eq!(target_for(&app("pycharm64", "pycharm64.exe"), None), t(TargetKind::CodingAgent, "pycharm64"));
        assert_eq!(target_for(&app("chrome", "chrome.exe"), Some("ChatGPT - Google Chrome")), t(TargetKind::Chat, "ChatGPT"));
        assert_eq!(target_for(&app("msedge", "msedge.exe"), Some("Claude - Personal - Microsoft\u{200b} Edge")), t(TargetKind::Chat, "Claude"));
        assert_eq!(target_for(&app("chrome", "chrome.exe"), Some("News - Google Chrome")).kind, TargetKind::Unknown);
        assert_eq!(target_for(&app("claude", "claude.exe"), None), t(TargetKind::Chat, "Claude"));
        assert_eq!(target_for(&app("notepad", "notepad.exe"), None).kind, TargetKind::Unknown);
    }

    #[test]
    fn native_ai_apps() {
        assert_eq!(target_for(&app("Claude", "com.anthropic.claudefordesktop"), Some("Claude")), t(TargetKind::Chat, "Claude"));
        assert_eq!(target_for(&app("Claude", "com.anthropic.claudefordesktop"), Some("Claude Code")), t(TargetKind::CodingAgent, "Claude Code"));
        assert_eq!(target_for(&app("ChatGPT", "com.openai.chat"), None), t(TargetKind::Chat, "ChatGPT"));
        assert_eq!(target_for(&app("ChatGPT", "com.openai.codex"), Some("ChatGPT")), t(TargetKind::Chat, "ChatGPT"));
        assert_eq!(target_for(&app("ChatGPT", "com.openai.codex"), Some("Codex")), t(TargetKind::CodingAgent, "Codex"));
        assert_eq!(target_for(&app("Codex", "com.openai.codex"), None), t(TargetKind::CodingAgent, "Codex"));
        assert_eq!(target_for(&app("Perplexity", "ai.perplexity.mac"), None), t(TargetKind::Research, "Perplexity"));
        assert_eq!(target_for(&app("Microsoft Excel", "com.microsoft.Excel"), None), t(TargetKind::Data, "Excel"));
    }

    #[test]
    fn browser_titles() {
        let brave = app("Brave Browser", "com.brave.Browser");
        let cases = [
            ("ChatGPT", TargetKind::Chat, "ChatGPT"),
            ("Trip ideas - ChatGPT", TargetKind::Chat, "ChatGPT"),
            ("Refactor plan - Claude", TargetKind::Chat, "Claude"),
            ("Claude Code", TargetKind::CodingAgent, "Claude Code"),
            ("Codex", TargetKind::CodingAgent, "Codex"),
            ("Gemini", TargetKind::Chat, "Gemini"),
            ("Perplexity", TargetKind::Research, "Perplexity"),
            ("Midjourney", TargetKind::Image, "Midjourney"),
            ("Lovable", TargetKind::CodingAgent, "Lovable"),
            ("v0 by Vercel", TargetKind::CodingAgent, "v0"),
            ("Runway | Generative AI", TargetKind::Video, "Runway"),
            ("Sora", TargetKind::Video, "Sora"),
            ("Budget 2027 - Google Sheets", TargetKind::Data, "Google Sheets"),
            ("Voiceflow", TargetKind::BotBuilding, "Voiceflow"),
            ("Draft post - Google Docs", TargetKind::Writing, "Google Docs"),
        ];
        for (title, kind, label) in cases {
            assert_eq!(target_for(&brave, Some(title)), t(kind, label), "{title}");
        }
        // Site name (last part) wins over a mention earlier in the title.
        assert_eq!(target_for(&brave, Some("ChatGPT vs Claude - Perplexity")).label, "Perplexity");
        // Not a word match.
        assert_eq!(target_for(&brave, Some("Sorabji concert tickets")).kind, TargetKind::Unknown);
        assert_eq!(target_for(&brave, Some("Weather")).kind, TargetKind::Unknown);
        assert_eq!(target_for(&brave, None).kind, TargetKind::Unknown);
        // Safari and Chrome too.
        assert_eq!(target_for(&app("Safari", "com.apple.Safari"), Some("Claude")).label, "Claude");
        assert_eq!(target_for(&app("Google Chrome", "com.google.Chrome"), Some("Gemini")).label, "Gemini");
    }

    #[test]
    fn other_apps_are_unknown_even_with_ai_words_in_title() {
        assert_eq!(target_for(&app("Notes", "com.apple.Notes"), Some("ChatGPT prompts")), t(TargetKind::Unknown, "Unknown"));
        assert_eq!(target_for(&app("Finder", "com.apple.finder"), Some("Claude")), t(TargetKind::Unknown, "Unknown"));
        assert_eq!(target_for(&app("", ""), None).kind, TargetKind::Unknown);
    }

    #[test]
    fn hint_string() {
        assert_eq!(t(TargetKind::CodingAgent, "Claude Code").hint().as_deref(), Some("coding agent: Claude Code"));
        assert_eq!(t(TargetKind::Unknown, "Unknown").hint(), None);
    }

    #[test]
    fn word_match() {
        assert!(contains_word("hello v0 by", "v0"));
        assert!(!contains_word("v01", "v0"));
        assert!(contains_word("é Claude", "Claude"));
        assert!(!contains_word("éClaude", "Claude"));
        assert!(!contains_word("", "x"));
    }
}
