# 09 · 构建与发布

## 依赖
Rust（stable）、Node 20+、Xcode 命令行工具（codesign / hdiutil）、网络（GitHub 下载可经 `DP_PROXY` 代理；PyPI 用清华镜像）。

## 一键打包
```bash
./scripts/build_dmg.sh            # → build/DataPilot-<版本>.dmg
```
内部步骤：
1. `scripts/fetch_vendor.sh`：下载 python-build-standalone 3.12（aarch64 install_only_stripped，约 25MB）到 `build/vendor/python`，下载 ECharts 6.1 到 `Resources/web/vendor/`。
2. `scripts/build_venv.sh`：按 `scripts/requirements.lock`（由 `requirements.in` 经 `uv pip compile` 生成）把 pandas/duckdb/pyarrow/openpyxl/calamine/sqlglot 等装进内置 Python；删除测试与头文件。约 300MB。
3. `apps/desktop/scripts/build_mac.sh`：编译 `datapilot-core`（sidecar）→ `tauri build` 打 `DataPilot.app`（前端 + Resources + 内置 Python）→ 逐个签名 `.so/.dylib`、python 可执行文件和核心 → Hardened Runtime 签名 App。
4. `hdiutil create -format ULFO`，含 `/Applications` 快捷方式，输出 `build/DataPilot-<版本>.dmg`。

环境变量：
| 变量 | 作用 |
|---|---|
| `DP_PROXY` | GitHub 下载代理（默认不用） |
| `DP_PUBLIC=1` | 对外发布版：不带 `Resources/defaults.local.json`（单位预设），并检查包内不含其中的地址 |
| `DP_PYPI_INDEX` | PyPI 镜像（默认清华） |
| `DP_SIGN_ID` | 签名身份，默认 `-`（ad-hoc）；如 `Developer ID Application: XXX (TEAMID)` |
| `DP_NOTARY_PROFILE` | `xcrun notarytool store-credentials` 保存的配置名，设置后自动公证并 staple |
| `SKIP_APP_BUILD=1` | 复用已有的 `DataPilot.app`，只重新打 DMG |

## 单独打 App / Windows 安装包
```bash
apps/desktop/scripts/build_mac.sh   # → apps/desktop/src-tauri/target/release/bundle/macos/DataPilot.app（DMG=1 同时出 dmg）
apps/desktop/scripts/build_win.sh   # Mac 上交叉编译 Windows 安装包 → src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/DataPilot_<版本>_x64-setup.exe
```
Windows 交叉编译需要：`rustup target add x86_64-pc-windows-msvc`、`cargo install cargo-xwin`、`brew install makensis llvm`（用 `llvm-rc`）、`~/.local/xwin-bin` 里 `lld-link`/`llvm-lib`/`llvm-rc` 链接、uv。
Windows Python 运行时：`python3 -I scripts/fetch_vendor.py --target windows` → `build/vendor-win/python`（只装 win_amd64 二进制 wheel，不执行下载的代码）。
交叉编译不可用时，在 Windows 上运行 `scripts\windows\build.ps1`。安装包未签名，首次运行 SmartScreen 会提示；实测清单见 `docs/21-windows-test-checklist.md`。

## 签名与 Gatekeeper
- 当前为 **ad-hoc 签名**：拷到其它 Mac 首次打开会被拦截 → “系统设置 → 隐私与安全性 → 仍要打开”，或 `xattr -dr com.apple.quarantine /Applications/DataPilot.app`。
- 不启用 App Sandbox（需嵌套 sandbox-exec）；启用 Hardened Runtime。
- 正式分发：设置 `DP_SIGN_ID` 与 `DP_NOTARY_PROFILE` 后重新运行脚本。
- Computer Use 所需的“屏幕录制/辅助功能”授权绑定签名；ad-hoc 每次重建签名变化，需要重新授权，Developer ID 可解决。

## 开发
```bash
(cd apps/desktop && npm run tauri dev)                # 开发模式运行（使用仓库内的 Resources 与 build/vendor/python）
(cd datapilot-core && cargo test --release)           # 核心单元 + 黄金样本
(cd apps/desktop && npx tsc --noEmit && npx vitest run && node e2e/run.mjs)   # 前端类型 / 单元 / 端到端
build/vendor/python/bin/python3 -I tests/python/test_kernel.py        # 沙箱内核测试
build/vendor/python/bin/python3 -I tests/python/test_concurrency.py   # 内核并发 / 取消 / 顺序测试
build/vendor/python/bin/python3 -I scripts/run_eval.py --base-url ... --model ...   # Text-to-SQL 评测
```

## 签名身份与钥匙串弹窗
- **API Key 的存储方式**：仍然保存在用户的登录钥匙串（加密存储），但改为通过系统自带的 `/usr/bin/security` 读写，与 Claude Code CLI 的做法相同。钥匙串条目的访问控制授权给这个由 Apple 签名的工具，而不是授权给 DataPilot 本身，所以无论 App 用 ad-hoc 签名还是正式签名、怎么更新，都不会反复弹出“想要使用钥匙串中的机密信息”。
  - 旧版本通过 SecItem 创建的条目，第一次读取时会自动迁移（删除后用 security 重新写入）。
  - 代价：同一用户下运行的其他程序也能通过 `security find-generic-password -s io.github.datapilot.apikeys -w` 读出 Key，隔离性比 Codex 那种按 Developer ID 团队授权的方式弱。
- **签名身份**：`apps/desktop/scripts/build_mac.sh` 优先使用 `DP_SIGN_ID`，否则使用本机的 “Developer ID Application” / “Apple Development” 证书，都没有时用 ad-hoc（`-`）。
  - 本机开发可设 `DP_SIGN_ID="Apple Development: …"`，这样屏幕录制和辅助功能的授权在重新构建后仍然有效。
  - Apple Development 证书签的包不能分发给别人。

## 分发给其他人
| 方案 | 用户首次打开 | 成本 | 建议 |
|---|---|---|---|
| ad-hoc 签名 DMG（当前默认） | macOS 15+ 会拦截：系统设置 → 隐私与安全性 → “仍要打开”，或执行 `xattr -dr com.apple.quarantine /Applications/DataPilot.app` | 免费 | 适合内部小范围试用 |
| Developer ID 签名 + 公证（notarization） | 双击即可打开，没有警告 | Apple Developer Program 99 美元/年（可先问学校或机构是否已有组织账号） | **面向很多用户时推荐** |
| Mac App Store | 无警告 | 同上，另需开启 App Sandbox | 不适用：App Sandbox 内无法嵌套 sandbox-exec |

公证流程：`DP_SIGN_ID="Developer ID Application: XXX (TEAMID)" DP_NOTARY_PROFILE=<notarytool 钥匙串配置名> scripts/build_dmg.sh`。

## 中性标识（0.2 起，为开源准备）
- Bundle ID：`io.github.datapilot.app`；钥匙串服务名：`io.github.datapilot.apikeys`。
- 旧服务名的 Key 迁移已在 0.1.11 移除（只读新服务名）。数据目录 `~/Library/Application Support/DataPilot` 不变。
- 注意：Bundle ID 变化后，macOS 会要求重新授予“屏幕录制 / 辅助功能”权限（仅 Computer Use 需要）。
- 许可：根目录 `LICENSE`（Apache-2.0）与 `THIRD_PARTY_NOTICES.md`。仓库里不含任何单位的网关 / OCR 地址；单位预设放在不进 git 的 `Resources/defaults.local.json`，对外安装包用 `DP_PUBLIC=1` 打。
