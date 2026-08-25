//! The dashboard's embedded client script (U4). Poll / tick / follow / keys / pause,
//! plain ES, no build; ~150-line budget is a ruling (KTD5).

pub const JS: &str = r##"
/* grind serve — embedded client. Plain ES, no build (KTD5).
   Five modules: poller · ticker · log follower · keyboard · pause/stamp.
   It only ever GETs fragments and deltas — read-only by construction (ADR-0013). */
(function () {
"use strict";
function $(sel, root) { return (root || document).querySelector(sel); }
function $all(sel, root) {
  return Array.prototype.slice.call((root || document).querySelectorAll(sel));
}
function runId() {
  var m = location.pathname.match(/^\/runs\/([^\/]+)/);
  return m ? decodeURIComponent(m[1]) : null;
}

/* ── poller: self-rescheduling setTimeout, AbortController per tick ── */
var gap = 1000;              /* doubles on error, cap 30 s, resets on success */
var timer = null;
var inflight = null;         /* AbortController for the tick in the air */
var paused = false;

function cadence() {
  var root = $('[data-g-root="board"],[data-g-root="run"]');
  return root && root.getAttribute("data-live") === "true" ? 2000 : 10000;
}
function schedule() {
  clearTimeout(timer);
  timer = setTimeout(tick, Math.min(cadence() * (gap / 1000), 30000));
}
function fragUrl() {
  if ($('[data-g-root="board"]')) return "/f/roster";
  if ($('[data-g-root="run"]') && runId())
    return "/f/runs/" + encodeURIComponent(runId());
  return null;
}
function tick() {
  if (paused || document.hidden) { schedule(); return; }
  var url = fragUrl();
  if (!url) return;
  inflight = new AbortController();
  var ctl = inflight;
  fetch(url, { signal: ctl.signal }).then(function (r) {
    if (!r.ok) throw new Error("http " + r.status);
    return r.text();
  }).then(function (html) {
    if (ctl.signal.aborted) return;
    swap(html);                    /* replacement built before any mutation */
    stamp();
    gap = 1000;
    follow();                      /* log deltas ride the same beat */
    schedule();
  }, function () {
    if (ctl.signal.aborted) return;/* superseded mid-air, not an outage */
    gap = Math.min(gap * 2, 30000);
    schedule();
  });
}
function swap(html) {
  var tpl = document.createElement("template");
  tpl.innerHTML = html;
  var fresh = tpl.content.firstElementChild;
  var cur = fresh && $('[data-g-root="' + fresh.getAttribute("data-g-root") + '"]');
  if (!cur) return;
  /* the log pane is stateful (offset + held lines): carry it across swaps */
  var oldLog = $('[data-g-root="log"]', cur);
  var newLog = oldLog && $('[data-g-root="log"]', fresh);
  if (newLog) newLog.replaceWith(oldLog);
  cur.replaceWith(fresh);
}
function stamp() {
  var s = $("[data-g-stamp]");
  if (s) s.setAttribute("data-epoch", String(Math.floor(Date.now() / 1000)));
}

/* ── ticker: ages + UTC clock, one interval, write only on change ── */
function ago(sec) {
  sec = Math.max(0, Math.floor(sec));
  if (sec < 60) return sec + "s";
  if (sec < 3600) return Math.floor(sec / 60) + "m";
  if (sec < 86400) return Math.floor(sec / 3600) + "h";
  return Math.floor(sec / 86400) + "d";
}
setInterval(function () {
  var now = Date.now() / 1000;
  $all("[data-epoch]").forEach(function (el) {
    var txt = ago(now - parseFloat(el.getAttribute("data-epoch"))) + " ago";
    if (el.hasAttribute("data-g-stamp")) txt = "updated " + txt;
    if (el.textContent !== txt) el.textContent = txt;
  });
  var clk = document.getElementById("clk");
  if (clk) clk.textContent = new Date().toISOString().slice(11, 19) + " UTC";
}, 1000);

/* ── log follower: byte-offset deltas, buffer while scrolled up ── */
var logOffset = 0;
var logReady = false;
var held = [];                 /* lines buffered while the reader scrolled up */
var following = true;
var logBox = null;

function follow() {
  var root = $('[data-g-root="log"]');
  if (!root || !runId()) return;
  var box = $(".g-log", root) || root;
  logBox = box;
  if (!logReady) {
    logOffset = parseInt(box.getAttribute("data-offset"), 10) || 0;
    logReady = true;
  }
  fetch("/f/runs/" + encodeURIComponent(runId()) + "/log?o=" + logOffset)
    .then(function (r) {
      if (!r.ok) throw new Error("http " + r.status);
      var offHdr = r.headers.get("X-New-Offset");   /* server owns the byte math */
      var reset = r.headers.get("X-Log-Reset") === "1";
      return r.text().then(function (html) {
        if (offHdr !== null) logOffset = parseInt(offHdr, 10);
        apply(box, html, reset);
      });
    })
    .catch(function () {});                          /* the next beat retries */
}
function apply(box, html, reset) {
  var tpl = document.createElement("template");
  tpl.innerHTML = html;
  var nodes = Array.prototype.slice.call(tpl.content.childNodes);
  var pinned = box.scrollHeight - box.scrollTop - box.clientHeight < 24;
  if (reset) {                     /* truncated upstream: replace wholesale */
    box.textContent = "";
    held = [];
    nodes.forEach(function (n) { box.appendChild(n); });
  } else if (pinned && !held.length) {
    nodes.forEach(function (n) { box.appendChild(n); });
  } else {
    held = held.concat(nodes);     /* scrolled up: buffer, never drop */
  }
  pill(held.length);
  if (following && (pinned || reset)) box.scrollTop = box.scrollHeight;
}
function pill(n) {
  var p = $(".g-pill");
  if (!p) return;
  p.style.display = n ? "" : "none";
  if (n) p.textContent = "\u2193 " + n + " new";
}
function release(box) {
  held.forEach(function (n) { box.appendChild(n); });
  held = [];
  pill(0);
  if (following) box.scrollTop = box.scrollHeight;
}

/* ── keyboard: j/k cursor, enter open, esc back, log helpers ── */
function cards() { return $all("[data-gid]"); }
function at(list) {
  for (var i = 0; i < list.length; i++)
    if (list[i].classList.contains("g-cursor")) return i;
  return -1;
}
function move(list, to) {
  if (to < 0 || to >= list.length) return;
  var from = at(list);
  if (from >= 0) list[from].classList.remove("g-cursor");
  list[to].classList.add("g-cursor");
  list[to].scrollIntoView({ block: "nearest" });
}
document.addEventListener("keydown", function (ev) {
  var t = ev.target;
  if (ev.metaKey || ev.ctrlKey || ev.altKey) return;
  if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" ||
            t.isContentEditable)) return;
  var log = $('[data-g-root="log"] .g-log');
  if (ev.key === "End" && log) { log.scrollTop = log.scrollHeight; ev.preventDefault(); return; }
  if (ev.key === "Home" && log) { log.scrollTop = 0; ev.preventDefault(); return; }
  if (ev.key === "w" && log) { log.classList.toggle("g-nowrap"); return; }
  if (ev.key === "f") { following = !following; if (following && logBox) release(logBox); return; }
  if (ev.key === "Escape") { history.back(); return; }
  var list = cards(), i = at(list);
  if (ev.key === "j") move(list, i < 0 ? 0 : Math.min(i + 1, list.length - 1));
  else if (ev.key === "k") move(list, i < 0 ? 0 : Math.max(i - 1, 0));
  else if (ev.key === "Enter" && i >= 0)
    location.href = "/runs/" + encodeURIComponent(list[i].getAttribute("data-gid"));
});

/* ── pause/stamp: hidden-tab gate, Pause toggle, scroll wiring ── */
document.addEventListener("visibilitychange", function () {
  if (document.hidden) {           /* hidden: stop the beat, cancel in-flight */
    clearTimeout(timer);
    if (inflight) inflight.abort();
  } else {                         /* back: one immediate refresh */
    gap = 1000;
    tick();
  }
});
document.addEventListener("click", function (ev) {
  var btn = ev.target.closest("[data-g-pause]");
  if (btn) {
    paused = !paused;
    btn.textContent = paused ? "resume" : "pause";
    if (!paused) tick();
    return;
  }
  if (logBox && ev.target.closest(".g-pill")) release(logBox);
});
document.addEventListener("scroll", function (ev) {
  if (logBox && ev.target === logBox &&
      logBox.scrollHeight - logBox.scrollTop - logBox.clientHeight < 24)
    release(logBox);
}, true);

tick();
follow();
})();
"##;

#[cfg(test)]
mod tests {
    use super::JS;

    #[test]
    fn the_client_pauses_when_hidden_and_cancels_in_flight() {
        assert!(
            JS.contains("visibilitychange"),
            "hidden-tab gate is load-bearing"
        );
        assert!(
            JS.contains("AbortController"),
            "each tick must be cancellable"
        );
    }

    #[test]
    fn the_log_follows_the_server_owned_offset() {
        assert!(
            JS.contains("X-New-Offset"),
            "offset comes from the server header"
        );
        assert!(
            JS.contains("X-Log-Reset"),
            "shrink must replace, not append"
        );
    }

    #[test]
    fn polling_self_reschedules_and_never_intervals() {
        assert!(JS.contains("setTimeout"));
        let beats = JS.matches("setInterval").count();
        assert_eq!(beats, 1, "exactly one interval (the ticker), found {beats}");
    }
}
