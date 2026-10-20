// 主题：亮色为默认值，用户可在设置面板切到暗色。
// 必须在样式表生效前同步跑完，否则首屏会先画一套再切到另一套。
// 提示窗（tooltip.html）也加载本文件，保证两个窗口取到同一主题。
(function () {
  const KEY = "afp:theme";
  let theme = "light";
  try {
    if (localStorage.getItem(KEY) === "dark") theme = "dark";
  } catch (_) {
    /* 读不到 localStorage 就用默认亮色 */
  }
  document.documentElement.dataset.theme = theme;
})();
