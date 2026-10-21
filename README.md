# pswitch

一个极简的命令行工具，用来给 **Claude Code** 和 **Codex** 切换模型 Provider。

- 只做“直连切换”：直接改写工具自己的配置文件，没有本地代理、没有常驻进程。pswitch 关掉或卸载都不影响工具使用。
- 只改必要字段，其余配置（权限、hooks、MCP、注释、格式）原样保留。
- 第一次修改前自动备份，写文件用原子写入（临时文件 + rename），含密钥的文件权限为 `0600`。

## 安装

```bash
cargo build --release
cp target/release/pswitch ~/.local/bin/
```

## 用法

```bash
# 添加 Provider
pswitch add claude myrelay --url https://api.example.com --key sk-xxx --model claude-sonnet-4-5
pswitch add codex  myrelay --url https://api.example.com/v1 --key sk-xxx --model gpt-5 [--wire responses]

pswitch list                 # 列出所有 Provider（* 为当前，key 已打码）
pswitch current              # 两个工具各自当前用的 Provider
pswitch use claude myrelay   # 切换
pswitch use codex  myrelay
pswitch use claude official  # 切回官方
pswitch use codex  official  # 切回官方，并还原 ChatGPT 登录
pswitch remove codex myrelay # 删除（不能删除正在使用的）
pswitch restore              # 把所有配置文件恢复成 pswitch 第一次修改之前的样子
```

Claude Code 切换后新开一个会话生效；Codex 需要重启。

## 改了什么

**Claude Code** —— `~/.claude/settings.json` 的 `env` 里只动三个键：
`ANTHROPIC_BASE_URL`、`ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_MODEL`。切回 official 时删除这三个键。

**Codex** —— `~/.codex/config.toml`（用 `toml_edit` 修改，保留注释和格式）：

```toml
model = "gpt-5"
model_provider = "pswitch_myrelay"

[model_providers.pswitch_myrelay]
name = "myrelay"
base_url = "https://api.example.com/v1"
wire_api = "responses"
requires_openai_auth = true
```

`requires_openai_auth = true` 让 Codex 从 `~/.codex/auth.json` 读取密钥，pswitch 会写入
`{"auth_mode": "apikey", "OPENAI_API_KEY": "..."}`。
如果原来的 `auth.json` 是 ChatGPT 登录（OAuth token），会先暂存到 `~/.pswitch/codex-login.bak`，
`use codex official` 时还原，并删除 `model_provider` 和 pswitch 添加的 `[model_providers.pswitch_*]` 段，`model` 恢复为原值。

> 注意：新版 Codex 已移除 `wire_api = "chat"`，只支持 `responses`。`--wire chat` 仍可保存（给旧版用），但会给出警告。

## 数据文件

```
~/.pswitch/
├── providers.json      # Provider 列表（含密钥，0600）
├── state.json          # 每个工具当前使用的 Provider
├── codex-login.bak     # 暂存的 ChatGPT 登录（仅在需要时存在）
└── backups/            # 每个配置文件第一次被修改前的原样备份
```

环境变量 `PSWITCH_HOME` 可以改数据目录；`HOME` 决定 `~/.claude`、`~/.codex` 的位置（测试时用临时目录）。

## 测试

```bash
cargo test
```
