// Shared motion: a critically damped spring (from --spring in style.css) for
// page changes, list reorders and the overlay pill. Reduced Motion gets short
// opacity fades instead.

const css = getComputedStyle(document.documentElement);
export const SPRING = css.getPropertyValue("--spring").trim() || "cubic-bezier(0.2, 0.8, 0.2, 1)";
export const SPRING_MS = 550;
const reduce = matchMedia("(prefers-reduced-motion: reduce)");

/** Content arriving: fade up from a few px below. */
export function enter(el: Element | null, dy = 6) {
  if (!el) return;
  if (reduce.matches) return el.animate({ opacity: [0, 1] }, { duration: 150 });
  return el.animate({ opacity: [0, 1], transform: [`translateY(${dy}px)`, "none"] }, { duration: SPRING_MS, easing: SPRING });
}

/** Content leaving; resolves when done so the caller can remove or hide it. */
export async function leave(el: Element | null, dy = -4) {
  if (!el) return;
  const frames = reduce.matches ? { opacity: [1, 0] } : { opacity: [1, 0], transform: ["none", `translateY(${dy}px)`] };
  await el.animate(frames, { duration: reduce.matches ? 120 : 160, easing: "ease-in", fill: "forwards" }).finished;
}

const key = (el: HTMLElement) => el.dataset.word ?? el.dataset.at;

/** Re-render a list without jumps: rows that stay slide to their new place, new rows fade in. */
export async function flip(root: HTMLElement, render: () => Promise<void>) {
  const rows = () => [...root.querySelectorAll<HTMLElement>("li[data-word], li[data-at]")];
  const before = new Map(rows().map((el) => [key(el), el.getBoundingClientRect().top]));
  await render();
  for (const el of rows()) {
    const top = before.get(key(el));
    if (top === undefined) enter(el, 4);
    else if (!reduce.matches && top !== el.getBoundingClientRect().top)
      el.animate({ transform: [`translateY(${top - el.getBoundingClientRect().top}px)`, "none"] }, { duration: SPRING_MS, easing: SPRING });
  }
}

/** Collapse a row away before it is deleted. */
export async function collapse(el: HTMLElement) {
  const h = el.getBoundingClientRect().height;
  const frames = reduce.matches
    ? { opacity: [1, 0] }
    : { opacity: [1, 0], height: [`${h}px`, "0px"], paddingTop: [getComputedStyle(el).paddingTop, "0px"], paddingBottom: [getComputedStyle(el).paddingBottom, "0px"] };
  el.style.overflow = "hidden";
  await el.animate(frames, { duration: 220, easing: SPRING, fill: "forwards" }).finished;
}
