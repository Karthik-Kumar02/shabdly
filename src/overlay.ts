// The pill at the bottom of the screen: mic level while recording, a spinner
// while processing, short notices, and the "Learned" toast with Undo.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type OverlayState = { state: "recording" | "processing" | "warming" | "error" | "hidden"; level?: number; message?: string | null };
type LearnEvent = { kind: "added" | "heard_added" | "hint_only" | "promoted"; word: string; heard: string };

const pill = document.getElementById("pill")!;
const BARS = 5;
let hideTimer: number | undefined;
let toastTimer: number | undefined;

function show(html: string, cls: string) {
  window.clearTimeout(hideTimer);
  pill.className = `pill ${cls}`;
  pill.innerHTML = html;
  pill.hidden = false;
}

function hideSoon(ms: number) {
  window.clearTimeout(hideTimer);
  hideTimer = window.setTimeout(() => {
    pill.hidden = true;
    invoke("overlay_interactive", { on: false });
  }, ms);
}

function bars(level: number) {
  // Speech RMS is small; scale so normal talking fills most of the bar.
  const h = Math.min(1, Math.sqrt(level) * 3);
  return Array.from({ length: BARS }, (_, i) => {
    const k = 0.35 + 0.65 * h * (1 - Math.abs(i - 2) / 3);
    return `<span class="bar" style="transform: scaleY(${k.toFixed(2)})"></span>`;
  }).join("");
}

function escapeHtml(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

listen<OverlayState>("overlay-state", ({ payload }) => {
  window.clearTimeout(toastTimer);
  switch (payload.state) {
    case "recording":
      show(`<span class="dot"></span><span class="bars">${bars(payload.level ?? 0)}</span>`, "recording");
      break;
    case "processing":
      show(`<span class="spinner"></span>`, "busy");
      break;
    case "warming":
      show(`<span class="spinner"></span><span class="text">Warming up</span>`, "busy");
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
