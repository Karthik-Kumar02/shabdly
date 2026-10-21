// The pill at the bottom of the screen: mic level while recording, pulsing dots
// while processing, short notices, and the "Learned" toast with Undo.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { SPRING, SPRING_MS, enter } from "./motion";

type OverlayState = { state: "recording" | "processing" | "warming" | "error" | "hidden"; level?: number; message?: string | null };
type LearnEvent = { kind: "added" | "heard_added" | "hint_only" | "promoted"; word: string; heard: string };

const pill = document.getElementById("pill")!;
const BARS = 5;
const DOTS = `<span class="dots"><i></i><i></i><i></i></span>`;
let hideTimer: number | undefined;
let toastTimer: number | undefined;
let exit: Animation | undefined;

// A visible pill morphs to its new width; a hidden one pops in (pillin in style.css).
function show(html: string, cls: string) {
  window.clearTimeout(hideTimer);
  exit?.cancel();
  const wasShown = !pill.hidden;
  const from = pill.getBoundingClientRect().width;
  pill.className = `pill ${cls}`;
  pill.innerHTML = html;
  pill.hidden = false;
  if (!wasShown) return;
  const to = pill.getBoundingClientRect().width;
  if (from !== to) pill.animate({ width: [`${from}px`, `${to}px`] }, { duration: SPRING_MS, easing: SPRING });
  for (const child of pill.children) enter(child, 0);
}

function hideSoon(ms: number) {
  window.clearTimeout(hideTimer);
  hideTimer = window.setTimeout(() => {
    exit = pill.animate({ opacity: [1, 0], transform: ["none", "scale(0.9)"] }, { duration: 160, easing: "ease-in" });
    exit.onfinish = () => {
      pill.hidden = true;
      invoke("overlay_interactive", { on: false });
    };
  }, ms);
}

// Level updates arrive many times a second: move the existing bars so their transition glides.
function setBars(level: number) {
  const els = pill.querySelectorAll<HTMLElement>(".recording .bar");
  if (pill.hidden || els.length !== BARS) return show(`<span class="bars">${bars(level)}</span>`, "recording");
  window.clearTimeout(hideTimer);
  exit?.cancel();
  scales(level).forEach((k, i) => (els[i].style.transform = `scaleY(${k})`));
}

function scales(level: number) {
  // Speech RMS is small; scale so normal talking fills most of the bar.
  const h = Math.min(1, Math.sqrt(level) * 3);
  return Array.from({ length: BARS }, (_, i) => (0.35 + 0.65 * h * (1 - Math.abs(i - 2) / 3)).toFixed(2));
}

function bars(level: number) {
  return scales(level).map((k) => `<span class="bar" style="transform: scaleY(${k})"></span>`).join("");
}

function escapeHtml(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

listen<OverlayState>("overlay-state", ({ payload }) => {
  window.clearTimeout(toastTimer);
  switch (payload.state) {
    case "recording":
      setBars(payload.level ?? 0);
      break;
    case "processing":
      show(DOTS, "busy");
      break;
    case "warming":
      show(`${DOTS}<span class="text">Warming up</span>`, "busy");
      break;
    case "error":
      show(`<span class="text">${escapeHtml(payload.message ?? "Something went wrong")}</span>`, "notice");
      hideSoon(2500);
      break;
    case "hidden":
      hideSoon(150);
      break;
  }
});

listen<{ events: LearnEvent[] }>("learned", ({ payload }) => {
  const shown = payload.events.filter((e) => e.kind !== "hint_only");
  if (!shown.length) return;
  const words = [...new Set(shown.map((e) => e.word))].join(", ");
  show(`<span class="text">Learned: ${escapeHtml(words)}</span><button id="undo">Undo</button>`, "toast");
  invoke("overlay_interactive", { on: true });
  document.getElementById("undo")!.onclick = async () => {
    for (const e of shown) await invoke("undo_learn", { event: e });
    show(`<span class="text">Undone</span>`, "notice");
    hideSoon(1200);
  };
  toastTimer = window.setTimeout(() => hideSoon(0), 6000);
});
