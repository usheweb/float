# 唐三浪悬浮窗 —— 项目规则

## 一、协作方式（用户明确要求）

- **不做自动化测试。** 不要新增、执行单元 / 集成 / E2E 测试，也不要用浏览器自动化或子代理做界面验收。
- 功能完成后由**用户人工验证**；允许的检查仅限编译级 / 语法级：`node --check`、`cargo check`、`npm run tauri:build`。
- 不要为了「验证」在业务代码里加临时条目、开关或测试专用分支。

## 二、工程约定

- Tauri 2 桌面悬浮窗，前端为纯静态 HTML/JS（`ui/`，无构建步骤），Rust 后端在 `src-tauri/`。
- **`tauri.conf.json` 的 `frontendDist` 指向 `../ui`，静态资源会被编译进二进制**：改完 `ui/` 里的 JS / CSS，必须重新 `npm run tauri:build` 才会生效，`tauri:dev` 模式下才会热加载。
- 前端脚本 / 样式拆分：`ui/main.js`（主逻辑）、`ui/util.js`（公共工具，挂 `window.AFP_UTIL`）、`ui/sources/*.js`（各数据源渲染模块，IIFE 自注册）、`ui/tooltip.html`（独立提示窗，样式写在页内）。
- 版本号必须四处同步：`package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`（`name = "tangsanlang-float"` 的下一行）。设置面板显示的是 `env!("CARGO_PKG_VERSION")`，取自 **Cargo.toml**，漏改会导致安装包自报旧版本。
- 数据刷新失败必须保留上一次成功的数据（内存 + localStorage 双份），不得清空成占位空卡；仅从未成功过的数据源才显示占位文案。
- 提交统一署名：作者「唐三浪」，邮箱 `usheweb@users.noreply.github.com`；禁止提交含密钥的本地文件。

## 三、UI 视觉规范（对齐 dbmanx / clean-c）

视觉语言与 `dbmanx`（`dbman-tauri/src/assets/styles/theme.css`）、`clean-c`（`src/styles/tokens.css` + `global.css`）保持同一套：
**玻璃拟态面板 + 径向光晕 + 42px 网格底纹 + 青色主强调 + 紫色次强调**。

**默认亮色**（浅灰蓝底、青绿主色，与 clean-c / dbmanx 一致）；**暗色是用户手动切换项**（深空暗底 + 霓虹青），不是默认值。主题切换入口在设置面板「数据源」列表底部的「外观主题」行。

### 1. 颜色令牌

全部定义在 `ui/styles.css` 的 `:root`（亮色）与 `:root[data-theme="dark"]`（暗色），**组件里禁止写死色值**（含 `ui/sources/*.js` 里动态拼接的颜色，必须写成 `var(--xxx)`）。

| 令牌 | 亮色（默认） | 暗色 | 用途 |
| --- | --- | --- | --- |
| `--bg-0` | `#eef2f8` | `#04070f` | 页面底色 |
| `--bg-1` | `#e2e9f3` | `#0a1020` | 次级背景 |
| `--panel` | `rgba(255,255,255,.72)` | `rgba(13,21,38,.66)` | 玻璃拟态面板 |
| `--panel-solid` | `#ffffff` | `#0d1526` | 不透明面板 |
| `--panel-rgb` | `255, 255, 255` | `13, 21, 38` | 顶栏等半透明叠层通道 |
| `--ink-rgb` / `--ink-hi-rgb` | `244,247,252` / `255,255,255` | `4,7,15` / `16,25,45` | 墨色通道（弹窗、悬浮卡渐变、输入框底） |
| `--scrim` | `rgba(15,23,42,.4)` | `rgba(2,5,12,.72)` | 弹窗遮罩 |
| `--line` | `rgba(0,141,168,.22)` | `rgba(0,214,255,.14)` | 常规描边 |
| `--line-strong` | `rgba(0,141,168,.5)` | `rgba(0,214,255,.45)` | 强调描边 / 聚焦 |
| `--cyan` | `#008da8` | `#00e5ff` | 主强调 |
| `--violet` | `#6b4ee6` | `#8b6cff` | 次强调 |
| `--magenta` | `#d62266` | `#ff3d81` | 错误 / 危险 / 已用过高 |
| `--green` | `#0a9e6e` | `#25f4b0` | 正常 / 成功 |
| `--yellow` | `#9a6700` | `#ffd166` | 警示 / 即将到期 |
| `--text` | `#16283d` | `#cfe6f5` | 正文 |
| `--text-dim` | `#5a6f86` | `#6c839c` | 次要文字 |
| `--text-faint` | `#8493a5` | `#5b7187` | 弱化文字 |
| `--on-accent` | `#04252e` | `#eafcff` | 强调底上的文字 |
| `--ring-track` | `rgba(0,141,168,.18)` | `rgba(0,229,255,.12)` | 圆环轨道 |
| `--glow-cyan` / `--glow-violet` / `--glow-danger` | 8/20px 低透明 | 10/26px 高透明 | 三档发光 |

每个强调色都带 `--*-rgb` 通道变量（`--cyan-rgb` 等），半透明色一律写 `rgba(var(--cyan-rgb), .12)`，不要再复制 RGB 字面量。

- 亮色的强调色是**加深版**（青 `#008da8` 而非 `#00e5ff`），浅色底上用高亮霓虹会看不清。
- `--ring-track`、`--glow-*` 必须按主题各给一份，不能共用一套。

### 1.1 主题切换实现

| 项 | 约定 |
| --- | --- |
| 默认值 | 亮色。`ui/theme.js` 读不到配置时一律 `light` |
| 切换方式 | `<html data-theme="dark">`；暗色规则写在 `:root[data-theme="dark"]` 覆盖 `:root` |
| 持久化 | `localStorage`，key 固定 `afp:theme`（只存 `light` / `dark`） |
| 首屏防闪 | `theme.js` 在 `<link rel="stylesheet">` **之前**同步加载，先定主题再渲染 |
| 提示窗同步 | 提示窗是独立窗口，CSS 不会跟着主窗口变；主窗口切换后 `emit("theme-change")`，`tooltip.js` 收到后改自己的 `data-theme` |
| 主题令牌 | 两个窗口各写一份，但取值必须一致（`tooltip.html` 的 `--tip-*` 跟随 `ui/styles.css`） |

### 2. 字体与中文排版

- `--font-ui`：`PingFang SC / HarmonyOS Sans SC / Microsoft YaHei` 中文优先回退链，界面文字一律用它。
- `--font-mono`：`JetBrains Mono / SF Mono / Consolas`，仅用于密钥、金额、时间、百分比等数字列。
- **不使用 Orbitron 等无中文字形的展示字体**；悬浮窗文字小（11~14px），中文必须优先。
- 中文禁止 `text-transform: uppercase`；`letter-spacing` 不超过 `1px`（现有 `.brand-text h1` 为 `0.6px`）。
- 科技感通过颜色、发光、字重、渐变体现，不依赖大写字距。

### 3. 背景质感

- `.bg-glow` = 两层径向光晕 + 底色：左上青 `rgba(var(--cyan-rgb), .12)`、右下紫 `rgba(var(--violet-rgb), .14)`，底色 `linear-gradient(180deg, var(--bg-1), var(--bg-0) 62%)`。
- `.bg-glow::before` 铺 42px×42px 双向网格（`rgba(var(--cyan-rgb), .05)`），用 `radial-gradient(circle at 50% 40%, #000 30%, transparent 85%)` 做 mask 从中心淡出；`position: fixed` + `pointer-events: none`。
- 滚动条滑块用青→紫 45% 透明度渐变，悬停变实色；轨道透明。

### 4. 面板、圆角与发光

- 卡片 / 弹窗 / 悬浮卡一律 `background: var(--panel)`（或 `--ink-*` 渐变）+ `backdrop-filter: blur(14px)`（保留 `-webkit-` 前缀）+ `1px solid var(--line)`，不用不透明死黑块。
- 圆角收紧：`--r-sm: 6px`、`--r: 8px`、`--r-md: 10px`、`--r-lg: 14px`。卡片用 `--r-md`，输入框 / 小按钮用 `--r-sm`，弹窗 12px，`.float-card` 因是窄条保留 20px 胶囊。
- 弹窗用 `--line-strong` 描边 + 青色内外阴影 + `--scrim` 遮罩（`blur(6px)`）；悬浮卡用 `--ink-*` 渐变 + **inset 青色投影**。
- **悬浮卡禁止用外投影**：窗口尺寸正好等于卡片尺寸，`box-shadow` 落在窗口外的部分会被裁掉，只在四角圆角切口露出灰青色边缘，亮色主题下看着像方角（提示窗 v0.2.3、悬浮卡 v0.3.16 各踩过一次）。
- 发光统一走 `--glow-cyan` / `--glow-violet` / `--glow-danger`，不要临时拼 `box-shadow`。
- 交互过渡统一 `var(--transition)`（`160ms cubic-bezier(.4,0,.2,1)`），按压用 `scale(.96)`，悬停用发光 + 描边加强，不做大幅位移。

### 5. 按钮与选中态

- 次要按钮（`.ghost-btn`、`.refresh-btn`、`.icon-btn`）：`rgba(var(--cyan-rgb), .04)` 底 + `var(--line)` 描边，悬停转青字 + `--line-strong` + `--glow-cyan`。
- 主按钮（`.primary-btn`）：**半透明青紫渐变底 + 青字 + `--line-strong` 描边**，悬停加浓到 32% 底、字变 `--on-accent`、加 `--glow-cyan`；不要用实心亮块。两套主题共用同一份规则，靠令牌换色。
- 危险操作（关闭、删除、失败）：品红 `--magenta` + `--glow-danger`，不用纯红。
- Tab / 列表选中态用「青紫渐变底 + 描边 + `0 0 22px -8px` 投影」，不用实心色块。
- 品牌名 / 区块标题可用青紫 `background-clip: text` 渐变 + `drop-shadow` 发光。

### 6. 数据可视化配色

- 环形进度 `ringColor()`（`ui/util.js`）：<30% 绿、30~60% 青、60~85% 黄、≥85% 品红，返回 `var(--*)` 字符串。
- 到期紧迫度 `urgencyColor()`（`ui/sources/aliyun.js`）：≤7 天品红、≤30 天黄、其余绿、未知 `--text-dim`。
- 动态颜色通过内联 `style="color:${color}"` / `style="stroke:${color}"` 注入（不可用 SVG presentation attribute 直接写 `var()`），兜底值同样用变量。
- 环底色走 `--ring-track`（亮色 `.18`、暗色 `.12`），不做灰白描边。

### 7. 实现约束

1. `.bg-glow` 必须留在最底层（`z-index: 0`），内容层用 `position: relative; z-index: 1`；顶栏 / Tab 条 `z-index: 2`。
2. 保留 `-webkit-` 前缀写法（`backdrop-filter`、`background-clip`、`mask-image`）。
3. 改 UI 后必须重新 build 并在真机上看过（悬浮窗只有 61px 高的紧凑态，任何内边距 / 字号调整都要实际确认不截断）；**先在默认亮色下验收，再切暗色复核一遍**，禁止只按一套主题调色。
4. 新增数据源模块时，颜色只能引用既有令牌，不新增色板；确需新色先在两套主题里各加一份。
5. 提示窗 `ui/tooltip.html` 是独立窗口，令牌单独定义在页内 `:root` / `:root[data-theme="dark"]`，取值必须与主窗口一致（亮色白底深字、暗色深空底浅字，标题分别为 `#008da8` / `#00e5ff`）。
6. 禁止在组件里按 `[data-theme="dark"]` 写局部覆盖来做「暗色专属修正」；需要区分时先抽成令牌。
