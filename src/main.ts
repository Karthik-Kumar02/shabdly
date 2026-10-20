// Settings, Dictionary and History tabs. No framework: each tab re-renders
// itself from the backend after every change.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { GLOBE_WARNING, startOnboarding } from "./onboarding";

export type Settings = {
  version: number;
  language: "english" | "hindi" | "hinglish" | "auto";
  provider: "local" | "gemini_free" | "gemini_paid" | "haiku";
  hotkey: string;
  learn_from_corrections: boolean;
  keep_history: boolean;
  whisper_model: string;
};
type Entry = {
  word: string;
  heard_as: string[];
  hint_only: string[];
  replace_with: string | null;
  source: "manual" | "learned";
  starred: boolean;
  uses: number;
};
type HistoryItem = { at_ms: number; raw: string; cleaned: string; final_text: string | null; app: string | null };
type LearnEvent = { kind: string; word: string; heard: string };

export const isMac = navigator.userAgent.includes("Mac");
export const HOTKEYS = isMac
  ? ["Fn", "RightAlt", "RightCmd", "RightCtrl", "F13"]
  : ["Ctrl+Win", "RightAlt", "RightCtrl", "Ctrl+Shift"];
export const LANGUAGES: [Settings["language"], string][] = [
  ["auto", "Auto (English stays English, Hindi as Hinglish)"],
  ["english", "English"],
  ["hindi", "Hindi (Devanagari)"],
  ["hinglish", "Hinglish (Roman script)"],
];
const PROVIDERS: [Settings["provider"], string][] = [
  ["local", "Local (private, free)"],
  ["gemini_free", "Gemini free tier (Free, may be slow or skip cleanup when busy)"],
  ["gemini_paid", "Gemini paid"],
  ["haiku", "Claude Haiku"],
];
const MODELS: [string, string][] = [
  ["ggml-large-v3-turbo-q8_0.bin", "Best accuracy (about 0.9 GB)"],
  ["ggml-small-q8_0.bin", "Smaller and faster (about 0.25 GB)"],
];

export function esc(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

export function options<T extends string>(list: [T, string][], selected: T) {
  return list.map(([v, label]) => `<option value="${esc(v)}" ${v === selected ? "selected" : ""}>${esc(label)}</option>`).join("");
}

const app = document.getElementById("app")!;
const tabs = ["Settings", "Dictionary", "History"] as const;
type Tab = (typeof tabs)[number];
let current: Tab = "Settings";

function flash(el: HTMLElement | null, text: string, bad = false) {
  if (!el) return;
  el.textContent = text;
  el.className = bad ? "status bad" : "status";
}

function keyProvider(p: Settings["provider"]) {
  return p === "haiku" ? "anthropic" : p === "local" ? null : "gemini";
}

async function renderSettings(root: HTMLElement) {
  const s = await invoke<Settings>("get_settings");
  const kp = keyProvider(s.provider);
  const hasKey = kp ? await invoke<boolean>("has_api_key", { provider: kp }) : false;
  root.innerHTML = `
    <label>Language<select id="language">${options(LANGUAGES, s.language)}</select></label>
    <label>Cleanup engine<select id="provider">${options(PROVIDERS, s.provider)}</select></label>
    ${
      kp
        ? `<div class="keyrow"><label>${kp === "gemini" ? "Gemini" : "Anthropic"} API key
             <input id="apikey" type="password" autocomplete="off" placeholder="${hasKey ? "Saved in your system keychain" : "Paste your key"}"></label>
             <button id="savekey">Save key</button></div>
           <p class="hint">Only your transcript and dictionary words are sent to this provider.</p>`
        : `<p class="hint">Everything stays on this computer.</p>`
    }
    <label>Dictation key<select id="hotkey">${options(HOTKEYS.map((k) => [k, k] as [string, string]), s.hotkey)}</select></label>
    <p id="globe" class="status bad"></p>
    <label>Speech model<select id="model">${options(MODELS, s.whisper_model)}</select></label>
    <div id="dl"></div>
    <label class="check"><input id="learn" type="checkbox" ${s.learn_from_corrections ? "checked" : ""}> Learn from my corrections</label>
    <label class="check"><input id="keep" type="checkbox" ${s.keep_history ? "checked" : ""}> Keep history</label>
    <p id="status" class="status"></p>`;
  const status = root.querySelector<HTMLElement>("#status");
  const save = async (patch: Partial<Settings>, rerender = false) => {
    try {
      await invoke("set_settings", { settings: { ...s, ...patch } });
      Object.assign(s, patch);
      flash(status, "Saved");
      if (rerender) renderSettings(root);
    } catch (e) {
      flash(status, String(e), true);
    }
  };
  const val = (id: string) => root.querySelector<HTMLSelectElement>(id)!.value;
  root.querySelector("#language")!.addEventListener("change", () => save({ language: val("#language") as Settings["language"] }));
  root.querySelector("#provider")!.addEventListener("change", () => save({ provider: val("#provider") as Settings["provider"] }, true));
  const globe = async () => {
    const el = root.querySelector<HTMLElement>("#globe")!;
    el.innerHTML = (await invoke<boolean>("globe_key_warning")) ? GLOBE_WARNING : "";
    el.querySelector("button")?.addEventListener("click", () => invoke("open_settings_pane", { pane: "keyboard" }));
  };
  globe();
  root.querySelector("#hotkey")!.addEventListener("change", async () => {
    await save({ hotkey: val("#hotkey") });
    globe();
  });
  // A model that is not on disk yet is downloaded before it is selected.
  root.querySelector("#model")!.addEventListener("change", async () => {
    const name = val("#model");
    const select = root.querySelector<HTMLSelectElement>("#model")!;
    if (!(await invoke<boolean>("whisper_ready", { name }))) {
      const box = root.querySelector<HTMLElement>("#dl")!;
      box.innerHTML = `<div class="progress"><div></div></div>`;
      const bar = box.querySelector<HTMLElement>(".progress > div")!;
      select.disabled = true;
      const stop = await listen<{ kind: string; done: number; total: number }>("setup-progress", ({ payload }) => {
        if (payload.kind === "whisper" && payload.total) bar.style.width = `${(100 * payload.done) / payload.total}%`;
      });
      try {
        await invoke("download_whisper", { name });
      } catch (e) {
        flash(status, String(e), true);
        select.value = s.whisper_model;
        return;
      } finally {
        stop();
        box.innerHTML = "";
        select.disabled = false;
      }
    }
    save({ whisper_model: name });
  });
  root.querySelector("#learn")!.addEventListener("change", (e) => save({ learn_from_corrections: (e.target as HTMLInputElement).checked }));
  root.querySelector("#keep")!.addEventListener("change", (e) => save({ keep_history: (e.target as HTMLInputElement).checked }));
  root.querySelector("#savekey")?.addEventListener("click", async () => {
    const input = root.querySelector<HTMLInputElement>("#apikey")!;
    try {
      await invoke("set_api_key", { provider: kp, key: input.value });
      input.value = "";
      flash(status, "Key saved in your system keychain");
    } catch (e) {
      flash(status, String(e), true);
    }
  });
}

async function renderDictionary(root: HTMLElement) {
  const entries = await invoke<Entry[]>("list_dictionary");
  entries.sort((a, b) => Number(b.starred) - Number(a.starred) || a.word.localeCompare(b.word));
  const row = (e: Entry) => {
    const heard = e.heard_as.length ? `heard as ${e.heard_as.map(esc).join(", ")}` : "";
    const hints = e.hint_only.length ? ` <span class="muted">(hint only: ${e.hint_only.map(esc).join(", ")})</span>` : "";
    const from = e.source === "learned" && e.heard_as.length + e.hint_only.length ? `Learned from: ${esc([...e.heard_as, ...e.hint_only][0])}` : "";
    const swap = e.replace_with ? `inserts "${esc(e.replace_with)}"` : "";
    return `<li data-word="${esc(e.word)}">
      <button class="star ${e.starred ? "on" : ""}" title="Star: always sent first">${e.starred ? "★" : "☆"}</button>
      <div class="grow"><strong>${esc(e.word)}</strong> <span class="muted">${[swap, heard].filter(Boolean).join(" · ")}</span>${hints}
      ${from ? `<div class="from">${from}</div>` : ""}</div>
      <button class="del">Delete</button></li>`;
  };
  root.innerHTML = `
    <form id="add" class="addform">
      <input name="word" placeholder="Word, spelled right" required>
      <input name="heard" placeholder="Heard as (comma separated)">
      <input name="replace" placeholder="Or insert this text instead">
      <button>Add</button>
    </form>
    <p id="status" class="status"></p>
    <ul class="list">${entries.map(row).join("") || `<li class="muted">No words yet. Correct a word after dictating, or add one here.</li>`}</ul>
    <div class="row">
      <label class="filebtn">Import JSON<input id="import" type="file" accept="application/json,.json" hidden></label>
      <button id="export">Copy dictionary as JSON</button>
    </div>`;
  const status = root.querySelector<HTMLElement>("#status");
  root.querySelector<HTMLFormElement>("#add")!.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const f = new FormData(ev.target as HTMLFormElement);
    try {
      await invoke("upsert_entry", {
        word: String(f.get("word")),
        heardAs: String(f.get("heard") ?? "").split(","),
        replaceWith: String(f.get("replace") ?? "") || null,
      });
      renderDictionary(root);
    } catch (e) {
      flash(status, String(e), true);
    }
  });
  root.querySelectorAll<HTMLLIElement>("li[data-word]").forEach((li) => {
    const word = li.dataset.word!;
    const entry = entries.find((e) => e.word === word)!;
    li.querySelector(".star")!.addEventListener("click", async () => {
      await invoke("star_entry", { word, starred: !entry.starred });
      renderDictionary(root);
    });
    li.querySelector(".del")!.addEventListener("click", async () => {
      await invoke("delete_entry", { word });
      renderDictionary(root);
    });
  });
  root.querySelector<HTMLInputElement>("#import")!.addEventListener("change", async (ev) => {
    const file = (ev.target as HTMLInputElement).files?.[0];
    if (!file) return;
    try {
      const n = await invoke<number>("import_dictionary", { json: await file.text() });
      await renderDictionary(root);
      flash(root.querySelector("#status"), `Imported ${n} new word${n === 1 ? "" : "s"}`);
    } catch (e) {
      flash(status, String(e), true);
    }
  });
  root.querySelector("#export")!.addEventListener("click", async () => {
    await navigator.clipboard.writeText(await invoke<string>("export_dictionary"));
    flash(status, "Dictionary JSON copied to the clipboard");
  });
}

async function renderHistory(root: HTMLElement) {
  const items = await invoke<HistoryItem[]>("list_history");
  const row = (h: HistoryItem) => {
    const when = new Date(h.at_ms).toLocaleString();
    const text = h.final_text ?? h.cleaned;
    return `<li data-at="${h.at_ms}"><div class="grow">
      <div class="muted">${esc(when)}${h.app ? ` · ${esc(h.app)}` : ""}</div>
      <div>${esc(text)}</div>
      ${h.raw !== text ? `<div class="muted small">Heard: ${esc(h.raw)}</div>` : ""}
      <form class="fix" hidden><input name="wrong" placeholder="Wrong word"><input name="right" placeholder="Right word"><button>Learn</button></form>
      </div>
      <div class="actions"><button class="copy">Copy</button><button class="again">Re-insert</button><button class="fixbtn">Fix a word</button></div></li>`;
  };
  root.innerHTML = `
    <p id="status" class="status"></p>
    <ul class="list">${items.map(row).join("") || `<li class="muted">Nothing yet. Your last 100 dictations show up here (text only, never audio).</li>`}</ul>
    ${items.length ? `<button id="clear">Clear history</button>` : ""}`;
  const status = root.querySelector<HTMLElement>("#status");
  root.querySelectorAll<HTMLLIElement>("li[data-at]").forEach((li) => {
    const item = items.find((h) => String(h.at_ms) === li.dataset.at)!;
    const text = item.final_text ?? item.cleaned;
    li.querySelector(".copy")!.addEventListener("click", async () => {
      await navigator.clipboard.writeText(text);
      flash(status, "Copied");
    });
    li.querySelector(".again")!.addEventListener("click", () => invoke("reinsert", { text }));
    const form = li.querySelector<HTMLFormElement>(".fix")!;
    li.querySelector(".fixbtn")!.addEventListener("click", () => (form.hidden = !form.hidden));
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const f = new FormData(form);
      try {
        const events = await invoke<LearnEvent[]>("fix_word_cmd", { atMs: item.at_ms, wrong: f.get("wrong"), right: f.get("right") });
        form.hidden = true;
        flash(status, events.length ? `Learned: ${events.map((e) => e.word).join(", ")}` : "Nothing to learn from that change", !events.length);
      } catch (e) {
        flash(status, String(e), true);
      }
    });
  });
  root.querySelector("#clear")?.addEventListener("click", async () => {
    await invoke("clear_history");
    renderHistory(root);
  });
}

const renderers: Record<Tab, (root: HTMLElement) => Promise<void>> = {
  Settings: renderSettings,
  Dictionary: renderDictionary,
  History: renderHistory,
};

export function renderApp() {
  app.innerHTML = `
    <header><h1>Shabdly</h1><span class="tagline">Words that learn you.</span></header>
    <nav>${tabs.map((t) => `<button class="tab ${t === current ? "active" : ""}" data-tab="${t}">${t}</button>`).join("")}</nav>
    <main id="panel"></main>`;
  app.querySelectorAll<HTMLButtonElement>(".tab").forEach((b) =>
    b.addEventListener("click", () => {
      current = b.dataset.tab as Tab;
      renderApp();
    }),
  );
  renderers[current](document.getElementById("panel")!);
}

// Refresh lists when the window comes back from the tray.
window.addEventListener("focus", () => {
  if (current !== "Settings") renderers[current](document.getElementById("panel")!);
});

invoke<"first_run" | "speech_model" | null>("needs_setup").then((setup) =>
  setup ? startOnboarding(app, renderApp, setup === "speech_model" ? "speech_model" : undefined) : renderApp(),
);
