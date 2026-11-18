# DataPilot — 开发约定（给 Claude Code）

## 版本管理（必须遵守）
- 仓库用 git 管理（只在本机，不推送到任何远程）。**每完成一项改动（代码、文档、配置）并通过相关测试后，立即提交一次**；一次提交只包含一件事，提交信息用一句话说明做了什么。
- 改动前先 `git status` 确认工作区干净；出问题时用 `git diff` / `git checkout -- <文件>` / `git revert` 回退，不要靠手工恢复。
- 不提交：`build/`、`target/`、`node_modules/`、`dist/`、`public/web/`、安装包、`.datapilot/`、任何密钥或 config.json。
- **备份（与 git 配合）**：
  1. 动手改代码前：`git status` 必须干净（有未提交改动先提交），这样任何改动都能 `git diff` 查看、`git checkout -- <文件>` / `git revert` 撤回。
  2. 每完成一项改动并测试通过：`git add -A && git commit -m "…"`，然后运行 `bash scripts/backup.sh`，把完整历史打成 git bundle 存到项目外的 `~/DataPilot-backups/`（保留最近 30 份），防止整个文件夹或 `.git` 丢失。
  3. 删除、批量改写、大重构之前：先提交 + 备份一次；删除不在 git 里的文件（如 `build/` 下的产物）前，先打 tar 包到 `build/backup/`。
  4. 恢复：单个文件 `git checkout <提交> -- <文件>`；整个仓库 `git clone ~/DataPilot-backups/<文件>.bundle DataPilot-restored`。
- 2026-11-17 事故：一个 Python 补丁脚本在断言失败前用 `open(p,'w')` 打开了 `agent.rs`，把文件清空。之后修改文件一律**先读入、全部替换成功后再写回**（或用 Edit 工具）；写入失败不得留下空文件。

## 结构
- `datapilot-core/`：Rust 核心（agent 循环、工具、权限、会话、UI 数据层），stdio JSON-RPC。
- `apps/desktop/`：Tauri 2 + React 界面（Mac / Windows 共用）。
- `Resources/`：Python 内核、网页资产（看板 / 图表 / ER 图）、提示词、内置 skills。
- `protocol/`：RPC / 事件 schema。新增 RPC 必须加入 `protocol/rpc.schema.json` 的 Method 枚举（Rust 测试会检查），再运行 `node apps/desktop/scripts/gen-protocol.mjs`。

## 测试
- Rust：`export PATH="$HOME/.cargo/bin:$PATH"; (cd datapilot-core && cargo test --release)`
- 前端（`apps/desktop`）：`npx tsc --noEmit && npx vitest run`；端到端 `node e2e/run.mjs`（b1）和 `node e2e/run.mjs b2-chart-sql`。
- Python 内核：`for t in tests/python/test_*.py; do build/vendor/python/bin/python3 -I $t; done`

## 打包
- Mac：`bash scripts/build_dmg.sh` → `build/DataPilot-<版本>.dmg`；安装到 `/Applications/DataPilot.app`。
- Windows：`bash apps/desktop/scripts/build_win.sh`（NSIS 下载常因网络失败，重试即可）；打完核对：没有比安装包更新的源码、核心 exe 三处哈希一致、前端资源都在 exe 里、`Resources`/`protocol`/Python 与源码一致。

## 其他
- 回复用户用中文。
- API Key / 数据库密码只在钥匙串 / 凭据管理器，绝不写进仓库文件或命令行参数。
- 看板数据集不要为提速而粗化。
