// Trae Code 积分：本机 Trae CN 登录态自动解密取 token，POST /trae/api/v2/pay/ide_user_ent_usage
// 后端下发的 data = { raw: 接口原始响应, identity: 套餐身份, expiredAt: 登录态到期, file: 登录态文件 }
(function () {
  const { fmtNum, num, ringColor } = window.AFP_UTIL;

  const R = 32;
  const CIRC = 2 * Math.PI * R;

  /// 复刻官方聚合口径：遍历 user_entitlement_pack_list
  /// credits_limit = -1 视为不限量；> 0 累加总额，usage.credits_amount 累加已用
  function aggregate(raw) {
    const packs = Array.isArray(raw?.user_entitlement_pack_list)
      ? raw.user_entitlement_pack_list
      : [];
    let used = 0;
    let total = 0;
    let unlimited = false;
    for (const pk of packs) {
      const quota = pk?.entitlement_base_info?.quota || {};
      const limit = quota.credits_limit;
      const amount = num(pk?.usage?.credits_amount) ?? 0;
      if (limit === -1) {
        unlimited = true;
        continue;
      }
      const l = num(limit);
      if (l !== null && l > 0) {
        total += l;
        used += amount;
      }
    }
    const remaining = unlimited ? null : Math.max(total - used, 0);
    return { used, total, unlimited, remaining, hasPacks: packs.length > 0 };
  }

  /// 登录态到期时间（ISO）-> MM-DD HH:MM
  function fmtDate(iso) {
    if (!iso) return "";
    const dt = new Date(iso);
    if (isNaN(dt.getTime())) return "";
    const mm = String(dt.getMonth() + 1).padStart(2, "0");
    const dd = String(dt.getDate()).padStart(2, "0");
    const hm = dt.toTimeString().slice(0, 5);
    return `${mm}-${dd} ${hm}`;
  }

  const DAY_MS = 24 * 3600 * 1000;

  /// 自动签到结果文案（后端 data.checkin = { checked_in, claimed, enable, ... }）
  /// 返回空串表示这次没拿到签到结果，界面不显示该行
  function checkinText(checkin) {
    if (!checkin) return "";
    if (checkin.claimed) return "已自动签到";
    if (checkin.checked_in) return "今日已签到";
    if (checkin.enable === false) return "签到未开放";
    return "签到失败";
  }

  /// 时间戳（秒）-> MM-DD
  function fmtMd(sec) {
    const dt = new Date(Number(sec) * 1000);
    if (isNaN(dt.getTime())) return "";
    const mm = String(dt.getMonth() + 1).padStart(2, "0");
    const dd = String(dt.getDate()).padStart(2, "0");
    return `${mm}-${dd}`;
  }

  /// 主订阅包：当前未过期、额度最大的那个包（Ultra 连续包月这类）。
  /// 日均消耗只按它算——签到 / 邀请 / 福利这类一次性奖励包往往很快就用光，
  /// 把它们算进去会把日均抬得虚高。
  function mainPack(raw) {
    const packs = Array.isArray(raw?.user_entitlement_pack_list)
      ? raw.user_entitlement_pack_list
      : [];
    const now = Date.now();
    let best = null;
    for (const pk of packs) {
      const b = pk?.entitlement_base_info || {};
      const limit = num(b?.quota?.credits_limit);
      const start = num(b.start_time);
      const end = num(b.end_time);
      if (limit === null || limit <= 0 || end === null || end * 1000 <= now) continue;
      if (!best || limit > best.limit) {
        best = { limit, start: start || 0, end, used: num(pk?.usage?.credits_amount) || 0 };
      }
    }
    return best;
  }

  /// 提示窗里的「预计可用天数」：
  /// 日均消耗 = 主订阅包已用 ÷ 该包已过天数；预计天数 = 剩余积分 ÷ 日均消耗，
  /// 且不超过距重置的天数——本周期内用不完就显示到重置为止，并在后面括注重置日期。
  /// 周期刚开始（不足 1 天）日均消耗还没代表性，返回「—」。
  function estimateDaysText(raw, remain) {
    const pk = mainPack(raw);
    const remainNum = typeof remain === "number" && isFinite(remain) ? remain : null;
    if (!pk || remainNum === null || remainNum <= 0) return "—";
    const now = Date.now();
    if (!(pk.start > 0)) return "—";
    const elapsedDays = (now - pk.start * 1000) / DAY_MS;
    if (elapsedDays < 1) return "—";
    const burn = pk.used / elapsedDays;
    if (!(burn > 0)) return "—";
    const leftInPeriod = (pk.end * 1000 - now) / DAY_MS;
    if (!(leftInPeriod > 0)) return "—";
    const days = Math.min(remainNum / burn, leftInPeriod);
    const n = days >= 10 ? days.toFixed(0) : days.toFixed(1);
    return `${n} 天（${fmtMd(pk.end)} 重置）`;
  }

  window.AFP_SOURCES = window.AFP_SOURCES || {};
  window.AFP_SOURCES.trae = {
    render(data) {
      const raw = data?.raw ?? data ?? {};
      const identity = data?.identity || "";
      const expiredAt = data?.expiredAt || "";
      const agg = aggregate(raw);
      const badge = identity || "Trae";
      const checkin = checkinText(data?.checkin);

      if (!agg.hasPacks) {
        return {
          badge,
          summary: "已连通，但返回里没有套餐信息",
          html: `<div class="empty">已连通，但没有可解析的积分字段</div>`,
          float: null,
        };
      }

      if (agg.unlimited) {
        return {
          badge,
          summary: "不限量套餐",
          html: `
            <div class="card">
              <div class="card-info">
                <div class="card-title">Trae Code 积分</div>
                <div class="card-sub">剩余 <span class="remain">∞</span> 积分 · 不限量套餐</div>
                <div class="card-reset">${identity ? `套餐 ${identity}` : "重置 —"}${checkin ? ` · ${checkin}` : ""}</div>
              </div>
            </div>`,
          float: {
            label: "Trae Code",
            remain: "∞",
            unit: "",
            sub: identity ? `${identity} · 不限量` : "不限量套餐",
            reset: "",
            pct: null,
            color: ringColor(0),
            // 悬停提示窗：不限量套餐没有「消耗完」的概念，不给预计天数（remainNum 为 null）
            title: "TRAE 积分余额",
            remainNum: null,
            tip: [
              ["套餐", identity ? `${identity} · 不限量` : "不限量套餐"],
              ...(checkin ? [["签到", checkin]] : []),
            ],
          },
        };
      }

      const pct = agg.total > 0 ? (agg.used / agg.total) * 100 : 0;
      const clamped = Math.max(0, Math.min(100, pct));
      const color = ringColor(pct);
      const offset = CIRC * (1 - clamped / 100);
      const remainTxt = fmtNum(agg.remaining);
      const daysText = estimateDaysText(raw, agg.remaining);
      // 卡片第三行：套餐身份后面直接跟「可用天数（重置日期）」，不用悬停就能看到还能撑多久
      const sub =
        daysText === "—"
          ? identity || `已用 ${pct.toFixed(1)}%`
          : identity
            ? `${identity} · ${daysText}`
            : daysText;

      return {
        badge,
        summary: `剩余积分 ${remainTxt}`,
        html: `
          <div class="card">
            <div class="ring">
              <svg width="78" height="78" viewBox="0 0 78 78">
                <circle class="ring-bg" cx="39" cy="39" r="${R}" fill="none" stroke-width="7" />
                <circle class="ring-fg" cx="39" cy="39" r="${R}" fill="none" stroke-width="7"
                  style="stroke:${color}" stroke-dasharray="${CIRC.toFixed(2)}"
                  stroke-dashoffset="${offset.toFixed(2)}" />
              </svg>
              <div class="ring-label">
                <div class="ring-pct" style="color:${color}">${pct.toFixed(1)}%</div>
                <div class="ring-unit">已用</div>
              </div>
            </div>
            <div class="card-info">
              <div class="card-title">Trae Code 积分</div>
              <div class="card-sub">剩余 <span class="remain">${remainTxt}</span> 积分 · 已用 ${fmtNum(agg.used)} / 共 ${fmtNum(agg.total)}</div>
              <div class="card-reset">重置 —${expiredAt ? ` · 登录态 ${fmtDate(expiredAt)} 到期` : ""}${checkin ? ` · ${checkin}` : ""}</div>
            </div>
          </div>`,
        float: {
          label: "Trae Code",
          remain: remainTxt,
          unit: "积分",
          sub,
          reset: "",
          pct,
          color,
          // 悬停提示窗：剩余行由前端按 remain/unit 统一补，这里给套餐、已用与预计剩余天数
          // （天数行用 accent 强调色，见 tooltip.html 的 .tip-row.accent）
          title: "TRAE 积分余额",
          remainNum: agg.remaining,
          tip: [
            ["套餐", identity || "—"],
            ["已用", `${fmtNum(agg.used)} / ${fmtNum(agg.total)}`],
            ["预计剩余", daysText, "accent"],
            ...(checkin ? [["签到", checkin]] : []),
          ],
        },
      };
    },
  };
})();
