/* Portreaper site — behavior: i18n toggle, OS highlight, version fetch, tabs, copy. */
(function () {
  "use strict";

  var LANG_KEY = "portreaper.site.lang";
  var DICT = window.I18N || { zh: {}, en: {} };

  /* ───── i18n ───── */

  function resolveInitialLang() {
    var saved = null;
    try {
      saved = localStorage.getItem(LANG_KEY);
    } catch {
      /* localStorage may be unavailable */
    }
    if (saved === "zh" || saved === "en") return saved;
    var nav = (navigator.language || "en").toLowerCase();
    return nav.indexOf("zh") === 0 ? "zh" : "en";
  }

  var currentLang = resolveInitialLang();

  function t(key) {
    var table = DICT[currentLang] || {};
    if (key in table) return table[key];
    var fb = DICT.zh || {};
    return key in fb ? fb[key] : key;
  }

  function applyLang(lang) {
    currentLang = lang === "en" ? "en" : "zh";
    try {
      localStorage.setItem(LANG_KEY, currentLang);
    } catch {
      /* ignore */
    }

    document.documentElement.setAttribute("lang", currentLang === "zh" ? "zh-CN" : "en");

    // textContent / innerHTML for data-i18n nodes
    var nodes = document.querySelectorAll("[data-i18n]");
    for (var i = 0; i < nodes.length; i++) {
      var el = nodes[i];
      var key = el.getAttribute("data-i18n");
      var val = t(key);
      if (el.tagName === "META") {
        el.setAttribute("content", val);
      } else if (el.tagName === "IMG") {
        // 图片的 data-i18n 落到 alt（评审发现：截图 alt 曾写死中文，
        // 英文访客的读屏拿到中文文案）
        el.setAttribute("alt", val);
      } else if (/<[a-z][\s\S]*>/i.test(val)) {
        el.innerHTML = val;
      } else {
        el.textContent = val;
      }
    }

    // aria-labels
    var ariaNodes = document.querySelectorAll("[data-i18n-aria]");
    for (var j = 0; j < ariaNodes.length; j++) {
      ariaNodes[j].setAttribute("aria-label", t(ariaNodes[j].getAttribute("data-i18n-aria")));
    }

    // toggle button shows the OTHER language
    var toggle = document.getElementById("lang-toggle");
    if (toggle) {
      toggle.textContent = currentLang === "zh" ? "EN" : "中文";
      toggle.setAttribute("aria-label", currentLang === "zh" ? "Switch to English" : "切换到中文");
    }

    // re-render version line in the active language if data is present
    renderVersion();
  }

  function bindLangToggle() {
    var toggle = document.getElementById("lang-toggle");
    if (!toggle) return;
    toggle.addEventListener("click", function () {
      applyLang(currentLang === "zh" ? "en" : "zh");
    });
  }

  /* ───── OS / arch highlight ───── */

  function detectTarget() {
    var ua = navigator.userAgent || "";
    var plat = (navigator.platform || "").toLowerCase();
    var uaData = navigator.userAgentData;
    var platformHint = uaData && uaData.platform ? uaData.platform.toLowerCase() : "";

    var isWindows =
      plat.indexOf("win") === 0 || platformHint.indexOf("win") !== -1 || /windows/i.test(ua);
    var isMac =
      plat.indexOf("mac") === 0 || platformHint.indexOf("mac") !== -1 || /mac os x/i.test(ua);

    if (isWindows) return "win";
    // arm vs intel mac can't be reliably detected → highlight macOS generally.
    if (isMac) return "mac";
    return null;
  }

  function highlightDownload() {
    var target = detectTarget();
    if (!target) return;
    var btns = document.querySelectorAll("#downloads [data-dl]");
    for (var i = 0; i < btns.length; i++) {
      var dl = btns[i].getAttribute("data-dl");
      var match = target === "win" ? dl === "win" : dl === "mac-arm" || dl === "mac-x64";
      if (match) btns[i].classList.add("is-recommended");
    }
  }

  /* ───── Latest release version (graceful) ───── */

  var releaseData = null;

  function renderVersion() {
    var line = document.getElementById("version-line");
    if (!line || !releaseData) return;
    var tag = releaseData.tag_name || "";
    var date = "";
    if (releaseData.published_at) {
      date = String(releaseData.published_at).slice(0, 10);
    }
    var label = t("version.label");
    line.innerHTML =
      label +
      ' <span class="ver-num">' +
      escapeHtml(tag) +
      "</span>" +
      (date ? " · " + escapeHtml(date) : "");
    line.hidden = false;
  }

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return {
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      }[c];
    });
  }

  function fetchVersion() {
    if (typeof fetch !== "function") return;
    fetch("https://api.github.com/repos/fanhefeng/portreaper/releases/latest", {
      headers: { Accept: "application/vnd.github+json" },
    })
      .then(function (r) {
        if (!r.ok) throw new Error("status " + r.status);
        return r.json();
      })
      .then(function (data) {
        if (data && data.tag_name) {
          releaseData = data;
          renderVersion();
        }
      })
      .catch(function () {
        /* silent failure — version line just stays hidden */
      });
  }

  /* ───── Install tabs ───── */

  function bindTabs() {
    var tabs = [
      { tab: "tab-mac", panel: "panel-mac" },
      { tab: "tab-win", panel: "panel-win" },
    ];

    function select(idx) {
      for (var i = 0; i < tabs.length; i++) {
        var tabEl = document.getElementById(tabs[i].tab);
        var panelEl = document.getElementById(tabs[i].panel);
        if (!tabEl || !panelEl) continue;
        var active = i === idx;
        tabEl.classList.toggle("is-active", active);
        tabEl.setAttribute("aria-selected", active ? "true" : "false");
        tabEl.tabIndex = active ? 0 : -1;
        panelEl.classList.toggle("is-active", active);
        panelEl.hidden = !active;
      }
    }

    for (var i = 0; i < tabs.length; i++) {
      (function (idx) {
        var el = document.getElementById(tabs[idx].tab);
        if (!el) return;
        el.addEventListener("click", function () {
          select(idx);
        });
        el.addEventListener("keydown", function (e) {
          if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
            e.preventDefault();
            var next =
              e.key === "ArrowRight"
                ? (idx + 1) % tabs.length
                : (idx - 1 + tabs.length) % tabs.length;
            select(next);
            var nextEl = document.getElementById(tabs[next].tab);
            if (nextEl) nextEl.focus();
          }
        });
      })(i);
    }
  }

  /* ───── Copy buttons ───── */

  function bindCopy() {
    var btns = document.querySelectorAll(".copy-btn[data-copy-target]");
    for (var i = 0; i < btns.length; i++) {
      btns[i].addEventListener("click", function () {
        var btn = this;
        var target = document.getElementById(btn.getAttribute("data-copy-target"));
        if (!target) return;
        var text = target.textContent || "";
        var done = function () {
          var original = t("install.copy");
          btn.textContent = t("copy.done");
          btn.classList.add("is-copied");
          setTimeout(function () {
            btn.textContent = original;
            btn.classList.remove("is-copied");
          }, 1600);
        };
        if (navigator.clipboard && navigator.clipboard.writeText) {
          navigator.clipboard.writeText(text).then(done, function () {
            legacyCopy(text, done);
          });
        } else {
          legacyCopy(text, done);
        }
      });
    }
  }

  function legacyCopy(text, done) {
    var ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    try {
      ta.select();
      // execCommand 失败时返回 false 而不是抛错 —— 不看返回值就会在什么都没
      // 复制的情况下弹出「已复制」，用户去粘贴才发现是空的
      var ok = document.execCommand("copy");
      if (ok) done();
    } catch {
      /* ignore */
    } finally {
      // 清理放 finally：execCommand 抛错时也不能把隐藏 textarea 留在 DOM 里
      document.body.removeChild(ta);
    }
  }

  /* ───── Init ───── */

  function init() {
    applyLang(currentLang);
    bindLangToggle();
    highlightDownload();
    bindTabs();
    bindCopy();
    fetchVersion();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
