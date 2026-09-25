# 变更契约 v3：硬件信息页加载流程 + 授权后 UI 残留

## 根因（已在发布版实测确认）

### 根因 1 — 加载期数据被整份丢弃（对应"优先加载所有信息流程而不是等待授权"）

实测冷启动时间线（`%APPDATA%\SECM\logs\<date>.log`，逐步毫秒）：

| 时刻 | 现象 |
| --- | --- |
| +0.35s | 应用启动，托盘就绪 |
| +4.47s | 首次 `get_sensor_data`：`GPU=none` `freq=0MHz(none)` `temp=0.0°C(none)`，diag=`lhm(不可用:正在进行首次采集（tier-1 性能计数器预热中）)` |
| +6.42s | 仍 `GPU=none` |
| +7.43s | **GPU 首次出现**、`freq=4648MHz(pdh)` |
| +12.4s | `temp=75.4°C(lhm)`（tier-2 生效） |

**机制**：`SnapshotBuilder` 的 `Available` 要求"至少一个有效指标"
（`Cpu.LoadPercent` / `ClockMhz` / gpu / disk / mem / `PackageTempC`）。
tier-1 PDH 预热需要两次采样（约 2.5–4.4s），预热期间所有指标都是 null →
sidecar 返回 `available:false` → `lhm.rs::lhm_snapshot()` 走到
**`else` 分支整份丢弃**（`lhm.rs:707-712`）→ `sensor.rs` 拿到 `snap=None` →
`gpu: Vec::new()`、`motherboard=None`。

**关键缺陷**：该分支丢弃了 payload 中**已经有效**的字段（例如 GPU 温度/功耗
本应经厂商用户态通道立即可用）。`available` 是"是否有任一指标"的粗粒度门控，
**不应**用作"是否丢弃整份快照"的判据。契约 v3 的每个字段都是
`Option`/空数组安全（`#[serde(default)]`），前端对缺失值已有 "—" 兜底。

### 根因 2 — 授权后 CPU 卡片残留图标与横线（对应"多出两个图片和横线…切回才消失"）

`Dashboard.tsx:492` 的 `SummaryCard` 用 **`key={s.text}`** 渲染 stats。
未授权时 CPU 卡三项文本为 `"—"` / `"4.4 GHz"` / `"—"` → **key 重复**
（两个 `"—"`）。授权完成后文本变为 `"75.4°C"` / `"4.4 GHz"` / `"35.2W"` →
key 集合在同一挂载实例内由"重复"变为"唯一"，React 协调（reconciliation）
在此场景会新增节点而保留部分旧节点，表现为卡片里多出图标与横线；
**只有重新挂载（切换页面再切回）才复位** —— 这正是用户观察到的症状签名。

同一类缺陷还有 `key={d.name}`（磁盘卡）；实测"存储空间"名称可重复
（`hardware_contract` 页曾出现两张 `Storage Space`）。

## A. 后端修复（`src-tauri/src/lhm.rs`，owner: rust-lhm）

**A.1 不再因 `available:false` 丢弃整份快照。**

`lhm_snapshot()` 的 `Ok(resp)` 分支改为：**只要 HTTP 调用成功就返回该快照**，
`available` 仅用于诊断文案，不再决定是否丢弃。要求：
- `resp` 依旧缓存进 `last_snapshot`（供 `lhm_cpu_temp()` 复用）——
  但注意 `lhm_cpu_temp` 自身仍以 `t > 0.0` 过滤，故无温度时不会伪造。
- `diag` 保持现状语义：`available:true` → `cpu_temp_diag(&resp)`；
  `available:false` → `lhm(不可用:<error>)`（把原因透出），
  但**返回值仍是 `Some(resp)`**。
- `Err(e)` 分支（HTTP 失败/崩溃/降级）**行为不变**，仍返回 `None`。
- 更新模块顶部文档注释（`lhm.rs:25` 附近）中"available:false → 返回 None"的描述。
- 更新受影响的既有单测：原断言 `available:false → None` 的用例改为断言
  "返回 `Some` 且 `diag` 含原因"；并新增一个用例证明
  "`available:false` 但 payload 含有效 GPU 时，GPU 数据仍透出"
  （这是本次修复的核心回归守卫）。

**A.2 不要卡在授权上。** 不改动；`AutoAuthorizeLoop` 已是独立后台线程
（`Program.cs:617`），无需调整。

## B. 前端修复（`src/pages/Dashboard.tsx`，owner: Lead）

**B.1 唯一 key（消除残留）。**
- `key={s.text}` → `key={`${i}-${s.text}`}`（stats 顺序固定，索引稳定）。
- `key={d.name}` → `key={`${d.name}-${i}`}`。
- 顺带加固：`key={s.name}`（主板传感器）→ `key={`${s.name}-${i}`}`。
- 不改其它列表 key（`key={item.label}` / `key={g.kind}` 语义唯一）。

**B.2 加载流程过渡（不再直接显示卡片容器）。**
- 引入"初始加载阶段"判定：`loading` 状态在**首个采样到达且必要数据就绪**后才结束。
  就绪判据：`data !== null` **且**（`data.gpu.length > 0` 或 已过 `BOOTSTRAP_GRACE_MS`）。
  必须设**有界兜底**（`BOOTSTRAP_GRACE_MS = 8000`），
  保证无 GPU / 采集异常环境下也必定结束加载、不永久骨架。
- 加载期渲染**骨架屏**（沿用现有 `Skeleton` 网格）并给出"正在加载硬件信息…"文案，
  **不渲染** CPU/GPU/内存/主板/趋势/存储/网络等卡片容器。
- 新增**过渡动画**：骨架 → 内容的切换使用既有的 `animate-fade-in`
  （已存在的动画工具类），避免"突兀出现"。

**B.3 授权/降级提示卡延后到加载完成之后再判定（"优先加载所有信息流程而不是等待授权"）。**
- 在加载阶段内**不渲染** `Tier2PrivilegeCard` / `DriverGuidanceCard`，
  避免预热期 `tier2_available=false` 导致授权卡先闪出来、让用户以为页面在等授权。
- 加载完成后按现有逻辑判定（tier-2 可用则完全不显示）。
- `FrequencyDiagCard` 同样延后到加载完成后判定。

**B.4 保持既有契约不变**：`SensorData` 字段名、`tier2_available`/`tier2_reason`/
`elevated` 语义、`SummaryCard` 的"—"兜底、错误态整页 `Alert` 行为均不变。

## C. 验收
1. `npm run build` / `npm run lint` 退出 0。
2. `cd src-tauri; cargo check --lib --tests` 退出 0；lib 单测与 4 个集成测试全绿。
3. 运行态（发布版）：
   - 冷启动立即显示骨架 + "正在加载硬件信息…"，**不出现**卡片容器、**不出现**授权卡。
   - 未授权环境下，GPU 温度/功耗、内存、磁盘、网络随首批数据一起出现
     （不再出现 3–4 秒的 `GPU=none` 窗口）。
   - 加载完成后，若 tier-2 不可用才显示授权卡。
   - 点击「授权并启用」→ 温度/功耗出现后，CPU 卡**不出现**多余图标或横线，
     且**无需切换页面**即保持正确。
