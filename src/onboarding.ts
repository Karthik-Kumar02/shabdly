// First-run setup (spec "First-run setup" steps 1 to 5). Shown until
// settings.json exists; the last step saves settings and opens the app.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { HOTKEYS, LANGUAGES, type Settings, esc, isMac, options } from "./main";
import { SPRING, SPRING_MS, enter } from "./motion";

type Step = (card: HTMLElement, next: () => void) => Promise<void> | void;

export const GLOBE_WARNING = `macOS also acts on the Fn (Globe) key, which can open the emoji picker while you dictate. In Keyboard settings, set "Press Globe key to" to "Do Nothing". <button>Open Keyboard settings</button>`;
type Progress = { kind: "all" | "model" | "whisper"; done: number; total: number };
export type ModelsStatus = {
  speech_ready: boolean;
  engine_ready: boolean;
  engine_offered: boolean;
  best_offered: boolean;
  bytes_needed: number;
  free_bytes: number;
};

export const gb = (n: number) => (n / 1e9).toFixed(1);

// Bar plus "{done} of {total} GB, about {m} min left", above the step's #st line. The time left
// uses the average rate over the last 10 s and is left out for the first 3 s.
export function progressBar(card: HTMLElement, kind: Progress["kind"]) {
  card.querySelectorAll(".progress, #pct").forEach((el) => el.remove());
  const html = `<div class="progress"><div></div></div><p class="status" id="pct"></p>`;
  const st = card.querySelector("#st");
  st ? st.insertAdjacentHTML("beforebegin", html) : card.insertAdjacentHTML("beforeend", html);
  const bar = card.querySelector<HTMLElement>(".progress > div")!;
  const label = card.querySelector<HTMLElement>("#pct")!;
  const start = performance.now();
  const seen: [number, number][] = [];
  return listen<Progress>("setup-progress", ({ payload: { kind: k, done, total } }) => {
    const now = performance.now();
    if (k !== kind || !total || (seen.length && now - seen[seen.length - 1][0] < 250)) return;
    seen.push([now, done]);
    while (now - seen[0][0] > 10_000) seen.shift();
    const rate = now > seen[0][0] ? (done - seen[0][1]) / (now - seen[0][0]) : 0;
    const left = now - start >= 3000 && rate > 0 ? `, about ${Math.max(1, Math.round((total - done) / rate / 60_000))} min left` : "";
    bar.style.width = `${(100 * done) / total}%`;
    label.textContent = `${gb(done)} of ${gb(total)} GB${left}`;
  });
}

const welcome: Step = (card, next) => {
  card.innerHTML = `
    <h2>Welcome to Shabdly</h2>
    <p>Hold a key, speak in English, Hindi or Hinglish, and clean text appears in whatever app you are typing in. Shabdly learns your words from your corrections.</p>
    <p>It runs on this computer by default. No account, and nothing leaves your machine.</p>
    <button class="primary" id="go">Get started</button>`;
  card.querySelector("#go")!.addEventListener("click", next);
};

const microphone: Step = (card, next) => {
  card.innerHTML = `
    <h2>Microphone</h2>
    <p>Shabdly listens only while you hold your dictation key.</p>
    <button class="primary" id="mic">Allow microphone</button>
    <p class="status" id="st"></p>`;
  card.querySelector("#mic")!.addEventListener("click", async () => {
    if (await invoke<boolean>("request_microphone")) return next();
    const st = card.querySelector<HTMLElement>("#st")!;
    st.className = "status bad";
    st.innerHTML = isMac
      ? `Shabdly can't hear the microphone yet. Click Allow if macOS asks, then click Allow microphone again. Still nothing? <button id="pane">Open Microphone settings</button>`
      : `Shabdly can't hear a microphone. Check that one is connected and allowed. <button id="pane">Open Microphone settings</button>`;
    card.querySelector("#pane")?.addEventListener("click", () => invoke("open_settings_pane", { pane: "microphone" }));
  });
};

const accessibility: Step = async (card, next) => {
  card.innerHTML = `
    <h2>Typing into other apps</h2>
    <p>Lets Shabdly type into other apps and learn from your corrections.</p>
    <p class="muted">macOS asks for two switches: <strong>Accessibility</strong> (to type) and <strong>Input Monitoring</strong> (to see your dictation key). Turn Shabdly on in both.</p>
    <div class="row">
      <button class="primary" id="ax">Allow Accessibility</button>
      <button id="im">Open Input Monitoring</button>
    </div>
    <p class="status" id="st"></p>
    <button id="cont">Continue</button>`;
  const st = card.querySelector<HTMLElement>("#st")!;
  const check = async () => {
    const ok = await invoke<boolean>("accessibility_status");
    st.textContent = ok ? "Accessibility is on." : "Accessibility is not on yet.";
    return ok;
  };
  card.querySelector("#ax")!.addEventListener("click", () => invoke("request_accessibility"));
  card.querySelector("#im")!.addEventListener("click", () => invoke("open_settings_pane", { pane: "input_monitoring" }));
  card.querySelector("#cont")!.addEventListener("click", next);
  const timer = window.setInterval(async () => {
    if (!card.isConnected) return window.clearInterval(timer);
    await check();
  }, 1000);
  await check();
};

// Behind the Download step's Advanced link: Ollama or a cloud engine, then the speech model.
const advanced: Step = async (card, done) => {
  const next = () => whisper(card, done);
  const useOllama = async () => {
    const s = await invoke<Settings>("get_settings");
    await invoke("set_settings", { settings: { ...s, provider: "local", local_engine: "ollama" } });
    next();
  };
  card.innerHTML = `<h2>Cleanup engine</h2><p>Checking for the local engine...</p>`;
  const status = await invoke<"missing" | "no_model" | "ready">("ollama_status");
  if (status === "ready") {
    card.innerHTML = `<h2>Cleanup engine</h2><p>The local engine is ready. Your words stay on this computer.</p><button class="primary" id="go">Continue</button>`;
    card.querySelector("#go")!.addEventListener("click", useOllama);
    return;
  }
  if (status === "no_model") {
    card.innerHTML = `
      <h2>Cleanup engine</h2>
      <p>Ollama is installed. Shabdly needs its cleanup model, gemma4:e4b (about 10 GB, one time).</p>
      <button class="primary" id="pull">Download the model</button>
      <p class="status" id="st"></p>`;
    card.querySelector("#pull")!.addEventListener("click", async (ev) => {
      (ev.target as HTMLButtonElement).disabled = true;
      const stop = await progressBar(card, "model");
      try {
        await invoke("pull_model");
        stop();
        await useOllama();
      } catch (e) {
        stop();
        card.querySelector<HTMLElement>("#st")!.textContent = String(e);
        (ev.target as HTMLButtonElement).disabled = false;
      }
    });
    return;
  }
  card.innerHTML = `
    <h2>Cleanup engine</h2>
    <p>The local engine (Ollama) is not installed. Install it for private, free cleanup, then check again.</p>
    <div class="row">
      <button class="primary" id="install">Get Ollama</button>
      <button id="again">Check again</button>
    </div>
    <p class="muted">Or use a cloud engine instead. Only your transcript and dictionary words are sent.</p>
    <div class="row">
      <button id="gemini">Use Gemini free tier</button>
      <button id="haiku">Use Claude Haiku</button>
    </div>
    <div id="key" hidden class="keyrow">
      <label>API key<input id="k" type="password" autocomplete="off"></label>
      <button id="save">Save and continue</button>
    </div>
    <p class="status" id="st"></p>`;
  card.querySelector("#install")!.addEventListener("click", () => openUrl("https://ollama.com/download"));
  card.querySelector("#again")!.addEventListener("click", () => advanced(card, done));
  let provider: Settings["provider"] = "gemini_free";
  const pick = (p: Settings["provider"]) => () => {
    provider = p;
    card.querySelector<HTMLElement>("#key")!.hidden = false;
  };
  card.querySelector("#gemini")!.addEventListener("click", pick("gemini_free"));
  card.querySelector("#haiku")!.addEventListener("click", pick("haiku"));
  card.querySelector("#save")!.addEventListener("click", async () => {
    const st = card.querySelector<HTMLElement>("#st")!;
    try {
      await invoke("set_api_key", { provider: provider === "haiku" ? "anthropic" : "gemini", key: card.querySelector<HTMLInputElement>("#k")!.value });
      const s = await invoke<Settings>("get_settings");
      await invoke("set_settings", { settings: { ...s, provider } });
      next();
    } catch (e) {
      st.className = "status bad";
      st.textContent = String(e);
    }
  });
};

const whisper: Step = async (card, next) => {
  const s = await invoke<Settings>("get_settings");
  if (await invoke<boolean>("whisper_ready", { name: s.whisper_model })) return next();
  card.innerHTML = `
    <h2>Speech model</h2>
    <p>Shabdly turns speech into text on this computer. It needs a speech model (about 0.9 GB, one time).</p>
    <button class="primary" id="dl">Download</button>
    <p class="status" id="st"></p>`;
  card.querySelector("#dl")!.addEventListener("click", async (ev) => {
    (ev.target as HTMLButtonElement).disabled = true;
    const stop = await progressBar(card, "whisper");
    try {
      await invoke("download_whisper", { name: s.whisper_model });
      stop();
      next();
    } catch (e) {
      stop();
      const st = card.querySelector<HTMLElement>("#st")!;
      st.className = "status bad";
      st.textContent = String(e);
      (ev.target as HTMLButtonElement).disabled = false;
    }
  });
};

// Speech and cleanup models in one go. Ollama and cloud users only need the speech model, and
// Macs without a bundled engine (macOS 11.0 to 13.2) go straight to the Advanced choices.
const download: Step = async (card, next) => {
  const s = await invoke<Settings>("get_settings");
  if (s.provider !== "local" || s.local_engine !== "bundled") return whisper(card, next);
  card.innerHTML = `<h2>Download</h2><p>Checking for models...</p>`;
  const m = await invoke<ModelsStatus>("models_status");
  if (!m.engine_offered) return advanced(card, next);
  if (!m.bytes_needed) {
    // The engine started before this check copied a cached model in, so it may be on a backup.
    if ((await invoke<{ state: string }>("engine_status")).state !== "builtin") invoke("retry_engine");
    card.innerHTML = `
      <h2>Download</h2>
      <p>Models found, ready.</p>
      <button class="primary" id="go">Continue</button>
      <button class="link" id="adv">Advanced: use Ollama or a cloud engine</button>`;
    card.querySelector("#go")!.addEventListener("click", next);
    card.querySelector("#adv")!.addEventListener("click", () => advanced(card, next));
    return;
  }
  card.innerHTML = `
    <h2>Download</h2>
    <p>Shabdly needs two models to work offline (${gb(m.bytes_needed)} GB, one time).</p>
    <button class="primary" id="dl">Download</button>
    <p class="status" id="st"></p>
    <button class="link" id="adv">Advanced: use Ollama or a cloud engine</button>`;
  const dl = card.querySelector<HTMLButtonElement>("#dl")!;
  const adv = card.querySelector<HTMLButtonElement>("#adv")!;
  const st = card.querySelector<HTMLElement>("#st")!;
  const run = async () => {
    dl.hidden = adv.hidden = true;
    st.className = "status";
    st.textContent = "";
    const stop = await progressBar(card, "all");
    try {
      await invoke("download_models");
      stop();
      next();
    } catch (e) {
      stop();
      adv.hidden = false;
      st.className = "status bad";
      st.innerHTML = `${esc(String(e))} <button id="again">Try again</button>`;
      st.querySelector("#again")!.addEventListener("click", run);
    }
  };
  dl.addEventListener("click", run);
  adv.addEventListener("click", () => advanced(card, next));
};

const practice: Step = async (card, next) => {
  const s = await invoke<Settings>("get_settings");
  const save = (patch: Partial<Settings>) => {
    Object.assign(s, patch);
    return invoke("set_settings", { settings: s });
  };
  card.innerHTML = `
    <h2>Try it</h2>
    <label>Dictation key<select id="hotkey">${options(HOTKEYS.map((k) => [k, k] as [string, string]), s.hotkey)}</select></label>
    <label>Language<select id="language">${options(LANGUAGES, s.language)}</select></label>
    <p><strong id="say">Hold ${esc(s.hotkey)} and say hello</strong></p>
    <textarea class="practice" placeholder="Your words appear here"></textarea>
    <div id="health" class="status"></div>
    <div id="globe" class="status bad"></div>
    <button class="primary" id="done">Finish</button>`;
  const box = card.querySelector<HTMLTextAreaElement>("textarea")!;
  const health = card.querySelector<HTMLElement>("#health")!;
  const globe = async () => {
    const el = card.querySelector<HTMLElement>("#globe")!;
    el.innerHTML = (await invoke<boolean>("globe_key_warning")) ? GLOBE_WARNING : "";
    el.querySelector("button")?.addEventListener("click", () => invoke("open_settings_pane", { pane: "keyboard" }));
  };
  const watch = async () => {
    globe();
    health.className = "status";
    health.textContent = "";
    box.focus();
    if (await invoke<boolean>("hotkey_health", { ms: 10000 })) return;
    if (!card.isConnected) return;
    health.className = "status bad";
    health.innerHTML = `Shabdly can't see your hotkey.
      ${isMac ? `<button id="pane">Open Input Monitoring</button>` : ""} <button id="recheck">Check again</button> Or pick another key above.`;
    card.querySelector("#pane")?.addEventListener("click", () => invoke("open_settings_pane", { pane: "input_monitoring" }));
    card.querySelector("#recheck")!.addEventListener("click", watch);
  };
  card.querySelector("#hotkey")!.addEventListener("change", async (ev) => {
    const hotkey = (ev.target as HTMLSelectElement).value;
    await save({ hotkey });
    card.querySelector("#say")!.textContent = `Hold ${hotkey} and say hello`;
    watch();
  });
  card.querySelector("#language")!.addEventListener("change", (ev) => save({ language: (ev.target as HTMLSelectElement).value as Settings["language"] }));
  card.querySelector("#done")!.addEventListener("click", async () => {
    await save({});
    next();
  });
  watch();
};

// "speech_model": settings exist but the chosen speech model is missing.
export function startOnboarding(root: HTMLElement, done: () => void, only?: "speech_model") {
  const steps: Step[] = only ? [download] : [welcome, microphone, ...(isMac ? [accessibility] : []), download, practice];
  let i = 0;
  const show = () => {
    if (i >= steps.length) return done();
    const dots = steps.map((_, k) => `<i class="${k < i ? "on" : k === i ? "on cur" : ""}"></i>`).join("");
    root.innerHTML = `<div class="onboard" data-tauri-drag-region><div class="onboard-inner">
      <header><div class="brand">Shabdly</div><span class="tagline">Words that learn you.</span></header>
      <div class="card" id="card"></div>
      <p class="steps"><span class="stepdots" aria-hidden="true">${dots}</span>Step ${i + 1} of ${steps.length}</p></div></div>`;
    const card = document.getElementById("card")!;
    steps[i](card, () => {
      i += 1;
      show();
    });
    enter(card, 10);
    root.querySelector(".stepdots .cur")?.animate({ width: ["6px", "16px"] }, { duration: SPRING_MS, easing: SPRING });
  };
  show();
}
