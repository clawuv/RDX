# P0 平台支持边界（2026-10-14）

状态：工程范围基线；具体 API 可用性与性能须在对应平台实测。此表不等于平台实现已交付。

| 平台 | 首轮验证目标（不是已验证最低版本） | 捕获 / 输入 / 音频 | 授权与明确不支持 | Broker |
|---|---|---|---|---|
| Windows | Windows 10 22H2、Windows 11，x64，已登录交互桌面 | DXGI / SendInput / 用户会话设备 | 不支持登录、锁屏、UAC 安全桌面及跨用户会话；不自动提权控制高完整性进程 | v1 默认不安装服务 |
| macOS | macOS 13+，Apple Silicon 与 Intel | ScreenCaptureKit / CGEvent / 用户会话设备 | 屏幕录制与辅助功能分别授权；不绕过 TCC；拒绝或撤销即停止相应能力，不支持登录/锁屏控制 | v1 默认不安装特权代理 |
| Linux X11 | Ubuntu 22.04/24.04，x64，用户 X11 会话 | X11/XDamage / XTest / 用户会话设备 | 不跨 DISPLAY/Xauthority 会话；不支持登录和锁屏控制 | 默认用户态 |
| Linux Wayland | 后续按桌面环境逐项验证，初始 MVP 不承诺控制能力 | ScreenCast + PipeWire / RemoteDesktop portal / 用户会话设备 | 必须经过 portal 授权；无实现或被拒绝则不可用，不自动回退 root/uinput | 不用 broker 绕过 portal |

## 能力状态和验收约定

每项能力区分 unsupported、permission-required、denied、available、revoked、temporarily-unavailable。界面不得把占位按钮当作 available。拒绝/撤销时停止生产媒体或注入输入并告知用户；恢复需重新授权，不静默重试提权。

显示器移除、分辨率改变、会话锁定和采集 API 失效应结束当前捕获器并由上层重建；恢复前不得重播旧输入。具体错误码映射属于 P1/P4 平台实现验收。

如以后引入 broker，必须另提 ADR：说明不可在用户态完成的 API、身份与会话校验、IPC ACL、最小权限、撤销、故障恢复及拷贝成本。默认不得联网或解析远程会话数据。

最低系统版本须待三平台真实捕获/输入验收后据实确定；上表的目标版本不能用作对外兼容承诺。
