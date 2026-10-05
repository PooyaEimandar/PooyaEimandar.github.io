type LoadStage = "matrix" | "face" | "timeline";

interface SlideLink {
  label: string;
  url: string;
}

interface SlideEntry {
  year: string;
  title: string;
  text: string;
  links: SlideLink[];
}

interface WasmBindings {
  default: (input?: {
    module_or_path: RequestInfo | URL | Response | BufferSource | WebAssembly.Module;
  }) => Promise<unknown>;
  set_reduced_motion?: (reduced: boolean) => void;
  set_header_height?: (cssPixels: number) => void;
  activate_timeline_link?: (x: number, y: number) => boolean;
  reveal_or_advance_timeline?: () => boolean;
}

interface RendererErrorDetail {
  message?: string;
}

interface TimelineChangeDetail {
  eyebrow?: string;
  heading?: string;
  description?: string;
  /** The slide's entries as JSON: `SlideEntry[]`. */
  entries?: string;
}

interface SceneProgressDetail {
  stage?: LoadStage;
  progress?: number;
  message?: string;
}

const WASM_MODULE_PATH = "./pkg/pooya_portfolio.js";
const WASM_BINARY_PATH = "./pkg/pooya_portfolio_bg.wasm";
const BUILD_ID = new URL(import.meta.url).searchParams.get("build") ?? "development";
const RENDERER_TIMEOUT_MS = 30_000;
const PRODUCTION_HOSTNAMES = new Set(["pooya.ai", "www.pooya.ai"]);
const WEBGPU_UNAVAILABLE_MESSAGE =
  "Welcome to website of Pooya Eimandar, it seems your browser doesn't support WebGPU, please update your browser or use another one.";

function requiredElement<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (!(element instanceof HTMLElement)) {
    throw new Error(`Required page element #${id} was not found.`);
  }
  return element as T;
}

function versionedRuntimeUrl(path: string): URL {
  const url = new URL(path, import.meta.url);
  url.searchParams.set("build", BUILD_ID);
  return url;
}

const page = document.body;
const loaderPanel = requiredElement<HTMLElement>("loader-panel");
const loaderTitle = requiredElement<HTMLElement>("loader-title");
const loaderMessage = requiredElement<HTMLElement>("loader-message");
const loaderPercent = requiredElement<HTMLElement>("loader-percent");
const progressTrack = requiredElement<HTMLElement>("progress-track");
const progressBar = requiredElement<HTMLElement>("progress-bar");
const unsupportedPanel = requiredElement<HTMLElement>("unsupported-panel");
const unsupportedTitle = requiredElement<HTMLElement>("unsupported-title");
const unsupportedMessage = requiredElement<HTMLElement>("unsupported-message");
const errorPanel = requiredElement<HTMLElement>("error-panel");
const errorMessage = requiredElement<HTMLElement>("error-message");
const retryButton = requiredElement<HTMLButtonElement>("retry-button");
const timelineEyebrow = requiredElement<HTMLElement>("timeline-year");
const timelineTitle = requiredElement<HTMLElement>("timeline-title");
const timelineDescription = requiredElement<HTMLElement>("timeline-description");
const timelineEntries = requiredElement<HTMLElement>("timeline-entries");
const timelineLinks = requiredElement<HTMLElement>("timeline-links");
const timelineCopy = requiredElement<HTMLElement>("timeline-copy");
const copyrightYear = requiredElement<HTMLTimeElement>("copyright-year");
const siteHeader = document.querySelector<HTMLElement>(".site-header");

const motionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
const loadStageElements = Array.from(document.querySelectorAll<HTMLElement>("[data-stage]"));

let wasmBindings: WasmBindings | null = null;
let rendererReady = false;
let rendererTimeout: number | undefined;

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value));
}

function updateLoadStage(stage: LoadStage, progress: number, message: string): void {
  const stageIndex = ["matrix", "face", "timeline"].indexOf(stage);
  const safeProgress = Math.round(clamp(progress, 0, 100));

  loadStageElements.forEach((element, index) => {
    element.classList.toggle("is-complete", index < stageIndex);
    if (index === stageIndex) {
      element.setAttribute("aria-current", "step");
    } else {
      element.removeAttribute("aria-current");
    }
  });

  loaderTitle.textContent = stage === "matrix"
    ? "Entering the matrix"
    : stage === "face"
      ? "Resolving the digital likeness"
      : "Opening Pooya's timeline";
  loaderMessage.textContent = message;
  loaderPercent.textContent = `${safeProgress.toString().padStart(2, "0")}%`;
  progressTrack.setAttribute("aria-valuenow", safeProgress.toString());
  progressBar.style.width = `${safeProgress}%`;
}

function clearRendererTimeout(): void {
  if (rendererTimeout !== undefined) {
    window.clearTimeout(rendererTimeout);
    rendererTimeout = undefined;
  }
}

function showUnsupportedBrowser(
  title = "The signal could not start.",
  message = WEBGPU_UNAVAILABLE_MESSAGE,
): void {
  clearRendererTimeout();
  page.dataset.renderState = "unsupported";
  page.classList.remove("webgpu-active");
  timelineCopy.removeAttribute("inert");
  loaderPanel.hidden = true;
  errorPanel.hidden = true;
  unsupportedTitle.textContent = title;
  unsupportedMessage.textContent = message;
  unsupportedPanel.hidden = false;
}

function redirectProductionToHttps(): boolean {
  if (
    window.isSecureContext
    || window.location.protocol !== "http:"
    || !PRODUCTION_HOSTNAMES.has(window.location.hostname)
  ) {
    return false;
  }

  const secureUrl = new URL(window.location.href);
  secureUrl.protocol = "https:";
  secureUrl.port = "";
  window.location.replace(secureUrl);
  return true;
}

function showRendererError(error: unknown): void {
  clearRendererTimeout();
  rendererReady = false;
  page.dataset.renderState = "error";
  page.classList.remove("webgpu-active");
  timelineCopy.removeAttribute("inert");
  timelineLinks.hidden = true;
  loaderPanel.hidden = true;
  unsupportedPanel.hidden = true;
  errorPanel.hidden = false;

  const detail = error instanceof Error ? error.message : "Unknown renderer error.";
  errorMessage.textContent = `The accessible timeline is still available below. Technical detail: ${detail}`;
  console.error("WebGPU renderer failed to start:", error);
}

function rendererError(value: unknown, fallback: string): Error {
  if (value instanceof Error) {
    return value;
  }
  if (typeof value === "string" && value.trim()) {
    return new Error(value);
  }
  return new Error(fallback);
}

function handleRendererError(event: Event): void {
  const detail = (event as CustomEvent<RendererErrorDetail>).detail ?? {};
  showRendererError(rendererError(detail.message, "WebGPU reported an uncaptured renderer error."));
}

function handleStartupWindowError(event: ErrorEvent): void {
  if (!rendererReady && page.dataset.renderState === "loading") {
    showRendererError(rendererError(event.error ?? event.message, "The browser stopped the WebGPU renderer."));
  }
}

function handleStartupRejection(event: PromiseRejectionEvent): void {
  if (!rendererReady && page.dataset.renderState === "loading") {
    showRendererError(rendererError(event.reason, "The browser rejected WebGPU initialization."));
  }
}

function slideEntries(json: string | undefined): SlideEntry[] {
  try {
    const entries: unknown = JSON.parse(json ?? "[]");
    return Array.isArray(entries) ? (entries as SlideEntry[]) : [];
  } catch (error) {
    console.error("The renderer described a slide in a form the page could not read:", error);
    return [];
  }
}

// The canvas shows the current slide only as pixels. This mirrors it as text
// for screen readers, and as real links for anyone not using a pointer.
function updateTimelineTranscript(detail: TimelineChangeDetail): void {
  const entries = slideEntries(detail.entries);
  timelineEyebrow.textContent = detail.eyebrow || "Pooya's timeline";
  timelineTitle.textContent = detail.heading || "Pooya's timeline";
  timelineDescription.textContent = detail.description ?? "";
  timelineEntries.replaceChildren(...entries.flatMap((entry) => {
    const heading = document.createElement("h3");
    heading.textContent = `${entry.year} — ${entry.title}`;
    if (!entry.text) {
      return [heading];
    }
    const text = document.createElement("p");
    text.textContent = entry.text;
    return [heading, text];
  }));

  const links = entries
    .flatMap((entry) => entry.links)
    .filter((link) => link.url.startsWith("https://"));
  timelineLinks.replaceChildren(...links.map((link) => {
    const anchor = document.createElement("a");
    anchor.href = link.url;
    anchor.target = "_blank";
    anchor.rel = "noopener noreferrer";
    anchor.textContent = link.label;
    return anchor;
  }));
  timelineLinks.hidden = links.length === 0;
}

function applySystemMotionPreference(): void {
  wasmBindings?.set_reduced_motion?.(motionQuery.matches);
}

// The fixed header covers the top of the canvas, so the renderer lays the
// terminal out below it.
function reportHeaderHeight(): void {
  if (siteHeader) {
    wasmBindings?.set_header_height?.(siteHeader.getBoundingClientRect().height);
  }
}

function installCanvasLinkActivation(): void {
  const canvas = document.querySelector<HTMLCanvasElement>("#webgpu-canvas");
  if (!canvas || canvas.dataset.linkActivation === "ready") {
    return;
  }
  canvas.dataset.linkActivation = "ready";

  let pointerStart: { id: number; x: number; y: number } | null = null;
  canvas.addEventListener("pointerdown", (event) => {
    if (!event.isPrimary || event.button !== 0) {
      return;
    }
    pointerStart = { id: event.pointerId, x: event.clientX, y: event.clientY };
  });
  canvas.addEventListener("pointercancel", (event) => {
    if (pointerStart?.id === event.pointerId) {
      pointerStart = null;
    }
  });
  canvas.addEventListener("pointerup", (event) => {
    const start = pointerStart;
    pointerStart = null;
    if (!start || start.id !== event.pointerId || !event.isPrimary || event.button !== 0) {
      return;
    }

    const travel = Math.hypot(event.clientX - start.x, event.clientY - start.y);
    if (travel > 12) {
      return;
    }
    const rect = canvas.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      return;
    }
    const x = (event.clientX - rect.left) * canvas.width / rect.width;
    const y = (event.clientY - rect.top) * canvas.height / rect.height;
    const activatedLink = wasmBindings?.activate_timeline_link?.(x, y) ?? false;
    if (activatedLink) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    wasmBindings?.reveal_or_advance_timeline?.();
  });
}

function handlePrimaryKeyboardAction(event: KeyboardEvent): void {
  if (
    !rendererReady
    || event.key !== "Enter"
    || event.repeat
    || event.defaultPrevented
    || event.altKey
    || event.ctrlKey
    || event.metaKey
  ) {
    return;
  }

  const target = event.target;
  if (
    target instanceof HTMLElement
    && (target.isContentEditable || target.closest("a, button, input, select, textarea"))
  ) {
    return;
  }

  event.preventDefault();
  wasmBindings?.reveal_or_advance_timeline?.();
}

function handleRendererReady(): void {
  rendererReady = true;
  clearRendererTimeout();
  page.dataset.renderState = "ready";
  page.classList.add("webgpu-active");
  // Keep the complete, server-delivered timeline in the document for search
  // engines while removing its clipped links from the keyboard focus order.
  // The live transcript above remains the accessible representation of the
  // currently rendered WebGPU page.
  timelineCopy.setAttribute("inert", "");
  updateLoadStage("timeline", 100, "Pooya's timeline is ready.");
  loaderPanel.hidden = true;
  unsupportedPanel.hidden = true;
  errorPanel.hidden = true;
  applySystemMotionPreference();
  installCanvasLinkActivation();
}

function handleTimelineChange(event: Event): void {
  updateTimelineTranscript((event as CustomEvent<TimelineChangeDetail>).detail ?? {});
}

function handleSceneProgress(event: Event): void {
  const detail = (event as CustomEvent<SceneProgressDetail>).detail ?? {};
  if (detail.stage && typeof detail.progress === "number") {
    updateLoadStage(detail.stage, detail.progress, detail.message ?? "Preparing the WebGPU scene.");
  }
}

async function initialiseRenderer(): Promise<void> {
  if (redirectProductionToHttps()) {
    return;
  }

  if (!window.isSecureContext) {
    showUnsupportedBrowser(
      "A secure connection is required.",
      "WebGPU is available only in a secure context. Open this website over HTTPS and try again.",
    );
    return;
  }

  const navigatorWithGpu = navigator as Navigator & {
    gpu?: unknown;
  };
  if (!("gpu" in navigatorWithGpu) || !navigatorWithGpu.gpu) {
    showUnsupportedBrowser();
    return;
  }

  page.dataset.renderState = "loading";
  loaderPanel.hidden = false;
  unsupportedPanel.hidden = true;
  errorPanel.hidden = true;
  updateLoadStage("matrix", 8, "WebGPU detected. Establishing the matrix signal.");

  rendererTimeout = window.setTimeout(() => {
    if (!rendererReady) {
      showRendererError(new Error("The renderer did not become ready within 30 seconds."));
    }
  }, RENDERER_TIMEOUT_MS);

  try {
    const moduleUrl = versionedRuntimeUrl(WASM_MODULE_PATH).href;
    const bindings = (await import(moduleUrl)) as WasmBindings;
    if (typeof bindings.default !== "function") {
      throw new Error("The generated WebAssembly module has no default initializer.");
    }

    updateLoadStage("matrix", 16, "Starting the Rust/WebGPU renderer.");
    await bindings.default({
      module_or_path: versionedRuntimeUrl(WASM_BINARY_PATH),
    });
    // Exports throw until the module is initialized, and the header's resize
    // observer can fire before then.
    wasmBindings = bindings;
    applySystemMotionPreference();
    reportHeaderHeight();
  } catch (error) {
    showRendererError(error);
  }
}

window.addEventListener("pooya:renderer-ready", handleRendererReady);
window.addEventListener("pooya:renderer-error", handleRendererError);
window.addEventListener("pooya:timeline-change", handleTimelineChange);
window.addEventListener("pooya:scene-progress", handleSceneProgress);
window.addEventListener("error", handleStartupWindowError);
window.addEventListener("unhandledrejection", handleStartupRejection);
window.addEventListener("keydown", handlePrimaryKeyboardAction);

retryButton.addEventListener("click", () => window.location.reload());
motionQuery.addEventListener("change", applySystemMotionPreference);
if (siteHeader) {
  new ResizeObserver(reportHeaderHeight).observe(siteHeader);
}

const currentYear = new Date().getFullYear().toString();
copyrightYear.dateTime = currentYear;
copyrightYear.textContent = currentYear;

void initialiseRenderer();
