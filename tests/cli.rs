use std::fs;
use std::path::Path;
use std::process::Command;

const CLAUDE: &str = r#"{
  "permissions": { "allow": ["Bash(ls:*)"] },
  "env": { "FOO": "bar", "ANTHROPIC_MODEL": "orig-model" },
  "hooks": {},
  "zzz_last": 1
}
"#;

const CODEX: &str = r#"# my codex config
model = "gpt-5"  # inline comment
approval_policy = "on-request"

[mcp_servers.docs]
# keep me
command = "npx"

[model_providers.mine]
name = "Mine"
base_url = "https://mine.example/v1"
"#;

const CHATGPT_AUTH: &str = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":"x","access_token":"y","refresh_token":"z"}}"#;

fn setup() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join(".claude")).unwrap();
    fs::create_dir_all(d.path().join(".codex")).unwrap();
    fs::write(d.path().join(".claude/settings.json"), CLAUDE).unwrap();
    fs::write(d.path().join(".codex/config.toml"), CODEX).unwrap();
    fs::write(d.path().join(".codex/auth.json"), CHATGPT_AUTH).unwrap();
    d
}

fn run(home: &Path, args: &[&str]) -> (bool, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_pswitch"))
        .args(args)
        .env("HOME", home)
        .env_remove("PSWITCH_HOME")
        .output()
        .unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

fn ok(home: &Path, args: &[&str]) -> String {
    let (s, out) = run(home, args);
    assert!(s, "{args:?} failed: {out}");
    out
}

fn read(home: &Path, rel: &str) -> String {
    fs::read_to_string(home.join(rel)).unwrap()
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn claude_switch_preserves_and_official_removes() {
    let d = setup();
    let h = d.path();
    ok(h, &["add", "claude", "relay", "--url", "https://r.example", "--key", "sk-claude-123456", "--model", "m1"]);
    ok(h, &["use", "claude", "relay"]);
    let v: serde_json::Value = serde_json::from_str(&read(h, ".claude/settings.json")).unwrap();
    assert_eq!(v["env"]["ANTHROPIC_BASE_URL"], "https://r.example");
    assert_eq!(v["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-claude-123456");
    assert_eq!(v["env"]["ANTHROPIC_MODEL"], "m1");
    assert_eq!(v["env"]["FOO"], "bar");
    assert_eq!(v["permissions"]["allow"][0], "Bash(ls:*)");
    let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["permissions", "env", "hooks", "zzz_last"], "key order preserved");
    assert_eq!(read(h, ".pswitch/backups/claude-settings.json"), CLAUDE);
    #[cfg(unix)]
    {
        assert_eq!(mode(&h.join(".claude/settings.json")), 0o600);
        assert_eq!(mode(&h.join(".pswitch/providers.json")), 0o600);
    }
    assert!(ok(h, &["current"]).contains("claude: relay"));
    assert!(!ok(h, &["list"]).contains("sk-claude-123456"), "key masked in list");

    ok(h, &["use", "claude", "official"]);
    let v: serde_json::Value = serde_json::from_str(&read(h, ".claude/settings.json")).unwrap();
    for k in ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_MODEL"] {
        assert!(v["env"].get(k).is_none());
    }
    assert_eq!(v["env"]["FOO"], "bar");
    assert!(ok(h, &["current"]).contains("claude: official"));
}

#[test]
fn codex_switch_stash_and_official_restore() {
    let d = setup();
    let h = d.path();
    ok(h, &["add", "codex", "relay", "--url", "https://r.example/v1", "--key", "sk-codex-abcdef", "--model", "gpt-x"]);
    ok(h, &["add", "codex", "other", "--url", "https://o.example/v1", "--key", "sk-other-abcdef", "--model", "gpt-y"]);
    ok(h, &["use", "codex", "relay"]);
    let cfg = read(h, ".codex/config.toml");
    assert!(cfg.starts_with("# my codex config\n"));
    assert!(cfg.contains("model = \"gpt-x\"  # inline comment"), "{cfg}");
    assert!(cfg.contains("# keep me"));
    assert!(cfg.contains("[model_providers.mine]"));
    assert!(cfg.contains("model_provider = \"pswitch_relay\""));
    let doc: toml_edit::DocumentMut = cfg.parse().unwrap();
    let t = &doc["model_providers"]["pswitch_relay"];
    assert_eq!(t["base_url"].as_str(), Some("https://r.example/v1"));
    assert_eq!(t["wire_api"].as_str(), Some("responses"));
    assert_eq!(t["requires_openai_auth"].as_bool(), Some(true));
    let auth: serde_json::Value = serde_json::from_str(&read(h, ".codex/auth.json")).unwrap();
    assert_eq!(auth["OPENAI_API_KEY"], "sk-codex-abcdef");
    assert_eq!(read(h, ".pswitch/codex-login.bak"), CHATGPT_AUTH);
    assert_eq!(read(h, ".pswitch/backups/codex-config.toml"), CODEX);

    // switching between providers replaces our table and keeps the stash
    ok(h, &["use", "codex", "other"]);
    let cfg = read(h, ".codex/config.toml");
    assert!(!cfg.contains("pswitch_relay"));
    assert!(cfg.contains("[model_providers.pswitch_other]"));
    assert_eq!(read(h, ".pswitch/codex-login.bak"), CHATGPT_AUTH);

    // removing the active one is refused
    assert!(!run(h, &["remove", "codex", "other"]).0);

    ok(h, &["use", "codex", "official"]);
    let cfg = read(h, ".codex/config.toml");
    assert!(!cfg.contains("model_provider ="));
    assert!(!cfg.contains("pswitch_"));
    assert!(cfg.contains("model = \"gpt-5\""));
    assert!(cfg.contains("[model_providers.mine]"));
    assert_eq!(read(h, ".codex/auth.json"), CHATGPT_AUTH);
    assert!(!h.join(".pswitch/codex-login.bak").exists());
    ok(h, &["remove", "codex", "other"]);
}

#[test]
fn restore_puts_back_originals() {
    let d = setup();
    let h = d.path();
    ok(h, &["add", "claude", "a", "--url", "u", "--key", "k", "--model", "m"]);
    ok(h, &["add", "codex", "b", "--url", "u", "--key", "k", "--model", "m", "--wire", "chat"]);
    ok(h, &["use", "claude", "a"]);
    ok(h, &["use", "codex", "b"]);
    ok(h, &["restore"]);
    assert_eq!(read(h, ".claude/settings.json"), CLAUDE);
    assert_eq!(read(h, ".codex/config.toml"), CODEX);
    assert_eq!(read(h, ".codex/auth.json"), CHATGPT_AUTH);
    assert!(ok(h, &["current"]).contains("codex: official"));
}

#[test]
fn missing_configs_are_created_and_restore_deletes_them() {
    let d = tempfile::tempdir().unwrap();
    let h = d.path();
    ok(h, &["add", "codex", "b", "--url", "u", "--key", "k", "--model", "m"]);
    ok(h, &["add", "claude", "a", "--url", "u", "--key", "k", "--model", "m"]);
    ok(h, &["use", "codex", "b"]);
    ok(h, &["use", "claude", "a"]);
    assert!(h.join(".codex/config.toml").exists());
    ok(h, &["restore"]);
    assert!(!h.join(".codex/config.toml").exists());
    assert!(!h.join(".codex/auth.json").exists());
    assert!(!h.join(".claude/settings.json").exists());
}

#[test]
fn pswitch_home_override_and_validation() {
    let d = setup();
    let data = tempfile::tempdir().unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_pswitch"))
        .args(["add", "claude", "x", "--url", "u", "--key", "k", "--model", "m"])
        .env("HOME", d.path())
        .env("PSWITCH_HOME", data.path())
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(data.path().join("providers.json").exists());
    assert!(!d.path().join(".pswitch").exists());

    let h = d.path();
    assert!(!run(h, &["add", "claude", "official", "--url", "u", "--key", "k", "--model", "m"]).0);
    assert!(!run(h, &["add", "codex", "w", "--url", "u", "--key", "k", "--model", "m", "--wire", "bogus"]).0);
    assert!(!run(h, &["use", "claude", "nope"]).0);
    ok(h, &["add", "claude", "dup", "--url", "u", "--key", "k", "--model", "m"]);
    assert!(!run(h, &["add", "claude", "dup", "--url", "u", "--key", "k", "--model", "m"]).0);
}
