# P0 许可与来源清单

本清单记录来源，不是法律评审或重新许可授权。当前不得把本仓库当作许可条件已清理的闭源发行物。

| 内容 | 来源 / 已知声明 | 当前处理 |
|---|---|---|
| UI 绘制、主题、窗口辅助代码 | DCX / SquirrelDisk，AGPL-3.0-only 声明 | 保留 REFERENCE-NOTICE.md 与原许可全文；发行方案确定前不移除声明、不改写成宽松许可 |
| Inter-Regular.ttf、Inter-SemiBold.ttf | 从 DCX 复制 | 需核对确切字体版本及上游许可，并附对应许可全文；当前来源说明不替代字体许可 |
| Rust 第三方依赖 | Cargo.lock 与各 crate 元数据 | cargo-deny 固定版本检查 licenses / bans / sources / advisories |
| RDX 自有代码 | workspace crate 未声明统一 license | 需权利人确定发行许可；不以自动忽略无许可错误通过审查 |
| 根目录 LICENSE（MIT） | a8eacf1 切换为 MIT 并移除 AGPL 派生 UI；其后 9f1e544 合并（prefer remote changes）又带回派生 UI，README 许可证说明被同时回退 | 与上表 AGPL 派生内容相抵，定案前不作为有效发行许可承诺 |

## 审查规则

CI 新增 dependencies 门槛，不使用 continue-on-error。许可证未知或工具报告错误时保持失败，不通过扩大白名单、忽略 workspace 或排除依赖掩盖问题。新增许可仅在核对具体包/版本与来源后采用定向例外。来源仅允许 crates.io；git 依赖须明确评审后再加入。

尚待决定：仓库统一发行许可、AGPL 派生代码的发行义务、字体对应版本许可及打包归档。P0 不能因 deny.toml 存在就视为许可审查完成。

## 2026-10-14 实测结果

cargo-deny 0.20.2 对当前 Cargo.lock 的检查：bans、sources 通过；licenses 失败（退出码 4）。具体阻塞为：

- 11 个自有 workspace crate 未声明 license，均报 unlicensed。
- clipboard-win 5.4.1、error-code 3.4.0 的 BSL-1.0 尚未列入允许许可。
- epaint_default_fonts 0.36.2 的 OFL-1.1、Ubuntu-font-1.0 尚未列入允许许可。该项为 crate 自带字体，与复制自 DCX 的 Inter 文件分开核对。

上述第三方包的定向许可例外仍待核对许可全文及分发要求；未自动放宽策略。版本重复仅为警告，不是本次失败原因。

advisories 单独联网检查失败（退出码 1）：RUSTSEC-2026-0192 报告 ttf-parser 0.25.1 停止维护，工具未提供安全升级版本。依赖路径为 winit → sctk-adwaita → ab_glyph → owned_ttf_parser → ttf-parser，来自 Linux/Wayland 窗口装饰依赖。Windows 默认目标的 cargo tree 不显示此路径，必须使用 --target all 检查。此结果是停维护告警，不应改述为已证实的可利用漏洞。后续需验证上游替换或功能裁剪方案，未添加 ignore，也未为通过审计而取消 Wayland 功能。
