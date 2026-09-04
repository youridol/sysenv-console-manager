# third_party/PawnIO —— 随包分发来源证据

本目录存放 SECM 随包分发的 **PawnIO 2.2.0 驱动**的全部来源材料与校验依据，供审计追溯。

## 一、来源构成（如实记录）

| 文件 | 来源 | 说明 |
|------|------|------|
| `PawnIO.sys` / `PawnIO.cat` / `pawnio.inf` | **本机 DriverStore 副本**：`C:/Windows/System32/DriverStore/FileRepository/pawnio.inf_amd64_*/` | 官方 2.2.0 安装器在本机安装的产物，**WHQL 签名有效**（Microsoft Windows Hardware Compatibility Publisher） |
| `PawnIO-src/` | 官方源码仓库 **`namazso/PawnIO`** tag `2.2.0`（源码，含 PawnIO 驱动 / PawnIOLib / PawnIOUtil / PawnPP 依赖声明 / COPYING（GPL-2.0 全文）） | GPL-2.0 源码提供义务随包承载 |

## 二、三件套身份验证（本机副本 = 官方 2.2.0 的证明链）

1. **INF 版本**：`pawnio.inf` DriverVer=`03/11/2026,2.2.0.0`、Provider=`namazso`、CatalogFile=`PawnIO.cat`、
   PnpLockdown=1（与官方 `PawnIO/PawnIO.inf.in` 模板一致，源码内核对）。
2. **驱动版本**：`PawnIO.sys` FileVersion=`2.2.0`、CompanyName=`namazso`。
3. **WHQL 签名**：`PawnIO.sys` / `PawnIO.cat` Authenticode 验证通过
   （Microsoft Windows Hardware Compatibility Publisher）。
4. 版本三重印证（INF 版本 + 驱动版本 + WHQL 签名）均指向官方 2.2.0。

## 三、诚实性说明（不伪称）

- 官方源码仓库 tag 2.2.0 不含预编译二进制资产；随包分发的三件套以本机 DriverStore
  副本（官方 2.2.0 安装产物，WHQL 可验证）为来源，哈希以本目录 `SHA256SUMS.txt` 为准。
- 源码目录内的 `.gitmodules`（PawnPP 子模块）未随源码带入（子模块需自行 clone），
  已在发行树中移除该悬空引用；PawnPP 仅为构建期工具依赖，不影响随包源码完整性。

## 四、随包使用方式

- `scripts/stage-driver.mjs` 把三件套复制到 `src-tauri/resources/driver/`（随安装包分发）；
  `PawnIO-src/` 源码随包至 `src-tauri/resources/driver/PawnIO-src/`（GPL-2.0 源码提供义务）。
- 运行时由 `src-tauri/src/driver_install/pawnio.rs` 以 SCM 服务方式部署 `PawnIO.sys`
  到 `%windir%\System32\drivers\` 并启动；已有官方服务时复用。
- 许可：驱动为 **GPL-2.0-or-later + 特殊例外**（例外全文见
  `sidecar-lhm/licenses/LICENSE-GPL-2.0-PawnIO.txt`）。

## 五、文件哈希

见 `SHA256SUMS.txt`（三件套 SHA-256）。
