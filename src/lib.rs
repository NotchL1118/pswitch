//! pswitch core: provider store + direct config-file editing for Claude Code and Codex.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CLAUDE_KEYS: [&str; 3] = ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_MODEL"];
pub const PROVIDER_PREFIX: &str = "pswitch_";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Claude,
    Codex,
}

impl std::str::FromStr for Tool {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "claude" => Ok(Tool::Claude),
            "codex" => Ok(Tool::Codex),
            _ => bail!("unknown tool `{s}` (expected claude or codex)"),
        }
    }
}

impl std::fmt::Display for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Tool::Claude => "claude",
            Tool::Codex => "codex",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    pub name: String,
    pub tool: Tool,
    pub base_url: String,
    pub key: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub providers: Vec<Provider>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// tool -> active provider name ("official" or absent = official)
    #[serde(default)]
    pub current: BTreeMap<String, String>,
    /// Codex `model` value before pswitch first touched it (None = key absent).
    #[serde(default)]
    pub codex_original_model: Option<Option<String>>,
}

/// All filesystem locations. Build with `Paths::from_env()` or `Paths::new(home, pswitch_home)`.
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub data: PathBuf,
}

impl Paths {
    pub fn new(home: impl Into<PathBuf>, data: Option<PathBuf>) -> Self {
        let home = home.into();
        let data = data.unwrap_or_else(|| home.join(".pswitch"));
        Paths { home, data }
    }
    pub fn from_env() -> Result<Self> {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        let data = std::env::var_os("PSWITCH_HOME").map(PathBuf::from);
        Ok(Self::new(PathBuf::from(home), data))
    }
    pub fn providers(&self) -> PathBuf { self.data.join("providers.json") }
    pub fn state(&self) -> PathBuf { self.data.join("state.json") }
    pub fn backups(&self) -> PathBuf { self.data.join("backups") }
    pub fn codex_login_stash(&self) -> PathBuf { self.data.join("codex-login.bak") }
    pub fn claude_settings(&self) -> PathBuf { self.home.join(".claude/settings.json") }
    pub fn codex_config(&self) -> PathBuf { self.home.join(".codex/config.toml") }
    pub fn codex_auth(&self) -> PathBuf { self.home.join(".codex/auth.json") }
    /// (backup file name, live path) for every config file we may touch.
    pub fn managed(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("claude-settings.json", self.claude_settings()),
            ("codex-config.toml", self.codex_config()),
            ("codex-auth.json", self.codex_auth()),
        ]
    }
}

// ---------- fs helpers ----------

/// Atomic write: temp file in the same dir, fsync, rename. `secret` => 0600.
pub fn atomic_write(path: &Path, data: &[u8], secret: bool) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.pswitch-tmp-{}",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // keep original perms when not secret, else 0600
            let mode = if secret {
                0o600
            } else {
                existing_mode(path).unwrap_or(0o644)
            };
            opts.mode(mode);
        }
        let mut f = opts.open(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn existing_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).ok().map(|m| m.permissions().mode() & 0o777)
}

fn read_opt(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

// ---------- the app ----------

pub struct App {
    pub paths: Paths,
}

impl App {
    pub fn new(paths: Paths) -> Self { App { paths } }

    pub fn load_store(&self) -> Result<Store> {
        Ok(match read_opt(&self.paths.providers())? {
            Some(s) => serde_json::from_str(&s).context("parse providers.json")?,
            None => Store::default(),
        })
    }
    fn save_store(&self, s: &Store) -> Result<()> {
        atomic_write(&self.paths.providers(), serde_json::to_string_pretty(s)?.as_bytes(), true)
    }
    pub fn load_state(&self) -> Result<State> {
        Ok(match read_opt(&self.paths.state())? {
            Some(s) => serde_json::from_str(&s).context("parse state.json")?,
            None => State::default(),
        })
    }
    fn save_state(&self, s: &State) -> Result<()> {
        atomic_write(&self.paths.state(), serde_json::to_string_pretty(s)?.as_bytes(), false)
    }

    pub fn add(&self, p: Provider) -> Result<()> {
        if p.name == "official" {
            bail!("`official` is reserved");
        }
        if !p.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            bail!("provider name may only contain [A-Za-z0-9_-]");
        }
        let mut p = p;
        match p.tool {
            Tool::Claude => p.wire_api = None,
            Tool::Codex => {
                let w = p.wire_api.get_or_insert_with(|| "responses".into());
                if w != "responses" && w != "chat" {
                    bail!("--wire must be responses or chat");
                }
            }
        }
        let mut s = self.load_store()?;
        if s.providers.iter().any(|x| x.tool == p.tool && x.name == p.name) {
            bail!("provider {} / {} already exists (remove it first)", p.tool, p.name);
        }
        s.providers.push(p);
        self.save_store(&s)
    }

    pub fn remove(&self, tool: Tool, name: &str) -> Result<()> {
        let st = self.load_state()?;
        if st.current.get(&tool.to_string()).map(String::as_str) == Some(name) {
            bail!("{tool}/{name} is currently active; run `pswitch use {tool} official` first");
        }
        let mut s = self.load_store()?;
        let before = s.providers.len();
        s.providers.retain(|x| !(x.tool == tool && x.name == name));
        if s.providers.len() == before {
            bail!("no provider {tool}/{name}");
        }
        self.save_store(&s)
    }

    pub fn current(&self, tool: Tool) -> Result<String> {
        Ok(self
            .load_state()?
            .current
            .get(&tool.to_string())
            .cloned()
            .unwrap_or_else(|| "official".into()))
    }

    /// Copy the live file to backups/ the first time we are about to modify it.
    /// A missing original is recorded with a `.absent` marker.
    fn backup_once(&self, backup_name: &str, live: &Path) -> Result<()> {
        let dir = self.paths.backups();
        let b = dir.join(backup_name);
        let marker = dir.join(format!("{backup_name}.absent"));
        if b.exists() || marker.exists() {
            return Ok(());
        }
        fs::create_dir_all(&dir)?;
        match read_opt(live)? {
            Some(c) => atomic_write(&b, c.as_bytes(), true)?,
            None => atomic_write(&marker, b"", false)?,
        }
        Ok(())
    }

    pub fn use_provider(&self, tool: Tool, name: &str) -> Result<()> {
        let provider = if name == "official" {
            None
        } else {
            let s = self.load_store()?;
            Some(
                s.providers
                    .into_iter()
                    .find(|x| x.tool == tool && x.name == name)
                    .with_context(|| format!("no provider {tool}/{name}; see `pswitch list`"))?,
            )
        };
        let mut st = self.load_state()?;
        match tool {
            Tool::Claude => self.apply_claude(provider.as_ref())?,
            Tool::Codex => self.apply_codex(provider.as_ref(), &mut st)?,
        }
        if provider.is_some() {
            st.current.insert(tool.to_string(), name.to_string());
        } else {
            st.current.remove(&tool.to_string());
        }
        self.save_state(&st)
    }

    fn apply_claude(&self, p: Option<&Provider>) -> Result<()> {
        let path = self.paths.claude_settings();
        let existing = read_opt(&path)?;
        if p.is_none() && existing.is_none() {
            return Ok(());
        }
        self.backup_once("claude-settings.json", &path)?;
        let mut root: Value = match &existing {
            Some(s) if !s.trim().is_empty() => {
                serde_json::from_str(s).with_context(|| format!("parse {}", path.display()))?
            }
            _ => Value::Object(Map::new()),
        };
        let obj = root.as_object_mut().context("settings.json root is not an object")?;
        match p {
            Some(p) => {
                let env = obj.entry("env").or_insert_with(|| Value::Object(Map::new()));
                let env = env.as_object_mut().context("settings.json `env` is not an object")?;
                env.insert("ANTHROPIC_BASE_URL".into(), p.base_url.clone().into());
                env.insert("ANTHROPIC_AUTH_TOKEN".into(), p.key.clone().into());
                env.insert("ANTHROPIC_MODEL".into(), p.model.clone().into());
            }
            None => {
                if let Some(env) = obj.get_mut("env").and_then(Value::as_object_mut) {
                    for k in CLAUDE_KEYS {
                        env.shift_remove(k);
                    }
                }
            }
        }
        let mut out = serde_json::to_string_pretty(&root)?;
        out.push('\n');
        atomic_write(&path, out.as_bytes(), true)
    }

    fn apply_codex(&self, p: Option<&Provider>, st: &mut State) -> Result<()> {
        let cfg_path = self.paths.codex_config();
        let auth_path = self.paths.codex_auth();
        let cfg_src = read_opt(&cfg_path)?;
        if p.is_none() && st.codex_original_model.is_none() && !self.paths.codex_login_stash().exists() {
            // never switched by us; nothing to undo
            return Ok(());
        }
        self.backup_once("codex-config.toml", &cfg_path)?;
        self.backup_once("codex-auth.json", &auth_path)?;

        let mut doc: toml_edit::DocumentMut = cfg_src
            .as_deref()
            .unwrap_or("")
            .parse()
            .with_context(|| format!("parse {}", cfg_path.display()))?;

        if st.codex_original_model.is_none() {
            st.codex_original_model =
                Some(doc.get("model").and_then(|v| v.as_str()).map(str::to_string));
        }

        // drop any table we created earlier
        if let Some(mp) = doc.get_mut("model_providers").and_then(|i| i.as_table_like_mut()) {
            let ours: Vec<String> = mp
                .iter()
                .map(|(k, _)| k.to_string())
                .filter(|k| k.starts_with(PROVIDER_PREFIX))
                .collect();
            for k in ours {
                mp.remove(&k);
            }
            if mp.is_empty() {
                doc.remove("model_providers");
            }
        }

        match p {
            Some(p) => {
                let id = format!("{PROVIDER_PREFIX}{}", p.name);
                set_str(&mut doc, "model", &p.model);
                set_str(&mut doc, "model_provider", &id);
                if doc.get("model_providers").is_none() {
                    let mut t = toml_edit::Table::new();
                    t.set_implicit(true);
                    doc["model_providers"] = toml_edit::Item::Table(t);
                }
                let mut t = toml_edit::Table::new();
                t["name"] = toml_edit::value(p.name.clone());
                t["base_url"] = toml_edit::value(p.base_url.clone());
                t["wire_api"] = toml_edit::value(p.wire_api.clone().unwrap_or_else(|| "responses".into()));
                // read the key from ~/.codex/auth.json (OPENAI_API_KEY) instead of an env var
                t["requires_openai_auth"] = toml_edit::value(true);
                doc["model_providers"][&id] = toml_edit::Item::Table(t);
                atomic_write(&cfg_path, doc.to_string().as_bytes(), false)?;

                // auth.json: stash ChatGPT login once, then write API key
                if let Some(a) = read_opt(&auth_path)? {
                    if is_chatgpt_login(&a) && !self.paths.codex_login_stash().exists() {
                        atomic_write(&self.paths.codex_login_stash(), a.as_bytes(), true)?;
                    }
                }
                let auth = serde_json::json!({ "auth_mode": "apikey", "OPENAI_API_KEY": p.key });
                atomic_write(&auth_path, serde_json::to_string_pretty(&auth)?.as_bytes(), true)?;
            }
            None => {
                if doc
                    .get("model_provider")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.starts_with(PROVIDER_PREFIX))
                {
                    doc.remove("model_provider");
                }
                match st.codex_original_model.take().flatten() {
                    Some(m) => set_str(&mut doc, "model", &m),
                    None => {
                        doc.remove("model");
                    }
                }
                atomic_write(&cfg_path, doc.to_string().as_bytes(), false)?;

                // auth: stashed ChatGPT login > original backup > remove
                let stash = self.paths.codex_login_stash();
                if let Some(s) = read_opt(&stash)? {
                    atomic_write(&auth_path, s.as_bytes(), true)?;
                    fs::remove_file(&stash)?;
                } else if let Some(orig) = read_opt(&self.paths.backups().join("codex-auth.json"))? {
                    atomic_write(&auth_path, orig.as_bytes(), true)?;
                } else if auth_path.exists() {
                    fs::remove_file(&auth_path)?;
                }
            }
        }
        Ok(())
    }

    /// Put every backed-up file back exactly as it was, then forget the state.
    /// Returns the list of restored paths.
    pub fn restore(&self) -> Result<Vec<PathBuf>> {
        let dir = self.paths.backups();
        let mut done = vec![];
        for (bname, live) in self.paths.managed() {
            let b = dir.join(bname);
            let marker = dir.join(format!("{bname}.absent"));
            if let Some(c) = read_opt(&b)? {
                let secret = bname != "codex-config.toml";
                atomic_write(&live, c.as_bytes(), secret)?;
                done.push(live);
            } else if marker.exists() {
                if live.exists() {
                    fs::remove_file(&live)?;
                }
                done.push(live);
            }
        }
        let stash = self.paths.codex_login_stash();
        if stash.exists() {
            fs::remove_file(stash)?;
        }
        if self.paths.state().exists() {
            fs::remove_file(self.paths.state())?;
        }
        Ok(done)
    }
}

/// auth.json holds ChatGPT OAuth (tokens / auth_mode chatgpt) rather than a plain API key.
pub fn is_chatgpt_login(s: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(s) else { return false };
    let mode = v.get("auth_mode").and_then(Value::as_str);
    matches!(mode, Some("chatgpt") | Some("chatgptAuthTokens"))
        || v.get("tokens").is_some_and(|t| !t.is_null())
}

pub fn mask(key: &str) -> String {
    let n = key.chars().count();
    if n <= 8 {
        return "****".into();
    }
    let head: String = key.chars().take(4).collect();
    let tail: String = key.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

/// Set a top-level string, keeping the existing value's surrounding whitespace/comments.
fn set_str(doc: &mut toml_edit::DocumentMut, key: &str, val: &str) {
    let mut v = toml_edit::Value::from(val);
    if let Some(old) = doc.get(key).and_then(|i| i.as_value()) {
        *v.decor_mut() = old.decor().clone();
    }
    doc[key] = toml_edit::Item::Value(v);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_chatgpt_login() {
        assert!(is_chatgpt_login(r#"{"auth_mode":"chatgpt"}"#));
        assert!(is_chatgpt_login(r#"{"OPENAI_API_KEY":null,"tokens":{"a":1}}"#));
        assert!(!is_chatgpt_login(r#"{"OPENAI_API_KEY":"sk-x"}"#));
        assert!(!is_chatgpt_login("not json"));
    }

    #[test]
    fn masks_keys() {
        assert_eq!(mask("short"), "****");
        assert_eq!(mask("sk-1234567890"), "sk-1…7890");
    }

    #[test]
    fn set_str_keeps_comment() {
        let mut d: toml_edit::DocumentMut = "model = \"a\" # c\n".parse().unwrap();
        set_str(&mut d, "model", "b");
        assert_eq!(d.to_string(), "model = \"b\" # c\n");
    }
}
