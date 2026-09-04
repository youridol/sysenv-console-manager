# third_party —— 第三方内核驱动（随 SECM 分发的受控依赖）

> 自研内核驱动 hwmon-driver 已从项目中移除（2026 开源整理），温度/功耗数据源收敛为
> LHM sidecar（LibreHardwareMonitor + PawnIO）→ WinRing0 → ACPI。third_party 仅保留
> 两个第三方驱动依赖。随包分发许可文本见同目录 `OpenLibSys-LICENSE.txt`（WinRing0）与
> `PawnIO/` 内 COPYING（GPL-2.0）。

## WinRing0x64.sys

| 项 | 值 |
|---|---|
| 文件 | `WinRing0x64.sys`（本目录） |
| 版本 | 1.2.0.5 |
| 来源 | 本机 LHM（Libre Hardware Monitor）安装实例提取（`C:\Windows\System32\drivers\WinRing0x64.sys`），实为 OpenLibSys **hotproject 重编译变体**（端口 IO 协议已按此变体 IOCTL 确认：`IOCTL_READ_PORT_DWORD=0x9c4060d4` / `IOCTL_WRITE_PORT_DWORD=0x9c40a0e0`） |
| 签名 | GlobalSign 商业签名，CN=Noriyuki MIYAZAKI（CrystalDiskInfo 作者），`Get-AuthenticodeSignature` = Valid |
| SHA-256 | `11bd2c9f9e2397c9a16e0990e4ed2cf0679498fe0fd418a3dfdac60b5c160ee5` |
| 用途 | WinRing0 温度/功耗通道：LHM sidecar 不可用时，经 WinRing0 端口 IO 读取 AMD SMN 温度与 RAPL 功耗（见 `src-tauri/src/driver_install/winring0.rs`） |

### ⚠️ 安全风险声明

1. **历史 CVE**：WinRing0 系列存在任意端口 IO 漏洞（CVE-2020-14979 等）。本项目
   仅将其作为**受限数据通道**：固定自带本版本、仅使用端口 IO / MSR 读取最小面
   （SMN 温度 / RAPL 功耗），**不做任何任意地址读写操作**。
2. **HVCI 拦截**：内存完整性（HVCI）开启的机器上，`ensure_temp_channel()`
   自动跳过 WinRing0 通道（检测注册表 `DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity`），
   避免与旧驱动加载策略冲突。
3. **微软 Vulnerable Driver Blocklist**：自带版本在名单生效前已签名；若未来被
   封锁，WinRing0 探活失败即自动回退 ACPI，不影响系统稳定。

### 校验方法

```bat
certutil -hashfile WinRing0x64.sys SHA256
rem 期望输出: 11bd2c9f9e2397c9a16e0990e4ed2cf0679498fe0fd418a3dfdac60b5c160ee5
```
