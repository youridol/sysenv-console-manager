# 变更契约 v2：日志栏拉伸 + 按钮样式 + 滚动条 + 授权持久化

## A. 日志侧边栏拉伸（前端）

- `RightLogPanel` 左边缘新增**拖拽手柄**（`role="separator"`、`aria-orientation="vertical"`、
  `aria-label="调整日志面板宽度"`、`tabIndex=0`），支持鼠标拖拽与键盘（←/→，步长 16px）。
- 宽度约束：**最小 16rem（256px）**，**最大 `min(40rem, 50vw)`**；默认保持现状 22rem。
- 持久化：`localStorage` 键 `secm:log-panel-width`（存像素整数字符串）。
  非法/越界值一律 clamp 到范围内；缺失则用 22rem。
- 折叠态不显示手柄（宽度 `w-10` 固定）；展开态手柄宽 4px，hover/拖拽中高亮。
- 拖拽期间禁用过渡（`transition-[width]` 会让拖拽延迟跟手）；拖拽结束恢复过渡。
- 拖拽中给 `document.body` 加 `cursor-col-resize select-none`，结束时移除。

## B. 移除输出模式按钮的背景/描边（前端）

`LogsCard` 里「全部输出 | 按页输出」`ToggleGroup` 当前用 `variant="outline"`，
会渲染边框与选中态背景。要求：**去掉描边与选中背景**，保留"滑动指示器"
（已有的绝对定位 `<span>` 滑块）+ 选中文字高亮，视觉上像纯文字分段切换。

实现要求：
- `ToggleGroup` 传 `variant="ghost"`（或等效无边框变体），并显式清除
  `data-[spacing=0]:data-[variant=outline]:border-l-0`、`shadow-none`、`border` 等来源。
- `ToggleGroupItem` 用 `className` 覆盖掉由 `toggleVariants` 带入的
  `data-[state=on]:bg-*` / `border` 背景与描边（用 `bg-transparent`、
  `data-[state=on]:bg-transparent`、`border-0`、`hover:bg-transparent` 等）。
- 选中态仅靠：滑块背景 + `text-foreground font-medium`；未选中 `text-muted-foreground`。
- 滑块（既有 `<span>`）保留，作为唯一的"选中背景"来源。其圆角/内边距需与两段等宽对齐。
- **不得改变**功能：`type="single"`、值 `all`/`page`、`localStorage["secm:log-scope-mode"]`、
  `aria-label="日志输出模式"` 均保持不变。

## C. 全局隐藏滚动条（前端，仅 `src/index.css`）

要求：滚动条不可见，但**滚动功能完全不受影响**（滚轮/触控板/键盘/拖拽滚动条区域仍工作，
`overflow` 计算与滚动容器高度不得变化）。

- 顶部（tailwind 导入之后）加全局规则：
  ```css
  * { scrollbar-width: none; -ms-overflow-style: none; }
  *::-webkit-scrollbar { width: 0; height: 0; display: none; }
  ```
- 删掉/覆盖既有 `::-webkit-scrollbar{width:6px}`、`::-webkit-scrollbar-thumb{...}`、
  `.dark ::-webkit-scrollbar-thumb{...}` 三处（它们会让滚动条重新出现）。
- `.proc-scrollbar` 的显式可见样式（8px + 带底色轨道）与"全局隐藏"冲突：
  **按全局要求一并隐藏**（删除该块或加注释说明已被全局规则取代），并在代码注释中写明
  「如需恢复该列表的常驻可见滚动条，移除全局规则或在此块显式 `display:block`」。
- 不要用 `overflow: hidden` 实现"隐藏"——那会真正禁用滚动。必须是滚动条不可见而非不可滚。

## D. 授权持久化 + 管理员环境自动启用（后端 C#，核心）

### D.1 目标
1. **管理员环境下零点击**：SECM/sidecar 若本身以管理员运行，tier-2 应在启动时**自动**启用，
   用户无需点「授权并启用」。
2. **一次授权，冷启动不再弹 UAC**：用户授权一次后，之后每次冷启动自动获得 tier-2，不再需要
   任何点击、也不再弹 UAC。
3. 非管理员且从未授权过：行为不变（tier-1 正常，tier-2 需用户点击一次）。

### D.2 机制（Windows 计划任务）
标准免 UAC 提权方式：注册一个 `RunLevel=HighestAvailable` 的计划任务；任务一旦由管理员
创建成功，之后由同一用户 `schtasks /Run` 触发**不会弹 UAC**。

- 任务名常量：`SECM-LhmTier2`（冻结）。
- 任务动作：`"<当前 LhmSidecar.exe 绝对路径>" --port <port+1> --interval <ms> --role=elevated`
  （与现有一致：伴生进程监听 `port+1`）。
- 注册时机：**提权伴生进程自身启动时**（它已经是管理员，注册无需再弹 UAC），
  使用 `schtasks /Create /TN SECM-LhmTier2 /TR "<cmd>" /SC ONCE /ST 00:00 /RL HIGHEST /F`
  （`/F` 幂等覆盖；`/SC ONCE` + 立即 `/Run` 触发，不使用常驻触发器）。
- 冷启动路径：`RequestElevation()` 先尝试 `schtasks /Run /TN SECM-LhmTier2`；
  若任务不存在或未在**有界时间内**（≤8s）产出 tier-2，则回退到现有 `runas` 伴生路径（会弹 UAC）。
- **自动尝试**：sidecar 启动、tier-1 就绪后，在后台线程**自动**尝试一次任务触发
  （仅当任务已存在时；不存在则静默跳过，**绝不**因自动尝试而弹 UAC）。
  这样"授权一次后冷启动"完全无需用户操作。
- 撤销：新增 `POST /api/v1/elevate/revoke` → `schtasks /Delete /TN SECM-LhmTier2 /F`
  + 复位 `_elevatedRole`/`_elevationRefused`/`_companionPid`，返回
  `{"revoked":true,"reason":"..."}`。用于用户卸载/收回持久授权。

### D.3 安全与失败语义
- **绝不**在未授权时自动 `runas`（自动路径只允许"已存在的计划任务"，不得弹 UAC）。
- 任务注册失败（组策略禁用计划任务、权限不足等）→ 仅记日志，回退到按需 `runas`，
  功能不劣化。
- 不写入任何明文密码/凭据（任务以当前用户上下文 + HIGHEST 运行，不用 `/RU`/`/RP`）。
- 所有 `schtasks` 调用必须**隐藏窗口**（`CREATE_NO_WINDOW` 等效 / `ProcessWindowStyle.Hidden`）
  且带超时，失败静默降级。

### D.4 接口（冻结）
- `POST /api/v1/elevate` 响应体保持 `{"elevated":bool,"reason":string}`（语义扩展：
  可能通过计划任务生效，reason 文案需说明来源）。
- `POST /api/v1/elevate/revoke` → `{"revoked":bool,"reason":string}`。
- `GET /api/v1/privilege` 新增可选字段 `persistent: bool`（是否已注册持久任务）。
  Rust 侧 `LhmSensorResponse` 保持 `#[serde(default)]`，缺字段不 panic。

### D.5 Rust 侧
- 无需新增 Tauri 命令（用户未要求撤销 UI）。`/api/v1/privilege` 透传即可。
- 若 `LhmSensorResponse` 想暴露 `persistent`，必须 `#[serde(default)]` 且不影响既有字段。

## E. 验收
1. `npm run build` / `npm run lint` 退出 0。
2. `cd sidecar-lhm; dotnet build -c Release` 与 `dotnet test tests/LhmSidecar.Tests` 全绿。
3. `cd src-tauri; cargo check --lib --tests` 退出 0；lib 单测与 4 个集成测试全绿。
4. 运行态：
   - 右侧栏可拖拽改变宽度，刷新后保持；折叠态无手柄。
   - 输出模式按钮无描边、无选中背景块，仅文字 + 滑块。
   - 全局无可见滚动条，但各页面仍可正常滚动（滚轮/键盘）。
   - 管理员启动：**不出现**授权卡（tier-2 自动就绪）。
   - 普通用户授权一次后：`schtasks /Query /TN SECM-LhmTier2` 存在；重启应用后 tier-2
     自动可用且**未弹出 UAC**。
