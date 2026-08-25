//! The dashboard's embedded stylesheet (U4). One dark cockpit token sheet; every class
//! `g-`-prefixed so page chrome and panel widgets can never collide (KTD15).

pub const CSS: &str = r##"
/* grind serve — cockpit stylesheet. Token sheet verbatim from the approved
   mockups (KTD15); every class is g--prefixed so page chrome and panel
   widgets never collide. */

/* ── tokens & canvas ────────────────────────────────────── */
:root{
  --bg:#0b0e14; --panel:#11151c; --panel2:#151a22; --inset:#080b10;
  --hair:#1c232e; --edge:#2a3340;
  --text:#dbe4ee; --dim:#7d8590; --faint:#565e68;
  --live:#3fb950; --hold:#d29922; --dead:#f85149; --cyan:#79c0ff;
  color-scheme:dark;
}
*{box-sizing:border-box}
html,body{margin:0;color:var(--text);
  font:12px/1.45 ui-monospace,"SF Mono",Menlo,Consolas,monospace;
  font-variant-numeric:tabular-nums;}
body{background:var(--bg);
  background-image:radial-gradient(1100px 500px at 50% -8%, #111827 0%, var(--bg) 60%);}
/* phosphor scanlines */
body::after{content:"";position:fixed;inset:0;pointer-events:none;z-index:99;
  background:repeating-linear-gradient(0deg,rgba(255,255,255,.016) 0 1px,transparent 1px 3px);}
.g-wrap{max-width:1010px;margin:0 auto;padding:0 16px;}

/* glow utilities — restrained phosphor on state values */
.g-glow-live{text-shadow:0 0 9px rgba(63,185,80,.5);}
.g-glow-cyan{text-shadow:0 0 8px rgba(121,192,255,.35);}

/* ── command bar ────────────────────────────────────────── */
.g-bar{position:relative;z-index:10;display:flex;align-items:center;height:38px;
  border-bottom:1px solid var(--edge);background:var(--panel);}
.g-bar .g-seg{display:flex;align-items:center;gap:7px;padding:0 13px;height:100%;
  border-right:1px solid var(--hair);white-space:nowrap;font-size:11.5px;}
.g-brand{color:var(--live);font-weight:700;letter-spacing:.04em;
  text-shadow:0 0 10px rgba(63,185,80,.55);}
.g-back{color:var(--cyan);}
.g-faint{color:var(--faint);} .g-dim{color:var(--dim);}
.g-seg b{color:var(--text);font-weight:600;}
#clk{margin-left:auto;border-right:none;border-left:1px solid var(--hair);color:#b9d2ee;
  text-shadow:0 0 8px rgba(121,192,255,.35);}
.g-led{width:7px;height:7px;border-radius:50%;background:var(--live);
  box-shadow:0 0 7px rgba(63,185,80,.9);}
@keyframes g-blink{50%{opacity:.2}}
.g-blink{animation:g-blink 1.6s steps(1) infinite;}

/* ── telemetry strip ────────────────────────────────────── */
.g-tele{display:flex;border-bottom:1px solid var(--edge);background:rgba(13,17,25,.9);}
.g-tseg{padding:7px 13px;border-right:1px solid var(--hair);min-width:96px;}
.g-tlab{font-size:9px;letter-spacing:.14em;color:var(--faint);text-transform:uppercase;}
.g-tval{margin-top:2px;font-size:15px;font-weight:650;}
.g-tval.g{color:var(--live);text-shadow:0 0 9px rgba(63,185,80,.5);}
.g-tval.a{color:var(--hold);text-shadow:0 0 9px rgba(210,153,34,.45);}
.g-tval.n{color:var(--text);}
.g-tsub{font-size:10px;color:var(--dim);margin-top:1px;}
.g-meter{display:inline-block;width:86px;height:4px;background:var(--hair);
  border-radius:2px;vertical-align:2px;margin-left:8px;overflow:hidden;}
.g-meter i{display:block;height:100%;background:linear-gradient(90deg,#23864a,var(--live));
  box-shadow:0 0 6px rgba(63,185,80,.6);}

/* ── swimlane rails ─────────────────────────────────────── */
.g-lane{display:grid;grid-template-columns:118px 1fr;border-bottom:1px solid var(--hair);}
.g-rail{border-left:2px solid var(--edge);padding:12px 0 12px 12px;}
.g-rail.hold{border-color:var(--hold);} .g-rail.go{border-color:var(--live);}
.g-rail.stop{border-color:var(--dead);} .g-rail.done{border-color:#484f58;}
.g-rail .g-nm{font-size:9.5px;letter-spacing:.16em;text-transform:uppercase;font-weight:700;}
.g-rail.hold .g-nm{color:var(--hold);text-shadow:0 0 8px rgba(210,153,34,.35);}
.g-rail.go .g-nm{color:var(--live);text-shadow:0 0 8px rgba(63,185,80,.35);}
.g-rail.stop .g-nm{color:var(--dead);}
.g-rail.done .g-nm{color:var(--dim);}
.g-rail .g-wip{margin-top:4px;color:var(--faint);font-size:10.5px;}

/* ── board columns ──────────────────────────────────────── */
.g-board{display:flex;gap:10px;padding:10px 0 12px 14px;align-items:start;overflow:hidden;}
.g-col{flex:1;min-width:0;}
.g-colh{display:flex;align-items:center;gap:6px;margin-bottom:7px;
  font-size:9.5px;letter-spacing:.12em;text-transform:uppercase;color:var(--dim);}
.g-colh .g-c{background:#1a212b;border:1px solid var(--hair);border-radius:8px;
  padding:0 6px;font-size:9.5px;line-height:15px;color:var(--text);}
.g-slots{display:flex;flex-direction:column;gap:8px;}
.g-empty{border:1px dashed #1a212b;border-radius:5px;min-height:44px;}

/* ── cards ──────────────────────────────────────────────── */
.g-card{position:relative;background:linear-gradient(180deg,#131824 0%,#11151c 100%);
  border:1px solid var(--hair);border-radius:5px;padding:9px 11px 9px 13px;}
.g-card::before{content:"";position:absolute;left:0;top:0;bottom:0;width:2px;
  background:var(--edge);border-radius:5px 0 0 5px;}
.g-card.g::before{background:linear-gradient(180deg,var(--live),#1d5f38,var(--live));
  background-size:100% 220%;animation:g-flow 3.2s linear infinite;}
@keyframes g-flow{to{background-position:0 220%}}
.g-card.a::before{background:var(--hold);}
.g-card.r::before{background:repeating-linear-gradient(135deg,var(--dead) 0 5px,#7e2d26 5px 10px);}
.g-card.k::before{background:#3a414b;}
.g-card:hover{border-color:var(--edge);}
.g-cursor{outline:1px solid rgba(121,192,255,.65);outline-offset:0;
  box-shadow:0 0 0 1px rgba(121,192,255,.25), 0 0 18px rgba(121,192,255,.12);}
.g-l1{display:flex;gap:8px;align-items:baseline;}
.g-rid{color:var(--faint);font-size:10px;white-space:nowrap;overflow:hidden;
  text-overflow:ellipsis;}
.g-age{margin-left:auto;color:var(--faint);font-size:10px;flex:none;}
.g-l2{display:flex;gap:8px;align-items:center;margin-top:3px;}
.g-title{font-size:12px;font-weight:600;white-space:nowrap;overflow:hidden;
  text-overflow:ellipsis;}
.g-st{flex:none;display:inline-flex;align-items:center;gap:5px;font-size:10px;font-weight:600;}
.g-dot{width:6px;height:6px;border-radius:50%;flex:none;}
.g-dot.g{background:var(--live);box-shadow:0 0 6px rgba(63,185,80,.8);}
.g-dot.a{background:var(--hold);} .g-dot.r{background:var(--dead);}
.g-dot.k{background:#484f58;}
.g-dot.r.g-dim{opacity:.35;}
.g-h1 .g-dot{width:7px;height:7px;background:var(--live);
  box-shadow:0 0 7px rgba(63,185,80,.85);}
@keyframes g-pulse{0%,100%{box-shadow:0 0 0 0 rgba(63,185,80,.5)}
  50%{box-shadow:0 0 0 5px rgba(63,185,80,0)}}
.g-pulse{animation:g-pulse 2s ease-in-out infinite;}

/* tinted state badges — data wearing color, not a verdict */
.g-badge{padding:0 8px;line-height:17px;border-radius:99px;font-size:10px;}
.g-b-run{color:var(--live);background:rgba(63,185,80,.13);border:1px solid rgba(63,185,80,.35);}
.g-b-hold{color:var(--hold);background:rgba(210,153,34,.13);border:1px solid rgba(210,153,34,.35);}
.g-b-dead{color:var(--dead);background:rgba(248,81,73,.11);border:1px solid rgba(248,81,73,.32);}
.g-b-done{color:var(--dim);background:rgba(125,133,144,.1);border:1px solid rgba(125,133,144,.3);}
.g-l3{margin-top:6px;display:flex;gap:11px;color:var(--dim);font-size:10.5px;flex-wrap:wrap;}
.g-link{color:var(--cyan);text-decoration:none;opacity:.85;}
.g-budget i{display:inline-block;width:5px;height:5px;border-radius:1px;margin-right:1px;}
.g-budget .g-f{background:var(--live);box-shadow:0 0 3px rgba(63,185,80,.5);}
.g-budget .g-e{border:1px solid var(--edge);}
.g-l4,.g-prev{margin-top:7px;padding-top:6px;border-top:1px dashed var(--hair);color:#9aa7b5;
  font-size:10.5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;}
.g-l4::before{content:"… ";color:var(--faint);}
.g-quiet{color:var(--faint);font-style:normal;margin-left:6px;}

/* ── run head ───────────────────────────────────────────── */
.g-head{position:relative;z-index:1;background:rgba(17,21,28,.9);
  border-bottom:1px solid var(--edge);}
.g-h1{display:flex;align-items:center;gap:12px;padding:13px 0 4px;flex-wrap:wrap;}
.g-h1 .g-t{font-size:15px;font-weight:650;}
.g-fresh{color:var(--dim);font-size:11px;}
.g-fresh b{color:var(--live);text-shadow:0 0 7px rgba(63,185,80,.5);}
.g-spec{display:flex;gap:20px;flex-wrap:wrap;padding:7px 0 12px;font-size:11px;
  color:var(--dim);}
.g-spec b{color:var(--text);font-weight:600;}

/* ── panels & grid ──────────────────────────────────────── */
.g-grid{display:grid;grid-template-columns:1fr 396px;gap:12px;padding:12px 0;
  align-items:start;}
.g-panel{background:linear-gradient(180deg,#12171f,#11151c);border:1px solid var(--hair);
  border-radius:6px;overflow:hidden;}
.g-phead{display:flex;align-items:center;padding:7px 12px;border-bottom:1px solid var(--hair);
  font-size:9.5px;letter-spacing:.14em;text-transform:uppercase;color:var(--dim);
  background:rgba(21,26,34,.8);}
.g-phead .g-r{margin-left:auto;text-transform:none;letter-spacing:0;font-size:10px;}
.g-follow{color:var(--live);}

/* ── waterfall track ────────────────────────────────────── */
.g-wf{padding:12px 14px 8px;}
.g-axis{display:flex;justify-content:space-between;color:var(--faint);font-size:9px;
  margin-top:5px;letter-spacing:.03em;}
.g-track{position:relative;height:56px;background:var(--inset);border:1px solid var(--hair);
  border-radius:4px;overflow:hidden;margin-right:26px;}
.g-gridline{position:absolute;top:0;bottom:0;width:1px;background:var(--hair);opacity:.55;}
.g-wbar{position:absolute;top:8px;height:22px;border-radius:3px;font-size:9px;
  line-height:22px;padding:0 6px;white-space:nowrap;overflow:hidden;font-weight:600;}
.g-wbar.dead{background:linear-gradient(180deg,rgba(248,81,73,.85),rgba(248,81,73,.55));
  color:#2a0503;}
.g-wbar.amber{background:linear-gradient(180deg,rgba(210,153,34,.9),rgba(210,153,34,.55));
  color:#241703;}
.g-sleep{position:absolute;top:8px;height:22px;border-radius:3px;
  background:repeating-linear-gradient(135deg,rgba(125,133,144,.22) 0 4px,transparent 4px 9px);
  border:1px dashed #333c48;color:var(--dim);font-size:9px;line-height:20px;padding:0 6px;}
.g-wbar.g-livebar{left:0;right:0;width:auto;color:#03170a;
  background:linear-gradient(90deg,rgba(63,185,80,.95),rgba(63,185,80,.55));
  background-size:200% 100%;animation:g-sweep 2.4s linear infinite;}
@keyframes g-sweep{to{background-position:-200% 0}}
.g-now{position:absolute;top:0;bottom:0;width:0;border-left:1px solid var(--cyan);
  box-shadow:0 0 6px rgba(121,192,255,.7);}
.g-nowlbl{position:absolute;top:2px;color:var(--cyan);font-size:8.5px;letter-spacing:.08em;}
.g-wl{margin:8px 2px 2px;display:flex;gap:16px;font-size:9.5px;color:var(--dim);
  flex-wrap:wrap;}
.g-wl i{display:inline-block;width:8px;height:8px;border-radius:2px;margin-right:5px;
  vertical-align:-1px;}

/* ── attempts ───────────────────────────────────────────── */
.g-a{padding:9px 12px;border-top:1px solid var(--hair);}
.g-aline{display:flex;gap:10px;align-items:baseline;}
.g-idx{color:var(--faint);width:24px;flex:none;}
.g-verdict{font-weight:600;font-size:11.5px;}
.g-v-live{color:var(--live);} .g-v-dead{color:var(--dead);} .g-v-amber{color:var(--hold);}
.g-dur{margin-left:auto;color:var(--faint);font-size:10.5px;}
.g-asub{margin:3px 0 0 34px;color:var(--dim);font-size:10.5px;}
.g-stage{color:#9aa7b5;}
.g-ev{margin:5px 0 0 34px;font-size:10.5px;}
.g-ev span{margin-right:12px;}

/* ── log pane & jump pill ───────────────────────────────── */
.g-log{background:var(--inset);height:396px;padding:10px 0;overflow-y:auto;
  overflow-x:hidden;font-size:11px;line-height:1.62;}
.g-log .g-l{padding:0 12px;white-space:pre-wrap;}
.g-log.g-nowrap .g-l{white-space:pre;}
.g-l .g-ts{color:#414a56;}
.g-l.sys{color:var(--dim);}
.g-l.warn{color:var(--hold);text-shadow:0 0 7px rgba(210,153,34,.25);}
.g-hl{color:var(--text);}
.g-jump{position:relative;}
.g-pill{position:absolute;bottom:12px;left:50%;transform:translateX(-50%);
  background:var(--live);color:#04140a;font-weight:700;font-size:10px;cursor:pointer;
  padding:3px 12px;border-radius:99px;box-shadow:0 0 12px rgba(63,185,80,.4);}

/* ── status bar ─────────────────────────────────────────── */
.g-status{position:sticky;bottom:0;display:flex;height:26px;align-items:center;
  background:rgba(21,26,34,.96);border-top:1px solid var(--edge);font-size:10.5px;
  color:var(--dim);}
.g-status .g-seg2{padding:0 12px;border-right:1px solid var(--hair);height:100%;
  display:flex;align-items:center;gap:6px;white-space:nowrap;}
.g-k{color:var(--text);background:#1f2833;border:1px solid var(--edge);border-radius:3px;
  padding:0 4px;font-size:9.5px;line-height:14px;}
.g-spacer{flex:1;} .g-ok{color:var(--live);}

/* motion off when the operator asks for calm (KTD12/KTD15) */
@media (prefers-reduced-motion:reduce){
  body::after{display:none}
  .g-pulse,.g-blink,.g-card.g::before,.g-wbar.g-livebar{animation:none}
}
"##;

#[cfg(test)]
mod tests {
    use super::CSS;

    /// True when `prelude` (the text before a `{`) names at least one class selector.
    fn names_a_class(prelude: &str) -> bool {
        let b = prelude.as_bytes();
        (0..b.len().saturating_sub(1)).any(|i| b[i] == b'.' && b[i + 1].is_ascii_alphabetic())
    }

    /// Given the index of `{` in `s`, return `(inner, index-after-matching-})`.
    fn balanced(s: &str, open: usize) -> (&str, usize) {
        let b = s.as_bytes();
        debug_assert_eq!(b[open], b'{');
        let mut depth = 0usize;
        let mut i = open;
        while i < b.len() {
            match b[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return (&s[open + 1..i], i + 1);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        panic!("unbalanced braces in stylesheet");
    }

    /// Walk every rule: a selector that names any class must carry a `g-` class,
    /// and `@keyframes` names must start with `g-`. Element/`:root`/`#id` rules are exempt.
    fn audit(sheet: &str) {
        let mut tok = String::new();
        let mut i = 0usize;
        while let Some(c) = sheet[i..].chars().next() {
            match c {
                '{' => {
                    let head = std::mem::take(&mut tok);
                    let h = head.trim();
                    if let Some(name) = h.strip_prefix("@keyframes") {
                        assert!(
                            name.trim().starts_with("g-"),
                            "@keyframes name `{}` lacks the g- prefix",
                            name.trim()
                        );
                    } else if h.starts_with('@') {
                        let (inner, next) = balanced(sheet, i);
                        audit(inner);
                        i = next;
                        continue;
                    } else if names_a_class(h) {
                        assert!(h.contains(".g-"), "selector `{}` lacks a g- class", h);
                    }
                    let (_, next) = balanced(sheet, i);
                    i = next;
                    continue;
                }
                _ => tok.push(c),
            }
            i += c.len_utf8();
        }
    }

    #[test]
    fn carries_the_token_sheet_verbatim() {
        for tok in [
            "#0b0e14", "#11151c", "#151a22", "#080b10", "#1c232e", "#2a3340", "#dbe4ee", "#7d8590",
            "#565e68", "#3fb950", "#d29922", "#f85149", "#79c0ff",
        ] {
            assert!(CSS.contains(tok), "token {tok} missing");
        }
        assert!(
            CSS.contains("ui-monospace"),
            "full-monospace instrumentation"
        );
        assert!(CSS.contains("tabular-nums"), "numbers render tabular");
    }

    #[test]
    fn honors_prefers_reduced_motion() {
        let pos = CSS
            .find("@media (prefers-reduced-motion")
            .expect("reduced-motion block");
        let blk = &CSS[pos..];
        for host in [
            ".g-pulse",
            ".g-blink",
            ".g-card.g::before",
            ".g-wbar.g-livebar",
        ] {
            assert!(blk.contains(host), "reduced-motion must name {host}");
        }
        assert!(
            blk.contains("animation:none"),
            "reduced-motion must stop animation"
        );
        assert!(
            blk.contains("body::after"),
            "scanlines off under reduced motion"
        );
    }

    #[test]
    fn every_class_selector_is_g_namespaced() {
        audit(CSS);
    }
}
