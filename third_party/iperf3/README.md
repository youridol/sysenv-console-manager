# iperf3 随包二进制（第三方）

本目录存放随 SECM 分发的 iperf3 Windows 二进制，供「网络诊断 → 网络工具 → iperf3」
使用。**用户无需自行安装 iperf3**。

## 来源

| 项 | 值 |
| --- | --- |
| 上游项目 | [esnet/iperf](https://github.com/esnet/iperf)（iperf3） |
| 版本 | **iperf 3.21**（`iperf 3.21 (cJSON 1.7.15)`，Cygwin 3.6.7-1.x86_64） |
| 二进制构建 | [ar51an/iperf3-win-builds](https://github.com/ar51an/iperf3-win-builds) release `3.21` |
| 下载地址 | `https://github.com/ar51an/iperf3-win-builds/releases/download/3.21/iperf-3.21-win64.zip` |
| zip 摘要（sha256） | `9b73b7e0e0326347b5f4ac4f6a1fc34fe60a5966e5fd172c7bfcd0e1cc93e709` |

> 上游 ESnet 明确**不发布** Windows 二进制（见 `docs/obtaining.rst`：「ESnet does not
> distribute binary packages of iperf3」），Windows 版由第三方构建。上述构建仓库的
> README 声明「不对原始源码做任何修改」，并以 VirusTotal 检查后发布。

## 为什么是两个文件

Windows 版 iperf3 是 **Cygwin 链接**构建（POSIX 兼容层）：

| 文件 | 作用 | 缺失后果 |
| --- | --- | --- |
| `iperf3.exe` | 主程序 | 无 iperf3 可用 |
| `cygwin1.dll` | Cygwin 运行时（POSIX API 实现） | **iperf3.exe 无法启动**（缺 DLL） |

两者**必须一起分发**。`scripts/stage-iperf3.mjs` 会把它们一起复制到
`src-tauri/resources/iperf3/`，并在复制前逐项校验 SHA256。

## 校验和

`SHA256SUMS.txt` 记录当前入库产物的 sha256（格式与 coreutils 一致）：

```
08d35a41132930b3f2657b99cb81ae651ebbc9ac42e387084bb8d76547dbb389  iperf3.exe
0ab76b4724499df54b75b7fa701788f1e77425ce65c8bca0a9f2120598bb8a70  cygwin1.dll
```

构建时 `stage:iperf3` 会**校验不符即中止**，避免来源不明/损坏的二进制被静默打进安装包。

## 许可与合规

### iperf3 本体：BSD-3-Clause

`iperf3.exe` 源码来自 esnet/iperf，为 **BSD-3-Clause**（`LICENSE-IPERF3-BSD-3-Clause.txt`
即上游 LICENSE 全文）。该文件同时列明内嵌第三方代码的许可（cJSON/MIT、
`net.c`/MIT、`queue.h`/BSD、`units.c`/UIUC、`portable_endian.h`/Public Domain）。

BSD-3-Clause 对**二进制分发**的要求是「随附版权声明与免责声明」——
`third_party_notices` 中的 `THIRD-PARTY-NOTICES-IPERF3.txt` 与上游 LICENSE
全文随包分发即满足。

### Cygwin 运行时：LGPL-3.0-or-later（含链接例外）

`cygwin1.dll` 按 [Cygwin 官方许可页](https://cygwin.com/licensing.html)：
Cygwin API 库（`winsup` 子目录）适用 **LGPL v3 or later**；
且版权方给出**链接例外**：

> As a special exception, the copyright holders of the Cygwin library grant you
> additional permission to link libcygwin.a, crt0.o, and gcrt0.o with independent
> modules to produce an executable, and to convey the resulting executable under
> terms of your choice, without any need to comply with the conditions of LGPLv3
> section 4.

**合规结论**：

1. `iperf3.exe` 是 Cygwin 链接产物，受上述**链接例外**保护，可按 BSD-3-Clause 分发。
2. `cygwin1.dll` 由本目录**原样（未修改）**随包提供，SECM 自身**不链接**它——
   属于「聚合分发」而非衍生作品，因此 SECM 的 MIT 许可不受影响。
3. LGPLv3 §4 要求随附许可全文（`LICENSE-LGPL-3.0.txt`）与 GPLv3 全文
   （`LICENSE-GPL-3.0.txt`，LGPLv3 是对 GPLv3 的附加许可，二者需同时提供），
   并说明可获取对应源码。源码获取方式见下方。
4. 未对 `cygwin1.dll` 做任何修改，因此无需履行「修改须声明」的义务。

### 源码可获取性（LGPLv3 义务）

- iperf3 源码：<https://github.com/esnet/iperf>（BSD-3-Clause）
- Cygwin 源码：<https://cygwin.com/git.html>（对应 3.6.7-1 版本）

随包许可全文位于 `src-tauri/resources/iperf3/`，并已写入 `tauri.conf.json`
的 `bundle.resources`，随安装包一并落地。

## 更新步骤

1. 从上述下载地址获取新的 `iperf-<版本>-win64.zip`（或更新的稳定版）。
2. 解压出 `iperf3.exe` 与 `cygwin1.dll`，覆盖本目录同名文件。
3. 重新生成校验和清单：

   ```powershell
   Get-FileHash .\iperf3.exe  -Algorithm SHA256
   Get-FileHash .\cygwin1.dll -Algorithm SHA256
   ```

   把两个哈希写入 `SHA256SUMS.txt`（格式 `<hash>  <filename>`）。
4. 若上游 LICENSE 有更新，同步替换 `LICENSE-IPERF3-BSD-3-Clause.txt`。
5. 运行 `npm run stage:iperf3` 验证（校验通过才会复制）。
6. 更新本文件的「来源」表与 `THIRD-PARTY-NOTICES-IPERF3.txt` 中的版本号。

## 与仓库既有第三方分发的一致性

本目录沿用 `third_party/PawnIO/` 的范式：

- 原始产物入库（`third_party/<name>/`），**stage 产物不入库**
  （`src-tauri/resources/iperf3/` 已加入 `.gitignore`）；
- 许可全文随原始产物一起入库，并随包分发；
- stage 脚本做**完整性校验后**才复制，缺失即失败退出
  （不产出"残废"安装包）。
