# P0 许可与来源清单

本清单记录来源，不是法律评审或重新许可授权。

## 发行许可决策（2026-10-14，权利人裁定）

- 仓库发行许可定为 **AGPL-3.0-only**：根目录 LICENSE 替换为 GNU AGPL v3 全文（与 DCX/SquirrelDisk 来源声明一致），workspace 11 个 crate 声明 `AGPL-3.0-only`，`deny.toml` 白名单加入 AGPL-3.0-only（主要覆盖自有 crate）。
- 此前的冲突（a8eacf1 切 MIT 并移除派生 UI，9f1e544 合并带回派生 UI）就此消解：派生 UI 的 AGPL 义务与仓库许可一致，REFERENCE-NOTICE.md 与 SQUIRRELDISK-LICENSE 继续保留。

## 来源清单

| 内容 | 来源 / 已知声明 | 当前处理 |
|---|---|---|
| UI 绘制、主题、窗口辅助代码 | DCX / SquirrelDisk，AGPL-3.0-only 声明 | 与仓库许可一致；保留 REFERENCE-NOTICE.md 与原许可全文 |
| Inter-Regular.ttf、Inter-SemiBold.ttf | 从 DCX 复制；TTF name 表核对为 Inter v4.001（git-66647c0bb） | 上游 rsms/inter 以 SIL OFL-1.1 授权（保留字体名 Inter），与仓库 AGPL 兼容（字体许可独立于代码许可）；文件未嵌入许可文本，发行打包须附 OFL-1.1 全文与署名 |
| Rust 第三方依赖 | Cargo.lock 与各 crate 元数据 | cargo-deny 固定版本检查 licenses / bans / sources / advisories |
| RDX 自有代码 | workspace crate 声明 AGPL-3.0-only（2026-10-14 起） | 随 LICENSE 一致；新文件按 AGPL 头部惯例处理由发行工程（P7）统一定 |

## 审查规则

CI 新增 dependencies 门槛，不使用 continue-on-error。许可证未知或工具报告错误时保持失败，不通过扩大白名单、忽略 workspace 或排除依赖掩盖问题。新增许可仅在核对具体包/版本与来源后采用定向例外。来源仅允许 crates.io；git 依赖须明确评审后再加入。

尚待决定：发行打包归档与 OFL-1.1 全文随附；clipboard-win / error-code 的 BSL-1.0、epaint 自带字体的 OFL-1.1 / Ubuntu-font 定向例外核对；ttf-parser 停维护告警的替换或裁剪处置。仓库发行许可已定 AGPL-3.0-only，Inter 字体版本与许可已核对。

## 2026-10-14 实测结果

cargo-deny 0.20.2 对当前 Cargo.lock 的检查：bans、sources 通过；licenses 失败（退出码 4）。具体阻塞为：

- 11 个自有 workspace crate 未声明 license，均报 unlicensed。
- clipboard-win 5.4.1、error-code 3.4.0 的 BSL-1.0 尚未列入允许许可。
- epaint_default_fonts 0.36.2 的 OFL-1.1、Ubuntu-font-1.0 尚未列入允许许可。该项为 crate 自带字体，与复制自 DCX 的 Inter 文件分开核对。

上述第三方包的定向许可例外仍待核对许可全文及分发要求；未自动放宽策略。版本重复仅为警告，不是本次失败原因。

advisories 单独联网检查失败（退出码 1）：RUSTSEC-2026-0192 报告 ttf-parser 0.25.1 停止维护，工具未提供安全升级版本。依赖路径为 winit → sctk-adwaita → ab_glyph → owned_ttf_parser → ttf-parser，来自 Linux/Wayland 窗口装饰依赖。Windows 默认目标的 cargo tree 不显示此路径，必须使用 --target all 检查。此结果是停维护告警，不应改述为已证实的可利用漏洞。后续需验证上游替换或功能裁剪方案，未添加 ignore，也未为通过审计而取消 Wayland 功能。
