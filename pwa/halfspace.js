// Port of apply_persian_halfspace (src-tauri/src/audio_toolkit/text.rs).
// Conservative ZWNJ pass: merges می/نمی prefixes, the plural ها and the
// comparative تر/ترین onto the previous word. Only single-space joins.
(function (global) {
  var ZWNJ = "\u200c";
  var FA = /[\u0621-\u064A\u06F0-\u06F9]/;

  function isFa(ch) {
    return ch !== undefined && FA.test(ch);
  }

  function applyPersianHalfspace(text) {
    var words = text.split(" ");
    var out = [];
    var i = 0;
    while (i < words.length) {
      var w = words[i];
      if (i + 1 < words.length) {
        var next = words[i + 1];
        var wChars = Array.from(w);
        var merged = null;
        if ((w === "می" || w === "نمی") && isFa(Array.from(next)[0])) {
          merged = w + ZWNJ + next;
        } else if (next === "ها" && isFa(wChars[wChars.length - 1])) {
          merged = w + ZWNJ + "ها";
        } else if (
          (next === "تر" || next === "ترین") &&
          isFa(wChars[wChars.length - 1]) &&
          wChars.length > 2
        ) {
          merged = w + ZWNJ + next;
        }
        if (merged !== null) {
          out.push(merged);
          i += 2;
          continue;
        }
      }
      out.push(w);
      i += 1;
    }
    return out.join(" ");
  }

  global.applyPersianHalfspace = applyPersianHalfspace;
})(typeof window !== "undefined" ? window : globalThis);
