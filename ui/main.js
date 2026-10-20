const { invoke } = window.__TAURI__.core;

// Windows 使用系统原生标题栏，去掉为 macOS 红绿灯预留的左侧内边距
if (/Windows/i.test(navigator.userAgent)) {
  document.body.classList.add("is-windows");
}

// 自动刷新间隔按场景各自配置（E-02）：后端清单下发前先用默认值兜底
let defaultInterval = 60;
let minInterval = 5;
let maxInterval = 3600;
let refreshInterval = defaultInterval;
let autoTimer = null;

// 悬浮窗圆环
const FR = 20;
const FCIRC = 2 * Math.PI * FR;

const el = {
  cards: document.getElementById("cards"),
  badge: document.getElementById("plan-badge"),
  statusDot: document.getElementById("status-dot"),
  statusText: document.getElementById("status-text"),
  countdown: document.getElementById("countdown"),
  connectSm: document.getElementById("connect-sm"),
  error: document.getElementById("error-banner"),
  refreshBtn: document.getElementById("refresh-btn"),
  refreshText: document.getElementById("refresh-text"),
  subtitle: document.getElementById("subtitle"),
  pinBtn: document.getElementById("pin-btn"),
  tabs: document.getElementById("scenario-tabs"),

  floatRing: document.getElementById("float-ring"),
  floatCard: document.getElementById("float-card"),
  floatLabel: document.getElementById("float-label"),
  floatClose: document.getElementById("float-close"),
  floatRemain: document.getElementById("float-remain"),
  floatUnit: document.getElementById("float-unit"),
  floatSub: document.getElementById("float-sub"),
  floatReset: document.getElementById("float-reset"),
  floatRingFg: document.getElementById("float-ring-fg"),
  floatRingPct: document.getElementById("float-ring-pct"),

  settingsBtn: document.getElementById("settings-btn"),
  settingsModal: document.getElementById("settings-modal"),
  settingsBackdrop: document.getElementById("settings-backdrop"),
  settingsTitle: document.getElementById("settings-title"),
  settingsClose: document.getElementById("settings-close"),
  settingsMsg: document.getElementById("settings-msg"),
  storageDesc: document.getElementById("storage-desc"),
  listView: document.getElementById("settings-list-view"),
  detailView: document.getElementById("settings-detail-view"),
  sourceList: document.getElementById("source-list"),
  detailDesc: document.getElementById("detail-desc"),
  detailInfo: document.getElementById("detail-info"),
  accountList: document.getElementById("account-list"),
  accountAdd: document.getElementById("account-add"),
  accountForm: document.getElementById("account-form"),
  detailLabel: document.getElementById("detail-label"),
  detailFields: document.getElementById("detail-fields"),
  detailInterval: document.getElementById("detail-interval"),
  detailAutoInterval: document.getElementById("detail-auto-interval"),
  detailAutoIntervalInput: document.getElementById("detail-auto-interval-input"),
  detailBack: document.getElementById("detail-back"),
  detailCancel: document.getElementById("detail-cancel"),
  detailTest: document.getElementById("detail-test"),
  detailSave: document.getElementById("detail-save"),
  appVersion: document.getElementById("app-version"),
};

let loading = false;
/// 当前场景交给悬浮窗的文案；为 null 表示该场景没有可展示的悬浮信息
let lastFloat = null;

/// 每个账号「上一次成功渲染的结果」（按账号 id 分开存，切 tab 不会串台）。
/// 取数失败时不把已有的数据换成空占位，改用这里的旧结果顶上；同时落一份到
/// localStorage，这样冷启动时第一次取数就失败，也还能看到上一次的数据。
const LAST_GOOD_KEY = "afp:lastGood";

let lastGoodViews = (() => {
  try {
    const raw = JSON.parse(localStorage.getItem(LAST_GOOD_KEY) || "{}");
    return raw && typeof raw === "object" ? raw : {};
  } catch (_) {
    return {};
  }
})();

function saveLastGood() {
  try {
    localStorage.setItem(LAST_GOOD_KEY, JSON.stringify(lastGoodViews));
  } catch (_) {
    /* 落盘失败不影响本次展示，内存里这份还在 */
  }
}

/// 渲染结果里是否含真实内容：空占位（class="empty"）不算
function hasRealData(view) {
  return !!view && !!view.html && !/class="empty"/.test(view.html);
}

/// 取某账号缓存的上一次成功结果，兼容 v0.2.x 时代按「平台 id」存的旧缓存：
/// 多账号后账号 id 变成 "volcengine:1"，旧 key 却是 "volcengine"，不迁一下就永远对不上。
function cachedViewFor(id) {
  if (!id) return null;
  if (lastGoodViews[id]) return lastGoodViews[id];
  const srcId = entryById(id)?.source.id || (id.includes(":") ? id.slice(0, id.indexOf(":")) : "");
  if (srcId && srcId !== id && lastGoodViews[srcId]) {
    // 顺手把旧 key 搬到账号 id 下，之后走正常路径就能直接命中
    lastGoodViews[id] = lastGoodViews[srcId];
    delete lastGoodViews[srcId];
    saveLastGood();
    return lastGoodViews[id];
  }
  return null;
}

/// 把某个源上一次的成功结果铺到主视图与悬浮卡上；没有就返回 false
function applyCachedView(view) {
  if (!view) return false;
  el.cards.innerHTML = view.html;
  el.badge.textContent = view.badge || "—";
  lastFloat = view.float || null;
  updateFloat();
  return true;
}

/// 后端下发的平台清单（每个平台内含它自己的账号列表）
let catalog = [];
/// 当前正在展示的账号 id（形如 aliyun:1）；trae 这类自动凭证源就是 "trae"
let activeAccount = null;
/// 凭证存放方式（后端下发）：keychain = 系统钥匙串，file = 明文文件兜底
let credStorage = "keychain";
let credPath = "";
/// 设置里当前打开的平台（只存 id，取最新对象用 currentDef()）
let currentSource = null;
/// 设置里正在编辑的账号 id；null 表示「新增账号」
let editingAccountId = null;
/// 编辑表单被锁定（该账号已保存凭证但本次运行尚未连接）：隐藏保存 / 测试，避免覆盖旧密钥
let formLocked = false;

/// 当前展示的账号是否为「未解锁」的钥匙串账号（决定悬浮窗是否显示待授权入口）
let lockedScene = false;

/// 把清单里所有平台的账号拍平成一个列表：[{ account, source }]
function accountEntries() {
  const out = [];
  for (const s of catalog) for (const a of s.accounts || []) out.push({ account: a, source: s });
  return out;
}

/// 按账号 id 找到它所属的平台
function entryById(id) {
  if (!id) return null;
  return accountEntries().find((e) => e.account.id === id) || null;
}

/// 账号在 Tab / 列表上的显示名：自动凭证源用平台名，其余用「平台名 · 账号名」
function entryLabel(e) {
  if (!e) return "—";
  if (e.source.autoCreds) return e.source.name;
  const label = (e.account.label || "").trim() || "默认账号";
  return `${e.source.name} · ${label}`;
}

/// 设置里当前打开的平台定义（用 id 重新解析，避免拿到 reloadCatalog 前的旧对象）
function currentDef() {
  if (!currentSource) return null;
  return catalog.find((s) => s.id === currentSource.id) || null;
}

/// 某账号本次运行是否可立即取数：auto 源看「本机登录态可用」，钥匙串账号看「本次是否已连接」
function accountReady(id) {
  return entryById(id)?.account.ready === true;
}

/// 当前账号是否可立即取数（未连接时既不取数、也不启动自动刷新）
function activeReady() {
  return !!activeAccount && accountReady(activeAccount);
}

/* ---------- 通用 ---------- */

function setStatus(kind, text) {
  el.statusDot.className = "status-dot" + (kind ? " " + kind : "");
  el.statusText.textContent = text;
}

function showError(msg) {
  el.error.textContent = msg;
  el.error.classList.remove("hidden");
  fitWindowHeight();
}

function hideError() {
  el.error.classList.add("hidden");
  fitWindowHeight();
}

/* ---------- 悬浮窗 ---------- */

function updateFloat() {
  renderFloat();
  // 悬浮卡片随内容收缩宽度，短内容（如只有几位积分的 Trae）就不会在右侧空出一大块
  fitFloatWidth();
}

function renderFloat() {
  const fg = el.floatRingFg;
  if (!fg) return;

  // 锁定态钥匙串源：悬浮窗显示「待授权」，并把副标题做成可点击的解锁入口
  if (lockedScene) {
    el.floatRing.classList.add("hidden");
    el.floatLabel.textContent = "连接账号";
    el.floatReset.textContent = "";
    el.floatRemain.textContent = "待授权";
    el.floatRemain.style.color = "";
    el.floatUnit.textContent = "";
    el.floatSub.textContent = "点击此处解锁";
    el.floatSub.classList.add("float-action");
    return;
  }
  el.floatSub.classList.remove("float-action");

  const f = lastFloat;
  if (!f) {
    el.floatRing.classList.add("hidden");
    el.floatLabel.textContent = "—";
    el.floatReset.textContent = "—";
    el.floatRemain.textContent = "—";
    el.floatRemain.style.color = "";
    el.floatUnit.textContent = "";
    el.floatSub.textContent = "暂无数据";
    return;
  }

  el.floatLabel.textContent = f.label || "";
  el.floatReset.textContent = f.reset || "";
  el.floatRemain.textContent = f.remain ?? "—";
  el.floatRemain.style.color = f.color || "";
  el.floatUnit.textContent = f.unit || "";
  el.floatSub.textContent = f.sub || "";

  // 余额类场景没有「已用比例」这个概念，就不画圆环，把空间让给数值
  const pct = typeof f.pct === "number" ? Math.max(0, Math.min(100, f.pct)) : null;
  if (pct === null) {
    el.floatRing.classList.add("hidden");
    return;
  }
  el.floatRing.classList.remove("hidden");
  fg.setAttribute("stroke", f.color || "#22d3ee");
  fg.setAttribute("stroke-dasharray", FCIRC.toFixed(2));
  fg.setAttribute("stroke-dashoffset", (FCIRC * (1 - pct / 100)).toFixed(2));
  // 环里补上百分比，口径与展开视图一致（已用占比）；取整避免 100% 撑出环内
  el.floatRingPct.textContent = `${Math.round(pct)}%`;
  el.floatRingPct.style.color = f.color || "#22d3ee";
}

/// 上次应用到窗口的悬浮卡宽度（逻辑像素），用来抑制数字位数变化带来的反复抖动
let floatWidth = 0;
/// 量值偏窄的累计补偿：校验发现仍有截断并补宽后记下差额，之后按「量值 + 补偿」落宽。
/// 不记的话，下次刷新会按偏窄的量值把窗口缩回去、再被校验补宽，形成来回跳。
let floatWidthBias = 0;

/// 量出卡片「贴合内容」时的宽度：临时把卡片设为脱离文档流 + max-content，
/// 读完立刻还原样式——同一个任务里量完就还原，浏览器来不及重绘，用户看不到这一下。
///
/// 只把卡片设成 max-content 不够：`.float-info` 是 flex:1 子项，在容器的 max-content
/// 计算里按 flex-basis(0) 参与，内容宽度会被算漏，量出来的值比实际需要的窄一截
/// （Windows 上 194 的量值对着 208 的实际需要），窗口照此收缩后副标题就被省略号截掉。
/// 所以同时把它按内容定宽，量完再还原。
function measureFloatWidth() {
  const card = el.floatCard;
  const info = card?.querySelector(".float-info");
  if (!card || !info) return 0;
  const prevCard = card.getAttribute("style");
  const prevInfo = info.getAttribute("style");
  card.style.position = "absolute";
  card.style.width = "max-content";
  info.style.flex = "0 0 auto";
  info.style.width = "max-content";
  // 取小数宽（offsetWidth 取整会少算不到 1px，正文可能就差这 1px 出省略号）
  const w = Math.ceil(card.getBoundingClientRect().width);
  if (prevInfo === null) info.removeAttribute("style");
  else info.setAttribute("style", prevInfo);
  if (prevCard === null) card.removeAttribute("style");
  else card.setAttribute("style", prevCard);
  return w;
}

/// 卡片里被省略号截掉的最大像素量：`.float-sub` / `.float-label` 的文字都是
/// nowrap + overflow:hidden，scrollWidth 仍是完整文本宽度，减去 clientWidth 就是缺口。
/// 用来在窗口按量值收缩后做一次「真的没截断吗」的校验，把预估误差补掉。
function floatClipOverflow() {
  const nodes = el.floatCard?.querySelectorAll(".float-sub, .float-label > span:first-child");
  let max = 0;
  if (nodes) for (const n of nodes) max = Math.max(max, n.scrollWidth - n.clientWidth);
  return max;
}

/// 窗口宽度落地后校验文字是否仍被省略号截断：仍有缺口就按缺口加宽再落一次
/// （最多两轮）。量出来的宽度在 Windows（字体回退到微软雅黑）上偶尔差几像素，
/// 光靠预估不够，实测一遍才稳。宽度没变时也要校验——首次定住的量值如果偏窄，
/// 它本身就是「贴合」的，光看差值会被 6px 防抖挡掉，永远修不回来。
function verifyFloatWidth(tries = 2) {
  if (tries <= 0) return;
  if (pinAnimating) {
    // 过渡期间窗口尺寸由 Rust 的动画接管，等它结束再校验（结束后 pinAnimating 自动复位）
    setTimeout(() => verifyFloatWidth(tries), 320);
    return;
  }
  // 等 WebView 把新窗口宽度应用到布局上（下一帧起效）再量，否则量到的还是旧宽度
  requestAnimationFrame(() => {
    requestAnimationFrame(async () => {
      if (pinAnimating || !document.body.classList.contains("pinned")) return;
      const gap = floatClipOverflow();
      if (gap <= 0) return;
      // 记下这次补宽的量，后续量值一并加上它，避免下次刷新又缩回去
      floatWidthBias += gap + 2;
      const fixed = floatWidth + gap + 2;
      floatWidth = fixed;
      try {
        await invoke("set_float_width", { width: fixed });
      } catch (_) {
        /* 改不动尺寸不影响内容展示，保持原宽度即可 */
        return;
      }
      verifyFloatWidth(tries - 1);
    });
  });
}

/// 定住态下让窗口宽度贴合内容；只有宽度变化超过 6px 才真正改窗口，
/// 避免刷新时数字从 36,212.47 变成 9,999.99 就抖一下。
async function fitFloatWidth(force = false) {
  if (!document.body.classList.contains("pinned")) return;
  const w = measureFloatWidth() + floatWidthBias;
  if (!w) return;
  if (force || Math.abs(w - floatWidth) >= 6) {
    floatWidth = w;
    try {
      await invoke("set_float_width", { width: w });
    } catch (_) {
      /* 改不动尺寸不影响内容展示，保持原宽度即可 */
      return;
    }
  }
  verifyFloatWidth();
}

/* ---------- 窗口高度贴合内容 ---------- */

/// 量出主视图内容的「贴合高度」（逻辑像素）：顶栏 + Tab 条 + 内容区实际内容。
/// 内容区取最后一个可见子元素的位置 + 高度再加下内边距，元素间的外边距
/// 天然包含在 offsetTop 里，比逐项累加更稳。
function measureContentHeight() {
  const topbar = document.querySelector(".topbar");
  const tabsH = el.tabs.classList.contains("hidden") ? 0 : el.tabs.offsetHeight;
  const content = document.querySelector(".content");
  const kids = [...content.children].filter((c) => !c.classList.contains("hidden"));
  const last = kids[kids.length - 1];
  if (!last) return topbar.offsetHeight + tabsH + content.offsetHeight;
  const padBottom = parseFloat(getComputedStyle(content).paddingBottom) || 0;
  return topbar.offsetHeight + tabsH + last.offsetTop + last.offsetHeight + padBottom;
}

/// 上次应用到窗口的高度（逻辑像素），用来抑制数字位数 / 换行变化带来的反复抖动
let lastFitH = 0;
/// 启动后是否已贴合过：首次直接落位不播动画，避免窗口刚显示就「缩一下」
let bootFitted = false;

/// 把窗口高度调到当前内容贴合的高度（切换 tab / 内容变化时调用），带过渡动画。
/// 定住态与设置弹窗打开期间不贴合；floor 用于抬高下限（设置弹窗需要更多空间）。
async function fitWindowHeight(force = false, floor = 0) {
  if (document.body.classList.contains("pinned")) return;
  if (settingsOpen()) return;
  // 定住 / 取消定住的动画期间窗口尺寸正在被动画接管，别去抢
  if (pinAnimating) return;
  let h = measureContentHeight();
  if (!h) return;
  if (floor) h = Math.max(h, floor);
  if (!force && Math.abs(h - lastFitH) < 6) return;
  lastFitH = h;
  const animate = bootFitted;
  bootFitted = true;
  try {
    await invoke("fit_window_height", { height: h, animate });
  } catch (_) {
    /* 改不动尺寸不影响内容展示，保持当前高度即可 */
  }
}

/* ---------- 内容区 ---------- */

/// 数据源的展示交给它自己的渲染模块：ui/sources/<source>.js。
/// 返回 true 表示本次没取到真实数据（渲染失败或接口返回空），当前展示的是上一次的结果
function renderResult(res) {
  lockedScene = false;
  // 按账号 id 缓存：同一平台的不同账号各自保留自己的上一次结果
  const id = res?.account || res?.source;
  const mod = (window.AFP_SOURCES || {})[res?.source];
  let view = null;
  try {
    view = mod ? mod.render(res?.data) : null;
  } catch (e) {
    view = null;
    console.error(e);
  }

  if (hasRealData(view)) {
    lastGoodViews[id] = {
      html: view.html,
      badge: view.badge || "—",
      float: view.float || null,
    };
    saveLastGood();
    applyCachedView(lastGoodViews[id]);
    return false;
  }

  // 没拿到真实内容：用这个源上一次的成功结果顶替，绝不把已有数据换成空占位
  if (applyCachedView(cachedViewFor(id))) return true;

  // 这个源一次成功都没有过：保留原有的占位文案
  el.cards.innerHTML =
    (view && view.html) || `<div class="empty">这个数据源还没有对应的展示</div>`;
  el.badge.textContent = (view && view.badge) || "—";
  lastFloat = (view && view.float) || null;
  updateFloat();
  return false;
}

/// 锁定态主视图：显示「连接账号」按钮 + 说明，点击后才请求钥匙串授权并加载数据
function renderLocked() {
  lockedScene = true;
  el.badge.textContent = "待授权";
  const nm = entryById(activeAccount)?.source.name || "该平台";
  el.cards.innerHTML = `
    <div class="connect-card">
      <div class="connect-title">连接 ${nm}</div>
      <p class="connect-desc">「${nm}」的凭证存在系统钥匙串里。点击后 macOS 会请求钥匙串授权，一次性读取所有已保存账号的密钥并加载数据；此后切换其他 tab 不再重复请求。授权仅在本次运行内有效。</p>
      <button type="button" class="primary-btn connect-btn" id="connect-btn">连接账号</button>
    </div>`;
  el.cards
    .querySelector("#connect-btn")
    .addEventListener("click", () => connectAccount(activeAccount));
  lastFloat = null;
  updateFloat();
}

/// 按需连接某个账号：请求钥匙串授权 → 读取全部账号密钥 → 重新加载清单与数据 → 启动刷新
async function connectAccount(id) {
  const acc = id || activeAccount;
  if (!acc) return;
  const btn = document.getElementById("connect-btn");
  if (btn) {
    btn.disabled = true;
    btn.textContent = "连接中…";
  }
  try {
    // 后端做的是「全量解锁」：一次授权读所有已保存账号，之后切换账号不再弹框
    await invoke("unlock_accounts", { accountId: acc });
    await reloadCatalog();
    await refresh(true);
  } catch (e) {
    showError(String(e));
    if (btn) {
      btn.disabled = false;
      btn.textContent = "连接账号";
    }
  }
}

async function refresh(manual = false) {
  if (loading) return;
  loading = true;
  el.refreshBtn.classList.add("spinning");
  el.refreshText.textContent = "刷新中";
  if (manual) hideError();
  // 定住态看不到主窗口状态行，把「刷新中」的反馈落在悬浮窗副标题上
  const floatBusy = manual && document.body.classList.contains("pinned");
  if (floatBusy) el.floatSub.textContent = "刷新中…";

  try {
    // 显式带上账号 id：避免切换瞬间「当前账号」还没落盘时取错数据
    const res = await invoke("get_source_data", activeAccount ? { accountId: activeAccount } : {});
    el.subtitle.textContent = res?.subtitle || res?.name || "—";
    if (res?.state === "locked") {
      // 待授权：不是错误，主视图给「连接账号」入口
      setStatus("", "待授权");
      renderLocked();
    } else {
      const stale = renderResult(res);
      if (stale) {
        // 接口通了但没返回真实数据：继续展示上一次的结果，只把状态标出来
        setStatus("err", "显示上次数据");
        showError("本次没有取到新数据，当前显示的是上一次的结果");
      } else {
        hideError();
        const now = new Date().toLocaleTimeString("zh-CN", { hour12: false });
        lastUpdatedAt = now;
        setStatus("live", `更新于 ${now}`);
      }
    }
  } catch (e) {
    lockedScene = false;
    setStatus("err", "获取失败");
    showError(String(e));
    // 取数失败不显示空占位：回落到这个源上一次的成功结果（本次运行或上次运行存的），
    // 一次都没成功过才给占位文案。错误交给顶部状态与红色横幅表达，提示窗里的
    // 「更新于」停留在上次成功时间，可据此判断数据新旧。
    if (!applyCachedView(cachedViewFor(activeAccount))) {
      el.cards.innerHTML = `<div class="empty">暂无数据</div>`;
      el.badge.textContent = "—";
      lastFloat = null;
      updateFloat();
    }
  } finally {
    loading = false;
    el.refreshBtn.classList.remove("spinning");
    el.refreshText.textContent = "刷新";
    if (floatBusy) updateFloat();
    // 提示窗开着时取数完成：顺手把里面的数字刷成新的
    if (tipShown) showTip();
    // 取数完成即重置倒计时与自动刷新定时器（场景或间隔可能刚被改写）
    applyRefreshInterval();
    // 内容高度可能随场景 / 错误横幅变化：贴合一次（切换 tab 的动画过渡就走这里）
    fitWindowHeight();
  }
}

function updateCountdown() {
  el.countdown.textContent = `${secondsLeft}s 后自动刷新`;
}

el.refreshBtn.addEventListener("click", () => refresh(true));
el.connectSm.addEventListener("click", () => connectAccount(activeAccount));

/* ---------- 账号 Tab（M-05）+ 每账号刷新间隔（E-02） ---------- */

/// 只给「已配置」的账号建 Tab（含 trae 这条自动凭证条目）：未配置的账号在设置里录入后才出现
function renderTabs() {
  const configured = accountEntries().filter((e) => e.account.configured);
  el.tabs.innerHTML = "";

  // 只有一个已配置账号时 Tab 没有切换意义，收起把纵向空间还给内容
  if (configured.length < 2) {
    el.tabs.classList.add("hidden");
    fitWindowHeight();
    return;
  }
  el.tabs.classList.remove("hidden");

  for (const e of configured) {
    const tab = document.createElement("button");
    tab.type = "button";
    tab.className = "tab" + (e.account.id === activeAccount ? " active" : "");
    tab.textContent = entryLabel(e);
    tab.title = entryLabel(e);
    tab.addEventListener("click", () => switchAccount(e.account.id));
    el.tabs.appendChild(tab);
  }
  fitWindowHeight();
}

/// 取当前账号配置的刷新间隔，并重建自动刷新定时器。
/// 当前账号未连接（钥匙串未解锁）时不启动定时器，倒计时位置显示「待连接」（no-op，不刷出错误）。
function applyRefreshInterval() {
  if (autoTimer) clearInterval(autoTimer);
  autoTimer = null;

  if (!activeReady()) {
    secondsLeft = refreshInterval;
    el.countdown.textContent = "待连接";
    el.connectSm.classList.remove("hidden");
    return;
  }

  el.connectSm.classList.add("hidden");

  refreshInterval = entryById(activeAccount)?.account.interval || defaultInterval;
  secondsLeft = refreshInterval;
  updateCountdown();

  autoTimer = setInterval(() => refresh(false), refreshInterval * 1000);
}

async function switchAccount(id) {
  if (id === activeAccount) return;
  activeAccount = id;
  renderTabs();
  applyRefreshInterval();
  try {
    await invoke("set_active_account", { accountId: id });
  } catch (e) {
    showError(String(e));
  }
  // 切到需要凭证的 tab：此刻才读钥匙串（懒加载）。未连接才请求授权，已连接直接取数。
  const e = entryById(id);
  if (e && !e.source.autoCreds && !e.account.ready) {
    await connectAccount(id);
    return;
  }
  await refresh(true);
}

/* ---------- 设置：数据源列表 + 按平台动态表单 ---------- */

function setSettingsMsg(kind, text) {
  const box = el.settingsMsg;
  if (!text) {
    box.className = "settings-msg hidden";
    box.textContent = "";
    return;
  }
  box.className = "settings-msg" + (kind ? " " + kind : "");
  box.textContent = text;
}

function settingsOpen() {
  return !el.settingsModal.classList.contains("hidden");
}

function showView(name) {
  el.listView.classList.toggle("hidden", name !== "list");
  el.detailView.classList.toggle("hidden", name !== "detail");
  el.settingsTitle.textContent = name === "detail" ? currentDef()?.name || "数据源" : "数据源";
}

/// 凭证放在哪儿，由后端下发的 storage 决定（E-01）
function renderStorageDesc() {
  el.storageDesc.textContent =
    credStorage === "keychain"
      ? "凭证保存在系统钥匙串（macOS Keychain / Windows 凭据管理器），不落到明文文件。同一平台可保存多个账号，点平台进去各自录入。"
      : `凭证以明文保存在本机（${credPath}），因为当前系统钥匙串不可用。同一平台可保存多个账号，点平台进去各自录入。`;
}

/// 平台行的状态：按它下面所有账号汇总出「最能代表该平台」的一句话
function platformState(s) {
  const accs = s.accounts || [];
  if (s.autoCreds) {
    const ok = !!accs[0]?.configured;
    return { text: ok ? "可用" : "不可用", tone: ok ? "ok" : "" };
  }
  const n = accs.length;
  if (!n) return { text: "未配置", tone: "" };
  const ready = accs.filter((a) => a.ready).length;
  const saved = accs.filter((a) => a.hasCreds).length;
  if (ready === n) return { text: `已配置 ${n} 个账号`, tone: "ok" };
  // 有凭证但本次运行还没连接：不能显示成「未配置」，否则会诱导用户重录
  if (saved > ready) return { text: `已保存 ${n} 个 · 待连接`, tone: "warn" };
  return { text: `已配置 ${ready}/${n} 个账号`, tone: "ok" };
}

function renderSourceList() {
  el.sourceList.innerHTML = "";
  if (!catalog.length) {
    el.sourceList.innerHTML = `<div class="empty">暂时没有可用的数据源</div>`;
    return;
  }

  for (const s of catalog) {
    const isCurrent = (s.accounts || []).some((a) => a.id === activeAccount);
    const row = document.createElement("button");
    row.type = "button";
    row.className = "source-row" + (isCurrent ? " current" : "");

    const main = document.createElement("span");
    main.className = "source-row-main";
    const name = document.createElement("span");
    name.className = "source-row-name";
    name.textContent = s.name;
    const sub = document.createElement("span");
    sub.className = "source-row-sub";
    sub.textContent = s.subtitle;
    main.append(name, sub);

    const st = platformState(s);
    const state = document.createElement("span");
    state.className = "source-row-state" + (st.tone ? " " + st.tone : "");
    state.textContent = st.text;

    row.append(main, state);
    row.addEventListener("click", () => openDetail(s.id));
    el.sourceList.appendChild(row);
  }
}

async function reloadCatalog() {
  const res = await invoke("list_sources");
  catalog = res?.sources || [];
  activeAccount = res?.active || null;
  credStorage = res?.storage === "file" ? "file" : "keychain";
  credPath = res?.path || "";
  renderStorageDesc();
  if (res?.defaultInterval) defaultInterval = res.defaultInterval;
  if (res?.minInterval) minInterval = res.minInterval;
  if (res?.maxInterval) maxInterval = res.maxInterval;
  renderSourceList();
  renderTabs();
  applyRefreshInterval();
}

async function openSettings() {
  setSettingsMsg("", "");
  try {
    await reloadCatalog();
  } catch (e) {
    catalog = [];
    activeAccount = null;
    renderSourceList();
    el.sourceList.innerHTML = `<div class="empty">读取数据源失败</div>`;
    console.error(e);
  }
  showView("list");
  // 弹窗需要更多纵向空间：先把窗口抬到至少 460，关闭时再贴合回内容
  await fitWindowHeight(true, 460);
  el.settingsModal.classList.remove("hidden");
}

function closeSettings() {
  el.settingsModal.classList.add("hidden");
  setSettingsMsg("", "");
  currentSource = null;
  editingAccountId = null;
  formLocked = false;
  fitWindowHeight(true);
}

/// 首个凭证字段标题后面的指引链接：点它去对应的密钥管理页
function helpLink(def) {
  const a = document.createElement("a");
  a.className = "field-help";
  a.href = "#";
  a.title = def.helpText || "";
  a.textContent = `${def.helpText} →`;
  a.addEventListener("click", (e) => {
    e.preventDefault();
    if (def.helpUrl) invoke("open_url", { url: def.helpUrl }).catch(() => {});
  });
  return a;
}

/// 打开某个平台的账号管理页：账号列表 + 新增 / 编辑 / 删除 / 设为当前
function openDetail(id) {
  if (!catalog.find((s) => s.id === id)) return;
  currentSource = { id };
  closeAccountForm();
  setSettingsMsg("", "");
  showView("detail");
}

/// 详情页顶部的平台说明（自动凭证源额外渲染只读检测信息）
function renderDetailHeader() {
  const def = currentDef();
  if (!def) return;
  el.detailInfo.innerHTML = "";
  // 自动凭证源没有账号表单，刷新间隔单独摆一块；普通平台的间隔在账号表单里
  el.detailAutoInterval.classList.toggle("hidden", def.autoCreds !== true);

  if (def.autoCreds) {
    const info = def.autoInfo || {};
    el.detailDesc.textContent = `${def.subtitle} · 自动读取本机 Trae CN 登录态，无需填写凭证。`;
    const rows = [
      ["登录态文件", info.file || "—"],
      ["token 过期时间", info.expiredAt || "—"],
      ["套餐身份", info.identity || "—"],
      ["检测结果", info.available ? "正常" : `不可用：${info.unavailable || "未知原因"}`],
    ];
    for (const [label, value] of rows) {
      const wrap = document.createElement("div");
      wrap.className = "field";
      const lb = document.createElement("span");
      lb.className = "field-label";
      lb.textContent = label;
      const val = document.createElement("div");
      val.className = "ro";
      val.textContent = value;
      wrap.append(lb, val);
      el.detailInfo.appendChild(wrap);
    }
    // 回填该源当前生效的刷新间隔（后端存在源 id 这个固定账号上）
    const acc = (def.accounts || [])[0];
    el.detailAutoIntervalInput.value = String(acc?.interval || defaultInterval);
    el.detailAutoIntervalInput.min = String(minInterval);
    el.detailAutoIntervalInput.max = String(maxInterval);
  } else {
    el.detailDesc.textContent = `${def.subtitle} · 同一平台可保存多个账号，凭证只存本机。`;
  }
}

/// 渲染该平台已有的账号列表（含每行操作）
function renderAccountList() {
  const def = currentDef();
  if (!def) return;
  const accs = def.accounts || [];

  el.accountList.innerHTML = "";
  el.accountAdd.classList.toggle("hidden", def.autoCreds === true);

  if (def.autoCreds) {
    // 自动凭证源只有一条固定条目，没有凭证字段，也不参与新增 / 删除
    const a = accs[0];
    const ok = !!a?.configured;
    const row = document.createElement("div");
    row.className = "account-row";
    const main = document.createElement("div");
    main.className = "account-row-main";
    const name = document.createElement("span");
    name.className = "account-row-name";
    name.textContent = def.name;
    const sub = document.createElement("span");
    sub.className = "account-row-sub " + (ok ? "ok" : "warn");
    sub.textContent = ok ? "自动读取本机登录态 · 可用" : "不可用";
    main.append(name, sub);
    row.appendChild(main);
    el.accountList.appendChild(row);
    return;
  }

  if (!accs.length) {
    el.accountList.innerHTML = `<div class="empty">还没有账号，点下方「新增账号」录入密钥</div>`;
    return;
  }
  for (const a of accs) el.accountList.appendChild(accountRow(a));
}

/// 单个账号一行：名称 + 状态 + （设为当前 / 编辑 / 删除）
function accountRow(a) {
  const row = document.createElement("div");
  row.className = "account-row";

  const main = document.createElement("div");
  main.className = "account-row-main";
  const name = document.createElement("span");
  name.className = "account-row-name";
  name.textContent = (a.label || "").trim() || "默认账号";
  const sub = document.createElement("span");
  sub.className = "account-row-sub";
  if (a.ready) {
    sub.textContent = "已连接 · 可用";
    sub.classList.add("ok");
  } else if (a.hasCreds) {
    // 已保存但本次运行未连接：不能显示成「未配置」，否则会诱导用户重录
    sub.textContent = "已保存 · 待连接";
    sub.classList.add("warn");
  } else {
    sub.textContent = "未配置";
  }
  main.append(name, sub);
  row.appendChild(main);

  if (a.id === activeAccount) {
    const cur = document.createElement("span");
    cur.className = "account-cur";
    cur.textContent = "当前";
    row.appendChild(cur);
  }

  const actions = document.createElement("div");
  actions.className = "account-actions";

  if (a.id !== activeAccount) {
    const useBtn = document.createElement("button");
    useBtn.type = "button";
    useBtn.className = "ghost-btn";
    useBtn.textContent = "设为当前";
    useBtn.addEventListener("click", () => makeCurrent(a.id));
    actions.appendChild(useBtn);
  }

  const editBtn = document.createElement("button");
  editBtn.type = "button";
  editBtn.className = "ghost-btn";
  editBtn.textContent = "编辑";
  editBtn.addEventListener("click", () => openAccountForm(a.id));
  actions.appendChild(editBtn);

  const delBtn = document.createElement("button");
  delBtn.type = "button";
  delBtn.className = "ghost-btn danger";
  delBtn.textContent = "删除";
  delBtn.addEventListener("click", () => removeAccount(a));
  actions.appendChild(delBtn);

  row.appendChild(actions);
  return row;
}

/// 把某个账号设为当前展示的账号
async function makeCurrent(accId) {
  try {
    await invoke("set_active_account", { accountId: accId });
    activeAccount = accId;
    await reloadCatalog();
    renderAccountList();
    setSettingsMsg("ok", "已设为当前账号");
  } catch (e) {
    setSettingsMsg("err", String(e));
  }
}

/// 删除某个账号（含它在本机保存的凭证）
async function removeAccount(a) {
  const label = (a.label || "").trim() || "默认账号";
  if (!window.confirm(`确定删除账号「${label}」？本机保存的密钥会一并清除。`)) return;
  try {
    const wasActive = a.id === activeAccount;
    await invoke("delete_account", { accountId: a.id });
    await reloadCatalog();
    renderAccountList();
    if (wasActive) await refresh(true);
    setSettingsMsg("ok", "已删除该账号");
  } catch (e) {
    setSettingsMsg("err", String(e));
  }
}

/// 底部按钮随「账号列表 / 账号表单」两种状态切换
function syncFoot() {
  const isAuto = currentDef()?.autoCreds === true;
  const formOpen = !el.accountForm.classList.contains("hidden");
  const editable = formOpen && !formLocked;

  el.detailBack.classList.toggle("hidden", formOpen);
  el.detailCancel.classList.toggle("hidden", !formOpen);
  // 自动凭证源任何时候都能「重新检测」
  el.detailTest.classList.toggle("hidden", isAuto ? false : !editable);
  el.detailTest.textContent = isAuto ? "重新检测" : "测试连接";
  // 自动凭证源没有凭证，但详情页能改刷新间隔，所以也给它一个保存按钮
  el.detailSave.classList.toggle("hidden", isAuto ? false : !editable);
  el.detailSave.textContent = isAuto
    ? "保存设置"
    : editingAccountId
      ? "保存修改"
      : "保存并切换";
}

/// 收起账号表单，回到「账号列表」状态
function closeAccountForm() {
  editingAccountId = null;
  formLocked = false;
  el.accountForm.classList.add("hidden");
  renderDetailHeader();
  renderAccountList();
  syncFoot();
}

/// 打开账号表单；accountId 为空表示「新增账号」
function openAccountForm(accountId) {
  const def = currentDef();
  if (!def || def.autoCreds) return;
  editingAccountId = accountId || null;
  setSettingsMsg("", "");

  const acc = editingAccountId ? (def.accounts || []).find((a) => a.id === editingAccountId) : null;
  // 已保存凭证但本次运行尚未连接：不能在这一态读钥匙串，也不能诱导用户重录覆盖旧密钥
  formLocked = !!acc && acc.unlocked === false && acc.hasCreds === true;

  const accName = (acc?.label || "").trim() || "默认账号";
  el.detailDesc.textContent = acc
    ? `${def.subtitle} · 正在编辑「${accName}」的凭证。`
    : `${def.subtitle} · 新增一个「${def.name}」账号，凭证只存本机。`;

  el.detailLabel.value = acc?.label || "";

  // 编辑已有账号用该账号回填的值；新增用平台 schema（值均为空）
  const fields = (acc ? acc.fields : def.fields) || [];
  el.detailFields.innerHTML = "";
  for (const [i, f] of fields.entries()) {
    const wrap = document.createElement("div");
    wrap.className = "field";

    // 标题行：字段名 + （仅首个字段）指引链接，两者同一行不额外占纵向空间
    const head = document.createElement("span");
    head.className = "field-head";
    const label = document.createElement("span");
    label.className = "field-label";
    label.textContent = f.label;
    head.appendChild(label);
    if (i === 0 && def.helpText) head.appendChild(helpLink(def));
    wrap.appendChild(head);

    const input = document.createElement("input");
    input.type = f.secret ? "password" : "text";
    input.value = f.value || "";
    input.placeholder = f.placeholder || "";
    input.spellcheck = false;
    input.autocomplete = "off";
    input.dataset.key = f.key;

    if (f.secret) {
      const row = document.createElement("span");
      row.className = "field-row";
      const toggle = document.createElement("button");
      toggle.type = "button";
      toggle.className = "ghost-btn";
      toggle.textContent = "显示";
      toggle.addEventListener("click", () => {
        const isPw = input.type === "password";
        input.type = isPw ? "text" : "password";
        toggle.textContent = isPw ? "隐藏" : "显示";
      });
      row.append(input, toggle);
      wrap.appendChild(row);
    } else {
      wrap.appendChild(input);
    }

    el.detailFields.appendChild(wrap);
  }

  // 平台特定的授权提示（如阿里云建议用 RAM 子账号只读 AK）
  if (def.note) {
    const note = document.createElement("div");
    note.className = "field-note";
    note.textContent = def.note;
    el.detailFields.appendChild(note);
  }

  el.detailInterval.value = String(acc?.interval || defaultInterval);
  el.detailInterval.min = String(minInterval);
  el.detailInterval.max = String(maxInterval);

  el.accountForm.classList.remove("hidden");
  if (formLocked) {
    setSettingsMsg(
      "warn",
      "该账号已保存凭证，但本次运行尚未连接。请先切到它的 tab 点「连接账号」读取后再修改，避免覆盖已有密钥。"
    );
  }
  syncFoot();
  el.detailFields.querySelector("input")?.focus();
}

/// 读表单里的刷新间隔；空值 / 非法值回落到默认间隔（越界由后端夹紧）
function readIntervalValue(input = el.detailInterval) {
  const raw = Number(input.value);
  if (!Number.isFinite(raw) || raw <= 0) return defaultInterval;
  return Math.round(raw);
}

function readDetailValues() {
  const values = {};
  el.detailFields.querySelectorAll("input").forEach((i) => {
    values[i.dataset.key] = i.value.trim();
  });
  return values;
}

async function saveDetail() {
  const def = currentDef();
  if (!def) return;
  // 自动凭证源（Trae）没有凭证可存：这里只落「自动刷新间隔」
  if (def.autoCreds) return saveAutoInterval(def);
  el.detailSave.disabled = true;
  el.detailSave.textContent = "保存中…";
  try {
    const accId = await invoke("save_account_credentials", {
      sourceId: def.id,
      accountId: editingAccountId,
      label: el.detailLabel.value.trim(),
      values: readDetailValues(),
    });
    await invoke("set_refresh_interval", {
      accountId: accId,
      seconds: readIntervalValue(),
    });
    // 保存后后端会把该账号设为当前，重新拉清单让 Tab / 间隔都跟上
    activeAccount = accId;
    await reloadCatalog();
    await refresh(true);
    closeSettings();
  } catch (e) {
    setSettingsMsg("err", String(e));
  } finally {
    el.detailSave.disabled = false;
    syncFoot();
  }
}

/// 自动凭证源（Trae）只保存刷新间隔：该源在后端是「源 id 即账号 id」的固定条目
async function saveAutoInterval(def) {
  el.detailSave.disabled = true;
  el.detailSave.textContent = "保存中…";
  try {
    const accId = (def.accounts || [])[0]?.id || def.id;
    const secs = await invoke("set_refresh_interval", {
      accountId: accId,
      seconds: readIntervalValue(el.detailAutoIntervalInput),
    });
    await reloadCatalog();
    openDetail(def.id);
    await refresh(true);
    setSettingsMsg("ok", `已保存，自动刷新间隔 ${secs} 秒`);
  } catch (e) {
    setSettingsMsg("err", String(e));
  } finally {
    el.detailSave.disabled = false;
    syncFoot();
  }
}

async function testDetail() {
  const def = currentDef();
  if (!def) return;

  // 自动凭证的源：重新检测 = 重新拉清单（后端现场重读登录态文件）
  if (def.autoCreds) {
    el.detailTest.disabled = true;
    el.detailTest.textContent = "检测中…";
    try {
      await reloadCatalog();
      openDetail(def.id);
      setSettingsMsg("ok", "已重新检测本机登录态");
    } catch (e) {
      setSettingsMsg("err", String(e));
    } finally {
      el.detailTest.disabled = false;
      syncFoot();
    }
    return;
  }

  el.detailTest.disabled = true;
  el.detailTest.textContent = "测试中…";
  try {
    const r = await invoke("test_source", {
      sourceId: def.id,
      values: readDetailValues(),
    });
    let text = "连接成功";
    try {
      const view = (window.AFP_SOURCES || {})[def.id]?.render(r?.data);
      if (view?.summary) text = `连接成功 · ${view.summary}`;
    } catch (_) {
      /* 展示解析失败不影响「已连通」这个结论 */
    }
    setSettingsMsg("ok", text);
  } catch (e) {
    setSettingsMsg("err", String(e));
  } finally {
    el.detailTest.disabled = false;
    syncFoot();
  }
}

el.settingsBtn.addEventListener("click", openSettings);
el.settingsClose.addEventListener("click", closeSettings);
el.settingsBackdrop.addEventListener("click", closeSettings);
el.detailBack.addEventListener("click", () => {
  setSettingsMsg("", "");
  closeAccountForm();
  showView("list");
});
el.detailCancel.addEventListener("click", () => {
  setSettingsMsg("", "");
  closeAccountForm();
});
el.accountAdd.addEventListener("click", () => openAccountForm(null));
el.detailSave.addEventListener("click", saveDetail);
el.detailTest.addEventListener("click", testDetail);

document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || !settingsOpen()) return;
  // 详情页先退回列表，再按一次才关闭弹窗；表单打开时先收起表单
  if (!el.detailView.classList.contains("hidden")) {
    setSettingsMsg("", "");
    if (!el.accountForm.classList.contains("hidden")) closeAccountForm();
    else showView("list");
    return;
  }
  closeSettings();
});

/* ---------- 定住 / 取消定住 ---------- */

// 窗口会播放约 180ms 的缩放动画，期间忽略重复点击，避免状态错乱
let pinBusy = false;

/// 是否正处于定住 / 取消定住的过渡动画期间。
/// 收缩动画里窗口会从鼠标底下扫过，若不抑制，mouseenter 会触发 tip 弹出造成抖动；
/// 高度贴合这时也在跟动画抢窗口尺寸，一并停掉。
let pinAnimating = false;
let pinAnimatingTimer = 0;
function beginPinAnimation(ms = 300) {
  pinAnimating = true;
  clearTimeout(pinAnimatingTimer);
  // 定住方向 Rust 侧动画约 180ms，留点余量；取消定住方向 Rust 要先等标题栏恢复
  // （最长约 600ms）再播动画，抑制窗要盖到整段过渡结束，见 setPinned(false)
  pinAnimatingTimer = setTimeout(() => {
    pinAnimating = false;
  }, ms);
}

async function setPinned(on) {
  if (pinBusy) return;
  pinBusy = true;
  setTimeout(() => {
    pinBusy = false;
  }, 500);
  beginPinAnimation();
  try {
    if (on) {
      closeSettings();
      // 先切到悬浮布局：一是量出内容宽度，好让窗口一次缩到位（动画结束就不再跳一下）；
      // 二是让收缩过程里展示的就是悬浮卡片（Rust 随后还会补发一次 pin-state）
      document.body.classList.add("pinned");
      updateFloat();
      const w = measureFloatWidth() + floatWidthBias;
      floatWidth = w;
      await invoke("set_pinned", { on, width: w || null });
    } else {
      // 退出悬浮态时一并收起提示窗（Rust 侧也会收，这里先收掉避免闪一下）
      hideTip();
      // 取消定住的整段过渡（等标题栏 + 动画）最长约 800ms，这期间窗口还是窄尺寸：
      // 抑制 tip 弹出，也抑制高度贴合（贴合会保持当前窄宽度，把窄宽度固化下来）
      beginPinAnimation(1000);
      // 趁还在定住态先量主视图贴合高度（摘 class → 量 → 还原，同一任务内完成不会重绘），
      // 让 Rust 的取消定住动画直接落到这个高度，落地后不再二次调高。
      // 注意量之前把 body 临时固定到普通窗宽 420px：此刻窗口还是悬浮态的窄宽度，
      // 按窄宽度量会让卡片文字换行更多、高度偏大，回到 420 宽后底部就多出空白
      document.body.classList.remove("pinned");
      const prevBodyW = document.body.style.width;
      document.body.style.width = "420px";
      const h = measureContentHeight();
      document.body.style.width = prevBodyW;
      document.body.classList.add("pinned");
      lastFitH = h;
      bootFitted = true;
      await invoke("set_pinned", { on, height: h || null });
      // 兜底：等 Rust 的取消定住动画完全落地后（含等标题栏的时间）再用最终布局量一次，
      // 偏差超过阈值就纠正（量准则无动作）。太早量会量到过渡中的窄宽度
      setTimeout(() => fitWindowHeight(true), 1000);
    }
  } catch (e) {
    if (on) document.body.classList.remove("pinned");
    showError(String(e));
  }
}

// 由 Rust 在动画开始前发出，让内容布局与窗口缩放同步切换
window.__TAURI__.event.listen("pin-state", (e) => {
  const on = !!e.payload;
  document.body.classList.toggle("pinned", on);
  if (on) updateFloat();
  else hideTip();
});

el.pinBtn.addEventListener("click", () => setPinned(true));
el.floatClose.addEventListener("click", () => setPinned(false));

// 悬浮窗整卡可点：点一下立刻重取数据（锁定态且需钥匙串则先走解锁）。
// 卡片同时是原生拖拽区，拖完系统会补一个 click，所以要按位移把拖拽排掉。
// 这里必须用屏幕绝对坐标 e.screenX/e.screenY：拖拽时窗口跟着指针一起走，
// 指针相对窗口的 clientX/clientY 几乎不变，只有屏幕坐标才反映真实位移。
let floatDown = null;
/// 这一次按下有没有真的拖动窗口（Windows 走下面的阈值拖拽，拖过就不算点击）
let floatDragged = false;

el.floatCard.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  floatDown = { x: e.screenX, y: e.screenY };
  floatDragged = false;
  // 一按下去（可能拖拽）就收起提示窗，别让提示跟着窗口一起拖
  tipHold = true;
  clearTimeout(tipTimer);
  hideTip();
});

// Windows 不能用 Tauri 默认的「按下即拖」（data-tauri-drag-region="deep"）：
// 那条路走的是系统模态拖拽循环，鼠标松开的消息在那个循环里就被吃掉了，网页永远
// 收不到 click —— 表现就是「点卡片刷新，点了没反应」，偶尔数字变了其实是自动刷新。
// 这里改成先把窗口留着，指针真的移动了（阈值 5px）才把窗口交给系统去拖；
// 没移动就按普通点击走 click → 点击刷新恢复可靠。macOS 保持按下即拖不变。
if (document.body.classList.contains("is-windows")) {
  el.floatCard.removeAttribute("data-tauri-drag-region");
  el.floatCard.addEventListener("pointermove", (e) => {
    if (!floatDown || floatDragged || e.buttons !== 1) return;
    if (Math.hypot(e.screenX - floatDown.x, e.screenY - floatDown.y) < 5) return;
    floatDragged = true;
    invoke("plugin:window|start_dragging").catch(() => {});
  });
}

el.floatCard.addEventListener("pointercancel", () => {
  floatDown = null;
});
el.floatCard.addEventListener("click", (e) => {
  if (e.target.closest(".float-close")) return;
  const moved =
    floatDown && Math.hypot(e.screenX - floatDown.x, e.screenY - floatDown.y) > 6;
  floatDown = null;
  if (moved || floatDragged) return;
  if (lockedScene) connectAccount(activeAccount);
  else refresh(true);
});

// 悬浮卡片右键菜单：由 Rust 弹系统原生菜单（网页里自绘会被这么小的窗口裁掉，
// 系统菜单贴屏幕边缘时的翻转也由系统负责），
// 菜单项「退出悬浮窗」走 menu-unpin 事件回到 setPinned，「退出软件」由 Rust 直接退出。
// 这里 preventDefault 才不会再弹出 WebView 自带的 Reload 菜单。
el.floatCard.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  invoke("show_card_menu").catch(() => {});
});

window.__TAURI__.event.listen("menu-unpin", () => setPinned(false));

// 主窗口关闭按钮 = 定住：Rust 拦下关闭后发这个事件，这里走与「定住」按钮同一条路径
window.__TAURI__.event.listen("close-to-pin", () => setPinned(true));

/* ---------- 悬浮卡片悬停提示窗 ---------- */

/// 提示窗内容比卡片窗口还高，所以它落在另一个小窗口里（见 Rust 的 show_float_tip），
/// 这里只负责算内容和决定什么时候让它出现 / 收起。
/// 各行内容由数据源自己给（`float.tip`）——「预计可用天数」的口径各平台不同，
/// 比如 Trae 的月配额型按「订阅包已用 ÷ 已过天数」当场算，不走本地采样。
let lastUpdatedAt = "";
let tipShown = false;
/// 指针按下过（拖拽 / 点击）之后先别弹，等指针重新进来或松手后再弹
let tipHold = false;
let tipTimer = null;

/// 组装提示窗内容：标题 → 剩余 → 各平台自己的行 → 页脚
function buildTipPayload() {
  const f = lastFloat;
  if (!f) return null;

  const rows = [[f.remainLabel || "剩余", `${f.remain ?? "—"}${f.unit ? ` ${f.unit}` : ""}`]];
  for (const [k, v, tone] of f.tip || []) rows.push(tone ? [k, v, tone] : [k, v]);

  return {
    title: f.title || el.subtitle.textContent || "用量",
    rows,
    foot: [`更新于 ${lastUpdatedAt || "—"}`, "点击面板可刷新"],
  };
}

async function showTip() {
  if (!document.body.classList.contains("pinned")) return;
  // 过渡动画期间窗口在鼠标底下扫过，此时弹 tip 会造成抖动，跳过
  if (pinAnimating) return;
  const payload = buildTipPayload();
  if (!payload) return;
  tipShown = true;
  try {
    await invoke("show_float_tip", { payload });
    // 两个 invoke 之间不保证先后：若这次「显示」落地前 mouseleave 已把 tipShown 翻成
    // false（hide 先被 Rust 执行），窗口就会在 hide 之后又被 show 出来、一直挂着。
    // 这里补一次隐藏，把顺序纠回来。
    if (!tipShown) await invoke("hide_float_tip");
  } catch (_) {
    tipShown = false;
  }
}

async function hideTip() {
  if (!tipShown) return;
  tipShown = false;
  try {
    await invoke("hide_float_tip");
  } catch (_) {
    /* 收起失败也不影响使用，下次进入还会重设 */
  }
}

// 停在卡片上：略等一下再弹（只是划过就不弹了）；离开就收。
// 松手后再等一拍重弹，这样「点一下刷新」之后提示还会回来。
el.floatCard.addEventListener("mouseenter", () => {
  if (tipHold) return;
  clearTimeout(tipTimer);
  tipTimer = setTimeout(showTip, 150);
});
/// 这发 mouseleave 是不是「假的」：坐标还在悬浮卡范围内，就说明指针并没有真的离开。
/// WebKit 在我们自己改写卡片布局（量宽度时会临时把卡片设成 absolute + max-content）
/// 或程序改了窗口尺寸之后会重算命中测试，可能凭空补一发 mouseleave 却不补 mouseenter，
/// 原来的实现据此收提示窗，就成了「刷新数据时提示窗闪一下、然后没了」。
/// 真离开的 mouseleave 坐标必然落在卡片外，照旧收起。
function isSpuriousLeave(e) {
  if (typeof e.clientX !== "number" || typeof e.clientY !== "number") return false;
  const r = el.floatCard.getBoundingClientRect();
  const m = 2;
  return (
    e.clientX >= r.left - m &&
    e.clientX <= r.right + m &&
    e.clientY >= r.top - m &&
    e.clientY <= r.bottom + m
  );
}

el.floatCard.addEventListener("mouseleave", (e) => {
  clearTimeout(tipTimer);
  tipHold = false;
  if (isSpuriousLeave(e)) return;
  hideTip();
});
el.floatCard.addEventListener("pointerup", () => {
  tipHold = false;
  clearTimeout(tipTimer);
  tipTimer = setTimeout(showTip, 400);
});

// 倒计时每秒走一格；自动刷新定时器由 applyRefreshInterval 按当前场景的间隔维护
setInterval(() => {
  // 当前场景未连接：倒计时冻结（显示「待连接」），不做任何刷新
  if (!activeReady()) return;
  secondsLeft -= 1;
  if (secondsLeft <= 0) {
    secondsLeft = refreshInterval;
  }
  updateCountdown();
}, 1000);

async function bootstrap() {
  // 先拉清单（不触碰钥匙串）：让 tab / 列表 / 待连接卡片都能拿到平台名，再取数
  try {
    await reloadCatalog();
    const cur = entryById(activeAccount);
    // 已保存凭证但本次尚未连接的钥匙串账号不弹设置（主界面已给「连接账号」入口）；
    // trae 不可用 / 确实未配置仍按原逻辑引导去设置页录入。
    if (!cur?.account.configured) openSettings();
  } catch (_) {
    /* 忽略：清单读取失败不影响主流程 */
  }
  await refresh(false);
}

// 启动流程：窗口以 visible:false 创建（否则会先闪一下主界面大窗、悬浮态很穿帮）。
// 先问 Rust 上次是不是定住态，在窗口还隐藏着的时候就把布局切成对应的形态，再显示，最后才开始取数。
(async () => {
  let pinnedAtStart = false;
  try {
    const st = await invoke("startup_state");
    pinnedAtStart = !!st.pinned;
    document.body.classList.toggle("pinned", pinnedAtStart);
    if (pinnedAtStart) updateFloat();
    // 设置面板底部的版本号：Rust 侧带回编译进二进制的包版本
    if (st.version) el.appVersion.textContent = `唐三浪悬浮窗 v${st.version}`;
  } catch (_) {
    /* 拿不到状态就按主界面显示 */
  }
  try {
    await invoke("show_window");
  } catch (_) {
    /* 显示失败也有 Rust 侧 3 秒兜底 */
  }
  bootstrap();
})();
