// 悬浮卡片悬停提示窗的渲染脚本。
// 内容由主窗口（main.js）算好后通过 Rust 的 show_float_tip 以 tip-data 事件推过来，
// 这个窗口不接收鼠标事件，只负责画。
const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

/// 让提示窗高度贴合内容：各平台提示行数不同，窗口高度固定时行数少的会在下方空一块。
/// 量出 .tip 的真实高度后让 Rust 把窗口缩到刚好（Rust 会按同一「卡片上方/下方」规则重新摆位）。
function fitHeight() {
  const tip = document.querySelector(".tip");
  if (!tip) return;
  const h = Math.ceil(tip.getBoundingClientRect().height);
  if (h > 0) invoke("set_tip_height", { height: h }).catch(() => {});
}

/// 渲染提示内容：{ title, rows: [[标签, 值, 样式?]], foot: [文案] }
function render(p) {
  const title = document.getElementById("tip-title");
  const rows = document.getElementById("tip-rows");
  const foot = document.getElementById("tip-foot");

  title.textContent = p?.title || "";

  rows.innerHTML = "";
  for (const row of p?.rows || []) {
    const line = document.createElement("div");
    line.className = "tip-row" + (row[2] ? " " + row[2] : "");

    const k = document.createElement("span");
    k.className = "tip-k";
    k.textContent = row[0];

    const v = document.createElement("span");
    v.className = "tip-v";
    v.textContent = row[1];

    line.append(k, v);
    rows.appendChild(line);
  }

  foot.innerHTML = "";
  for (const text of p?.foot || []) {
    const line = document.createElement("div");
    line.textContent = text;
    foot.appendChild(line);
  }

  fitHeight();
}

listen("tip-data", (e) => render(e.payload));

// 主窗口切换主题时同步过来：两个窗口各自一份 CSS，必须显式通知
listen("theme-change", (e) => {
  document.documentElement.dataset.theme = e.payload === "dark" ? "dark" : "light";
});
