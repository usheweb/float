// 阿里云：费用中心余额 + 到期时间（云服务器 / 云数据库 / 域名 / SSL 证书）
// 后端把 BSS（QueryAvailableInstances）+ Domain（QueryDomainList）+ CAS
// （ListUserCertificateOrder）三个接口的结果合并成同一张到期列表，
// data = { balance, expiring[], sms, warnings[] }
(function () {
  const { fmtNum, num } = window.AFP_UTIL;

  const CUR_SYMBOL = { CNY: "¥", USD: "$", JPY: "JP¥" };
  const DAY_MS = 86400000;

  function symbol(currency) {
    const c = String(currency || "CNY").toUpperCase();
    return CUR_SYMBOL[c] || `${c} `;
  }

  /// ISO8601 -> MM-DD（到期日只需看月日）
  function fmtMd(iso) {
    const t = Date.parse(iso);
    if (!isFinite(t)) return "—";
    const dt = new Date(t);
    const mm = String(dt.getMonth() + 1).padStart(2, "0");
    const dd = String(dt.getDate()).padStart(2, "0");
    return `${mm}-${dd}`;
  }

  /// 剩余天数：按本地时间取整，今天到期算 0
  function daysLeft(iso) {
    const t = Date.parse(iso);
    if (!isFinite(t)) return null;
    return Math.floor((t - Date.now()) / DAY_MS);
  }

  /// 剩余天数的文案；后端已滤掉已过期的条目，"今天到期"是最后一天
  function daysText(d) {
    if (d === null) return "—";
    if (d < 0) return `已过期 ${-d} 天`;
    if (d === 0) return "今天到期";
    return `${d} 天`;
  }

  /// 到期紧迫度配色：≤7 天品红、≤30 天黄，其余绿（取家族 CSS 变量）
  function urgencyColor(d) {
    if (d === null) return "var(--text-dim)";
    if (d <= 7) return "var(--magenta)";
    if (d <= 30) return "var(--yellow)";
    return "var(--green)";
  }

  /// 统一的到期列表：后端已按到期时间升序，这里再兜一次并按有效期过滤
  function rows(data) {
    const list = Array.isArray(data?.expiring) ? data.expiring.slice() : [];
    return list
      .map((it) => ({ it, days: daysLeft(it?.endTime) }))
      .filter((r) => r.days !== null)
      .sort((a, b) => a.days - b.days);
  }

  function balanceCard(balance, nearest, count) {
    const sym = symbol(balance.currency);
    const available = num(balance.available);
    const cash = num(balance.cash);
    const main = available !== null ? available : cash;
    const rows = [];

    if (nearest) {
      rows.push(
        `<div class="balance-row"><span>最近到期</span><b style="color:${urgencyColor(
          nearest.days,
        )}">${nearest.it.product || "—"} · ${daysText(nearest.days)}</b></div>`,
      );
    }
    if (count) {
      rows.push(`<div class="balance-row"><span>到期事项</span><b>${count} 项</b></div>`);
    }
    // 可用额度与现金余额是两个口径（可用额度含代金券等），都在卡片上列出来
    if (available !== null) {
      rows.push(`<div class="balance-row"><span>可用额度</span><b>${fmtNum(available)}</b></div>`);
    }
    if (cash !== null && cash !== available) {
      rows.push(`<div class="balance-row"><span>现金余额</span><b>${fmtNum(cash)}</b></div>`);
    }

    return `
      <div class="balance-card">
        <div class="balance-title">账户余额（可用额度）</div>
        <div class="balance-value"><span>${
          main === null ? "—" : fmtNum(main)
        }</span><em>${balance.currency || "CNY"}</em></div>
        ${rows.length ? `<div class="balance-rows">${rows.join("")}</div>` : ""}
      </div>`;
  }

  function expireCard(r) {
    const it = r.it || {};
    const name = it.name || "";
    const region = it.region || "";
    const sub = region ? `${name} · ${region}` : name;
    return `
      <div class="card">
        <div class="card-info">
          <div class="card-title">
            <span>${it.product || "到期项"}</span>
            <span class="expire-tag" style="color:${urgencyColor(r.days)}">${daysText(r.days)}</span>
          </div>
          ${sub ? `<div class="card-sub">${sub}</div>` : ""}
          <div class="card-reset">${fmtMd(it.endTime)} 到期</div>
        </div>
      </div>`;
  }

  function smsCard(sms) {
    const remain = num(sms.remaining);
    const total = num(sms.total);
    const unit = sms.unit || "条";
    const rows = [];
    if (total !== null) {
      rows.push(
        `<div class="balance-row"><span>套餐总量</span><b>${fmtNum(total)} ${unit}</b></div>`,
      );
    }
    if (sms.endTime) {
      rows.push(
        `<div class="balance-row"><span>有效期至</span><b>${fmtMd(sms.endTime)}</b></div>`,
      );
    }
    return `
      <div class="balance-card">
        <div class="balance-title">短信套餐余量</div>
        <div class="balance-value"><span>${
          remain === null ? "—" : fmtNum(remain)
        }</span><em>${unit}</em></div>
        ${rows.length ? `<div class="balance-rows">${rows.join("")}</div>` : ""}
      </div>`;
  }

  window.AFP_SOURCES = window.AFP_SOURCES || {};
  window.AFP_SOURCES.aliyun = {
    render(data) {
      const balance = data?.balance || null;
      const list = rows(data);
      const nearest = list[0] || null;
      const sms = data?.sms || null;
      const warnings = Array.isArray(data?.warnings) ? data.warnings : [];

      if (!balance && !list.length && !sms) {
        return {
          badge: "—",
          summary: "已连通，但没有可展示的数据",
          html: `<div class="empty">已连通，但余额与到期信息都没取到</div>`,
          float: null,
        };
      }

      const html = [
        balance ? balanceCard(balance, nearest, list.length) : "",
        list.length ? `<div class="list-label">即将到期</div>` : "",
        list.map(expireCard).join(""),
        sms ? smsCard(sms) : "",
        warnings.length
          ? `<div class="balance-warn">部分数据未取到${warnings
              .map((w) => `<br>· ${w}`)
              .join("")}</div>`
          : "",
      ].join("");

      // 悬浮卡：大数字给「最近到期天数」，副标题给账户余额，两样同屏可见
      const sym = symbol(balance?.currency);
      const available = num(balance?.available);
      const cash = num(balance?.cash);
      const balanceNum = available !== null ? available : cash;
      const balanceTxt =
        balanceNum === null ? "账户余额 —" : `账户余额 ${sym}${fmtNum(balanceNum)}`;

      const sub = nearest
        ? `${balance ? balanceTxt : "账户"} · ${fmtMd(nearest.it.endTime)} 到期`
        : balanceTxt;

      const tip = [];
      if (nearest) {
        tip.push([
          "最近到期",
          `${nearest.it.product || "—"}${nearest.it.name ? ` · ${nearest.it.name}` : ""}`,
        ]);
        tip.push(["到期时间", `${fmtMd(nearest.it.endTime)}（${daysText(nearest.days)}）`, "accent"]);
      }
      if (list.length > 1) tip.push(["待续费", `${list.length} 项`]);
      if (balanceNum !== null) tip.push(["账户余额", `${sym}${fmtNum(balanceNum)}`]);
      if (sms) {
        const remain = num(sms.remaining);
        tip.push(["短信余量", remain === null ? "—" : `${fmtNum(remain)} ${sms.unit || "条"}`]);
      }

      return {
        badge: nearest ? `${list.length} 项到期` : "无到期",
        summary: nearest
          ? `最近到期 ${daysText(nearest.days)} · ${nearest.it.product || ""}`
          : "暂无即将到期",
        html,
        float: {
          label: "阿里云",
          remain: nearest ? nearest.days : "—",
          unit: nearest ? "天" : "",
          sub,
          reset: "",
          // 到期倒计时没有「已用比例」的概念，不画圆环，把空间让给数值
          pct: null,
          color: urgencyColor(nearest ? nearest.days : null),
          title: "阿里云到期提醒",
          remainLabel: "最近到期",
          tip,
        },
      };
    },
  };
})();
