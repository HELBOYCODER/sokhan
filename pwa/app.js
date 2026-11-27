(function () {
  "use strict";

  var $ = function (id) {
    return document.getElementById(id);
  };
  var textEl = $("text");
  var micEl = $("mic");
  var statusEl = $("status");
  var bannerEl = $("banner");
  var langEl = $("lang");
  var halfspaceEl = $("halfspace");

  var SR = window.SpeechRecognition || window.webkitSpeechRecognition;
  var standalone =
    window.navigator.standalone === true ||
    window.matchMedia("(display-mode: standalone)").matches;

  // ---- settings -----------------------------------------------------------
  function load(key, fallback) {
    try {
      var v = localStorage.getItem(key);
      return v === null ? fallback : v;
    } catch (e) {
      return fallback;
    }
  }
  function save(key, value) {
    try {
      localStorage.setItem(key, value);
    } catch (e) {}
  }

  langEl.value = load("sokhan.lang", "fa-IR");
  halfspaceEl.checked = load("sokhan.halfspace", "1") === "1";
  textEl.value = load("sokhan.text", "");
  langEl.addEventListener("change", function () {
    save("sokhan.lang", langEl.value);
  });
  halfspaceEl.addEventListener("change", function () {
    save("sokhan.halfspace", halfspaceEl.checked ? "1" : "0");
  });
  textEl.addEventListener("input", function () {
    save("sokhan.text", textEl.value);
  });

  // ---- helpers ------------------------------------------------------------
  function setStatus(msg) {
    statusEl.textContent = msg;
  }
  function showBanner(msg) {
    bannerEl.textContent = msg;
    bannerEl.hidden = !msg;
  }
  function join(a, b) {
    if (!a) return b;
    if (!b) return a;
    return a + (/\s$/.test(a) ? "" : " ") + b;
  }
  function fmt(s) {
    s = s.trim();
    var lang = langEl.value;
    if (halfspaceEl.checked && lang === "fa-IR") {
      s = window.applyPersianHalfspace(s);
    }
    return s;
  }

  // ---- recognition --------------------------------------------------------
  var rec = null;
  var wantRecording = false;
  var fatal = false;
  var base = "";
  var sessionText = "";

  function render() {
    textEl.value = join(base, fmt(sessionText));
    textEl.scrollTop = textEl.scrollHeight;
  }

  function setRecordingUi(on) {
    micEl.classList.toggle("on", on);
    micEl.setAttribute("aria-label", on ? "توقف ضبط" : "شروع ضبط");
    textEl.readOnly = on;
    langEl.disabled = on;
    setStatus(on ? "در حال گوش دادن… (برای توقف بزنید)" : "آماده");
  }

  function createRecognition() {
    var r = new SR();
    r.lang = langEl.value;
    r.continuous = true;
    r.interimResults = true;
    r.maxAlternatives = 1;

    r.onresult = function (e) {
      var s = "";
      for (var k = 0; k < e.results.length; k++) {
        s += e.results[k][0].transcript;
      }
      sessionText = s;
      render();
    };

    r.onerror = function (e) {
      if (e.error === "no-speech" || e.error === "aborted") return;
      if (e.error === "not-allowed" || e.error === "service-not-allowed") {
        fatal = true;
        showBanner(
          standalone
            ? "دسترسی به تشخیص گفتار در حالت برنامهٔ نصب‌شده رد شد. اجازهٔ میکروفون و «دیکته» را در تنظیمات iOS بررسی کنید (Settings ← Keyboard ← Enable Dictation). اگر باز هم کار نکرد، همین صفحه را در Safari (نه از آیکون صفحه اصلی) باز کنید."
            : "اجازهٔ میکروفون یا دیکته داده نشد. در Safari اجازه را بدهید و Settings ← Keyboard ← Enable Dictation را روشن کنید."
        );
      } else if (e.error === "network") {
        showBanner("خطای شبکه: اتصال اینترنت را بررسی کنید.");
      } else {
        showBanner("خطا: " + e.error);
      }
    };

    r.onend = function () {
      // commit what this session produced
      base = join(base, fmt(sessionText));
      sessionText = "";
      textEl.value = base;
      save("sokhan.text", base);
      if (wantRecording && !fatal) {
        // continuous mode on iOS ends by itself after a pause: restart
        setTimeout(function () {
          if (!wantRecording) return;
          try {
            rec = createRecognition();
            rec.start();
          } catch (err) {
            stop();
          }
        }, 150);
      } else {
        wantRecording = false;
        setRecordingUi(false);
      }
    };
    return r;
  }

  function start() {
    if (!SR) return;
    showBanner("");
    fatal = false;
    wantRecording = true;
    base = textEl.value;
    sessionText = "";
    setRecordingUi(true);
    try {
      rec = createRecognition();
      rec.start();
    } catch (err) {
      wantRecording = false;
      setRecordingUi(false);
      showBanner("شروع ضبط ممکن نشد: " + err.message);
    }
  }

  function stop() {
    wantRecording = false;
    if (rec) {
      try {
        rec.stop();
      } catch (err) {}
    }
    setRecordingUi(false);
  }

  micEl.addEventListener("click", function () {
    if (wantRecording) stop();
    else start();
  });

  if (!SR) {
    micEl.disabled = true;
    setStatus("پشتیبانی نمی‌شود");
    showBanner(
      "این مرورگر تشخیص گفتار ندارد. در آیفون از Safari (iOS 14.5 به بالا) استفاده کنید."
    );
  }

  // ---- actions ------------------------------------------------------------
  $("copy").addEventListener("click", function () {
    var t = textEl.value;
    if (!t) return;
    var done = function () {
      setStatus("کپی شد");
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(t).then(done, function () {
        textEl.select();
        document.execCommand("copy");
        done();
      });
    } else {
      textEl.select();
      document.execCommand("copy");
      done();
    }
  });

  $("share").addEventListener("click", function () {
    var t = textEl.value;
    if (!t) return;
    if (navigator.share) {
      navigator.share({ text: t }).catch(function () {});
    } else {
      setStatus("اشتراک‌گذاری پشتیبانی نمی‌شود؛ از «کپی» استفاده کنید");
    }
  });

  $("clear").addEventListener("click", function () {
    if (wantRecording) stop();
    textEl.value = "";
    base = "";
    save("sokhan.text", "");
  });

  // ---- service worker -----------------------------------------------------
  if ("serviceWorker" in navigator) {
    window.addEventListener("load", function () {
      navigator.serviceWorker.register("sw.js").catch(function () {});
    });
  }
})();
