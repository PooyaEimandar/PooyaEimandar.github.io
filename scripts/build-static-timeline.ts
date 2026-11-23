import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";

interface TimelineLink {
  label: string;
  url: string;
}

interface TimelineEntry {
  id: string;
  year: string;
  title: string;
  text: string;
  links: TimelineLink[];
  /** Players embedded beside the entry on /timeline/, in order, such as slides or videos. */
  embeds?: TimelineEmbed[];
  /** Pictures beside the entry on /timeline/, in order: each a JPEG in assets/images with a WebP of the same name. */
  images?: TimelineImage[];
}

interface TimelineEmbed {
  /** The player's https address, as the site's own embed code gives it. */
  src: string;
  /** What the player shows, for screen readers. */
  title: string;
  /** The embed code's size; the frame keeps this shape as it shrinks. */
  width: number;
  height: number;
}

interface TimelineImage {
  src: string;
  alt: string;
  width: number;
  height: number;
}

interface TimelineSection {
  id: string;
  title: string;
  range: string;
  entries: TimelineEntry[];
}

interface TimelineData {
  title: string;
  sections: TimelineSection[];
}

type RenderTarget = "home" | "page";

const siteUrl = "https://pooya.ai/";
const personId = `${siteUrl}#person`;
const websiteId = `${siteUrl}#website`;

type PageKind = "home" | "timeline";

// Deliberately not rendered as page copy. This Persian introduction exists so
// search engines can associate both spellings of Pooya's name with a Persian
// description of who he is; it ships, language-tagged beside the English one,
// as `Person.description` in the JSON-LD of both pages instead of as hidden
// text, which search engines discount.
const persianIntroduction = "پویا ایماندار (پویا ایمان دار)، کارآفرین حوزهٔ فناوری در ایران و امارات و از بنیان‌گذاران پلی‌پاد، پلتفرم بازی ابری، است.";
const englishIntroduction =
  "Pooya Eimandar is a technology entrepreneur and graphics engineer. He co-founded PlayPod, the first cloud-gaming platform in Iran and the MENA, wrote DirectX 11.1 Game Programming, co-authored Integrating Rust, and develops Sib, an open-source Rust library for real-time streaming, networking, and WebGPU rendering.";

// One graph for the whole site: every page, note, and tutorial that names
// Pooya points at `personId`, so search engines see a single person.
const sharedGraph = [
  {
    "@type": "WebSite",
    "@id": websiteId,
    url: siteUrl,
    name: "Pooya Eimandar",
    alternateName: "pooya.ai",
    inLanguage: "en",
    publisher: { "@id": personId },
  },
  {
    "@type": "Person",
    "@id": personId,
    name: "Pooya Eimandar",
    alternateName: ["Pooya Imandar", "پویا ایماندار", "پویا ایمان دار"],
    description: [
      { "@language": "en", "@value": englishIntroduction },
      { "@language": "fa", "@value": persianIntroduction },
    ],
    jobTitle: "Technology entrepreneur and graphics engineer",
    worksFor: { "@type": "Organization", name: "AINU Tech Source" },
    alumniOf: { "@type": "CollegeOrUniversity", name: "Shomal University", url: "https://shomal.ac.ir/en/" },
    image: "https://avatars.githubusercontent.com/u/1788248?v=4",
    url: siteUrl,
    sameAs: [
      "https://github.com/PooyaEimandar",
      "https://www.youtube.com/channel/UC5XZoDB5YHd07WSWeMAYyZQ",
    ],
    knowsAbout: ["Rust", "WebGPU", "Real-time graphics", "Game engines", "Cloud gaming"],
  },
  {
    "@type": "Book",
    name: "DirectX 11.1 Game Programming",
    isbn: "9781849694803",
    author: { "@id": personId },
    publisher: { "@type": "Organization", name: "Packt Publishing" },
    datePublished: "2013",
  },
  {
    "@type": "Book",
    name: "Integrating Rust",
    isbn: "9789365898262",
    author: [{ "@id": personId }, { "@type": "Person", name: "Abhishek Kumar" }],
    publisher: { "@type": "Organization", name: "BPB Publications" },
    datePublished: "2024",
  },
];

function pageStructuredData(kind: PageKind): object {
  const page = kind === "home"
    ? {
      "@type": "ProfilePage",
      "@id": `${siteUrl}#profile`,
      url: siteUrl,
      name: "Pooya Eimandar — Technology Entrepreneur & Graphics Engineer",
      isPartOf: { "@id": websiteId },
      mainEntity: { "@id": personId },
      inLanguage: "en",
    }
    : {
      "@type": "WebPage",
      "@id": `${siteUrl}timeline/#webpage`,
      url: `${siteUrl}timeline/`,
      name: "Pooya Eimandar's Timeline",
      isPartOf: { "@id": websiteId },
      about: { "@id": personId },
      inLanguage: "en",
    };
  return { "@context": "https://schema.org", "@graph": [page, ...sharedGraph] };
}

const projectRoot = new URL("../", import.meta.url);
const timelineDataUrl = new URL("data/timeline.json", projectRoot);
const homePageUrl = new URL("index.html", projectRoot);
const timelineDirectoryUrl = new URL("timeline/", projectRoot);
const timelinePageUrl = new URL("timeline/index.html", projectRoot);
const homeTimelineStart = "    <!-- timeline:generated:start -->";
const homeTimelineEnd = "    <!-- timeline:generated:end -->";
const homeStructuredDataStart = "  <!-- structured-data:generated:start -->";
const homeStructuredDataEnd = "  <!-- structured-data:generated:end -->";
const argumentsList = process.argv.slice(2);
const checkOnly = argumentsList.length === 1 && argumentsList[0] === "--check";

if (argumentsList.length > 0 && !checkOnly) {
  throw new Error("Usage: node scripts/build-static-timeline.ts [--check]");
}

function escapeHtml(value: string): string {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function assertNonEmptyString(value: unknown, label: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${label} must be a non-empty string.`);
  }
  return value;
}

function assertSafeId(value: unknown, label: string): string {
  if (typeof value !== "string" || !/^[a-z0-9-]+$/.test(value)) {
    throw new Error(`${label} must contain only lowercase letters, numbers, and hyphens.`);
  }
  return value;
}

function validateUrl(value: unknown, label: string): string {
  let url: URL;
  try {
    url = new URL(assertNonEmptyString(value, label));
  } catch {
    throw new Error(`${label} must be an absolute URL.`);
  }

  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new Error(`${label} uses an unsupported protocol: ${url.protocol}`);
  }
  return url.href;
}

function assertObject(value: unknown, label: string): asserts value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object.`);
  }
}

function validateTimeline(data: unknown): asserts data is TimelineData {
  assertObject(data, "Timeline data");
  assertNonEmptyString(data.title, "Timeline title");
  if (!Array.isArray(data.sections) || data.sections.length === 0) {
    throw new Error("Timeline data must contain at least one section.");
  }

  const sectionIds = new Set<string>();
  const entryIds = new Set<string>();
  const sections: unknown[] = data.sections;
  sections.forEach((section, sectionIndex) => {
    const sectionLabel = `Timeline section ${sectionIndex + 1}`;
    assertObject(section, sectionLabel);
    const sectionId = assertSafeId(section.id, `${sectionLabel} id`);
    if (sectionIds.has(sectionId)) {
      throw new Error(`Timeline section id is duplicated: ${sectionId}`);
    }
    sectionIds.add(sectionId);
    assertNonEmptyString(section.title, `${sectionLabel} title`);
    assertNonEmptyString(section.range, `${sectionLabel} range`);
    if (!Array.isArray(section.entries) || section.entries.length === 0) {
      throw new Error(`${sectionLabel} must contain at least one entry.`);
    }

    const entries: unknown[] = section.entries;
    entries.forEach((entry, entryIndex) => {
      const entryLabel = `${sectionLabel}, entry ${entryIndex + 1}`;
      assertObject(entry, entryLabel);
      const entryId = assertSafeId(entry.id, `${entryLabel} id`);
      if (entryIds.has(entryId)) {
        throw new Error(`Timeline entry id is duplicated: ${entryId}`);
      }
      entryIds.add(entryId);
      if (typeof entry.year !== "string" || !/^\d{4}$/.test(entry.year)) {
        throw new Error(`${entryLabel} has an invalid year.`);
      }
      assertNonEmptyString(entry.title, `${entryLabel} title`);
      assertNonEmptyString(entry.text, `${entryLabel} text`);
      if (!Array.isArray(entry.links)) {
        throw new Error(`${entryLabel} links must be an array.`);
      }
      const links: unknown[] = entry.links;
      links.forEach((link, linkIndex) => {
        const linkLabel = `${entryLabel}, link ${linkIndex + 1}`;
        assertObject(link, linkLabel);
        assertNonEmptyString(link.label, `${linkLabel} label`);
        validateUrl(link.url, `${linkLabel} URL`);
      });
      if (entry.embeds !== undefined) {
        if (!Array.isArray(entry.embeds) || entry.embeds.length === 0) {
          throw new Error(`${entryLabel} embeds must be a non-empty array.`);
        }
        const embeds: unknown[] = entry.embeds;
        embeds.forEach((embed, embedIndex) => validateEmbed(embed, `${entryLabel}, embed ${embedIndex + 1}`));
      }
      if (entry.images !== undefined) {
        if (!Array.isArray(entry.images) || entry.images.length === 0) {
          throw new Error(`${entryLabel} images must be a non-empty array.`);
        }
        const images: unknown[] = entry.images;
        images.forEach((image, imageIndex) => validateImage(image, `${entryLabel}, image ${imageIndex + 1}`));
      }
    });
  });
}

/// Escapes `<` so the payload can never terminate the surrounding script tag.
function renderStructuredData(indent: string, kind: PageKind): string {
  const json = JSON.stringify(pageStructuredData(kind), null, 2)
    .replaceAll("<", "\\u003C")
    .split("\n")
    .map((line) => `${indent}  ${line}`)
    .join("\n");
  return `${indent}<script type="application/ld+json">\n${json}\n${indent}</script>`;
}

function renderLinks(links: TimelineLink[]): string {
  if (links.length === 0) {
    return "";
  }

  const items = links.map((link) => {
    const url = validateUrl(link.url, `Timeline link ${link.label}`);
    return `                  <li><a href="${escapeHtml(url)}" target="_blank" rel="noopener noreferrer">${escapeHtml(link.label)}</a></li>`;
  }).join("\n");

  return `
                <ul class="timeline-entry-links" aria-label="Related links">
${items}
                </ul>`;
}

function validateEmbed(embed: unknown, label: string): void {
  assertObject(embed, label);
  if (!validateUrl(embed.src, `${label} src`).startsWith("https:")) {
    throw new Error(`${label} src must use https.`);
  }
  assertNonEmptyString(embed.title, `${label} title`);
  assertPixelSize(embed, label);
}

function assertPixelSize(value: Record<string, unknown>, label: string): void {
  for (const side of ["width", "height"] as const) {
    if (!Number.isInteger(value[side]) || (value[side] as number) <= 0) {
      throw new Error(`${label} ${side} must be a positive whole number of pixels.`);
    }
  }
}

function validateImage(image: unknown, label: string): void {
  assertObject(image, label);
  const src = assertNonEmptyString(image.src, `${label} src`);
  if (!/^[a-z0-9-]+\.jpg$/.test(src)) {
    throw new Error(`${label} src must be a .jpg file name in assets/images.`);
  }
  assertNonEmptyString(image.alt, `${label} alt`);
  assertPixelSize(image, label);
  // <picture> offers the WebP and falls back to the JPEG, so both must exist.
  for (const file of [src, src.replace(/\.jpg$/, ".webp")]) {
    if (!existsSync(new URL(`assets/images/${file}`, projectRoot))) {
      throw new Error(`${label}: assets/images/${file} does not exist.`);
    }
  }
}

function renderImages(entry: TimelineEntry, target: RenderTarget): string {
  if (target !== "page" || entry.images === undefined) {
    return "";
  }
  const pictures = entry.images.map((image) => {
    const base = `../assets/images/${image.src.replace(/\.jpg$/, "")}`;
    return `
                <picture>
                  <source srcset="${base}.webp" type="image/webp">
                  <img class="timeline-image" src="${base}.jpg" alt="${escapeHtml(image.alt)}" width="${image.width}" height="${image.height}" loading="lazy" decoding="async">
                </picture>`;
  }).join("");
  return `
              <div class="timeline-images">${pictures}
              </div>`;
}

// Only /timeline/ embeds: on the homepage the timeline is hidden behind the
// WebGPU scene, where a frame would load for every visitor and never be seen.
function renderEmbeds(entry: TimelineEntry, target: RenderTarget): string {
  if (target !== "page" || entry.embeds === undefined) {
    return "";
  }
  const frames = entry.embeds.map((embed) => `
                <iframe class="timeline-embed" src="${escapeHtml(embed.src)}" title="${escapeHtml(embed.title)}" style="max-width: ${embed.width}px; aspect-ratio: ${embed.width} / ${embed.height}" loading="lazy" referrerpolicy="strict-origin-when-cross-origin" allow="accelerometer; autoplay; fullscreen; gyroscope; picture-in-picture; clipboard-write; encrypted-media; web-share" allowfullscreen></iframe>`).join("");
  return `
              <div class="timeline-embeds">${frames}
              </div>`;
}

function renderEntry(entry: TimelineEntry, target: RenderTarget): string {
  const id = assertSafeId(entry.id, "Timeline entry id");
  const year = entry.year;
  const heading = target === "home" ? "h4" : "h3";
  const attributes = target === "home"
    ? `data-milestone data-id="${id}" data-year="${year}"`
    : `id="${id}" data-milestone data-id="${id}" data-year="${year}"`;

  return `          <li ${attributes}>
            <time datetime="${year}">${year}</time>
            <div>
              <${heading}>${escapeHtml(entry.title)}</${heading}>
              <p>${escapeHtml(entry.text)}</p>${renderLinks(entry.links)}${renderImages(entry, target)}${renderEmbeds(entry, target)}
            </div>
          </li>`;
}

function renderSection(section: TimelineSection, target: RenderTarget): string {
  const id = assertSafeId(section.id, "Timeline section id");
  const heading = target === "home" ? "h3" : "h2";

  return `      <section class="timeline-group" data-timeline-section="${id}" aria-labelledby="${id}-title">
        <header>
          <p>${escapeHtml(section.range)}</p>
          <${heading} id="${id}-title">${escapeHtml(section.title)}</${heading}>
        </header>
        <ol>
${section.entries.map((entry) => renderEntry(entry, target)).join("\n")}
        </ol>
      </section>`;
}

// Every fact comes from the data, never from the clock: the same JSON has to
// generate the same HTML next year, or `--check` would call this year's stale.
function timelineFacts(data: TimelineData): { firstYear: number; lastYear: number; milestoneCount: number } {
  const years = data.sections.flatMap((section) => section.entries).map((entry) => Number(entry.year));
  return {
    firstYear: Math.min(...years),
    lastYear: Math.max(...years),
    milestoneCount: years.length,
  };
}

function renderHomeTimeline(data: TimelineData): string {
  const { firstYear, lastYear, milestoneCount } = timelineFacts(data);
  const sections = data.sections.map((section) => renderSection(section, "home")).join("\n\n");

  return `    <section class="timeline-copy" id="timeline-copy" aria-labelledby="timeline-copy-title">
      <header class="timeline-copy-header">
        <p class="eyebrow">${firstYear} — ${lastYear}</p>
        <h2 id="timeline-copy-title" aria-label="${escapeHtml(data.title)}"><span aria-hidden="true">pooya@timeline:~$ history
            --all</span></h2>
        <p>${milestoneCount} milestones across graphics, games, publishing, teaching, technology leadership, and cloud
          platforms.</p>
      </header>

${sections}
    </section>`;
}

function renderTimelinePage(data: TimelineData): string {
  const { firstYear, lastYear } = timelineFacts(data);
  const sections = data.sections.map((section) => renderSection(section, "page")).join("\n\n");

  return `<!doctype html>
<!-- Generated by scripts/build-static-timeline.ts from data/timeline.json. -->
<html lang="en">

<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <meta name="theme-color" content="#020806">
  <meta name="color-scheme" content="dark">
  <meta name="description"
    content="Pooya Eimandar's complete timeline across graphics, game engines, publishing, teaching, cloud gaming, and technology leadership.">
  <meta name="author" content="Pooya Eimandar">
  <meta property="og:type" content="profile">
  <meta property="og:title" content="Pooya Eimandar's Timeline">
  <meta property="og:description" content="Pooya Eimandar's milestones across graphics, games, publishing, teaching, and technology leadership.">
  <meta property="og:url" content="https://pooya.ai/timeline/">
  <meta property="og:site_name" content="Pooya Eimandar">
  <meta property="og:image" content="https://pooya.ai/assets/images/social.png">
  <meta property="og:image:width" content="1200">
  <meta property="og:image:height" content="630">
  <meta property="og:image:alt" content="Pooya Eimandar, technology entrepreneur and graphics engineer">
  <meta name="twitter:card" content="summary_large_image">
  <meta name="twitter:title" content="Pooya Eimandar's Timeline">
  <meta name="twitter:description" content="Pooya Eimandar's milestones across graphics, games, publishing, teaching, and technology leadership.">
  <meta name="twitter:image" content="https://pooya.ai/assets/images/social.png">
  <meta name="twitter:image:alt" content="Pooya Eimandar, technology entrepreneur and graphics engineer">
  <link rel="canonical" href="https://pooya.ai/timeline/">
  <link rel="stylesheet" href="../assets/css/site.css">
  <title>Pooya Eimandar's Timeline</title>
${renderStructuredData("  ", "timeline")}
</head>

<body class="timeline-page">
  <a class="skip-link" href="#timeline-content">Skip to the timeline</a>

  <header class="site-header" aria-label="Site header">
    <a class="wordmark" href="/">
      <span class="wordmark-name">Stay Hungry, Stay Foolish</span>
    </a>

    <nav class="primary-nav" aria-label="Primary navigation">
      <a href="/">Home</a>
      <a href="/notes/">Notes</a>
      <a href="/webgpu/">WebGPU</a>
      <a href="https://github.com/sponsors/PooyaEimandar" target="_blank" rel="noopener noreferrer">Sponsor</a>
      <a href="mailto:mail@pooya.ai">Contact</a>
    </nav>
  </header>

  <main id="timeline-content">
    <article class="timeline-copy" aria-labelledby="timeline-page-title">
      <header class="timeline-copy-header">
        <p class="eyebrow">${firstYear} — ${lastYear}</p>
        <h1 id="timeline-page-title">Pooya Eimandar&#39;s Timeline</h1>
        <p>All milestones across graphics, games, publishing, teaching, technology leadership, and cloud platforms.</p>
      </header>

${sections}
    </article>
  </main>

  <footer class="site-footer">
    <p><a href="/">Go to the interactive timeline</a></p>
    <p>© <time id="copyright-year" datetime="${lastYear}">${lastYear}</time> Pooya Eimandar. All rights reserved.</p>
    <nav class="social-links" aria-label="Social profiles">
      <a href="https://www.linkedin.com/in/pooyaeimandar/" target="_blank" rel="noopener noreferrer" aria-label="LinkedIn"><svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true" focusable="false"><path d="M20.447 20.452h-3.554v-5.569c0-1.328-.027-3.037-1.852-3.037-1.853 0-2.136 1.445-2.136 2.939v5.667H9.351V9h3.414v1.561h.046c.477-.9 1.637-1.85 3.37-1.85 3.601 0 4.267 2.37 4.267 5.455v6.286zM5.337 7.433c-1.144 0-2.063-.926-2.063-2.065 0-1.138.92-2.063 2.063-2.063 1.14 0 2.064.925 2.064 2.063 0 1.139-.925 2.065-2.064 2.065zm1.782 13.019H3.555V9h3.564v11.452zM22.225 0H1.771C.792 0 0 .774 0 1.729v20.542C0 23.227.792 24 1.771 24h20.451C23.2 24 24 23.227 24 22.271V1.729C24 .774 23.2 0 22.222 0h.003z"/></svg></a>
      <a href="https://github.com/PooyaEimandar" target="_blank" rel="noopener noreferrer" aria-label="GitHub"><svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true" focusable="false"><path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12"/></svg></a>
      <a href="https://www.youtube.com/@WolfSource" target="_blank" rel="noopener noreferrer" aria-label="YouTube"><svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true" focusable="false"><path d="M23.498 6.186a3.016 3.016 0 0 0-2.122-2.136C19.505 3.545 12 3.545 12 3.545s-7.505 0-9.377.505A3.017 3.017 0 0 0 .502 6.186C0 8.07 0 12 0 12s0 3.93.502 5.814a3.016 3.016 0 0 0 2.122 2.136c1.871.505 9.376.505 9.376.505s7.505 0 9.377-.505a3.015 3.015 0 0 0 2.122-2.136C24 15.93 24 12 24 12s0-3.93-.502-5.814zM9.545 15.568V8.432L15.818 12l-6.273 3.568z"/></svg></a>
    </nav>
  </footer>
  <script>
    // The page is generated ahead of time; this keeps the copyright year current.
    const copyrightYear = document.getElementById("copyright-year");
    copyrightYear.dateTime = copyrightYear.textContent = new Date().getFullYear();
  </script>
</body>

</html>
`;
}

function replaceMarkedRegion(
  homePage: string,
  startMarker: string,
  endMarker: string,
  content: string,
  label: string,
): string {
  const start = homePage.indexOf(startMarker);
  const end = homePage.indexOf(endMarker);
  if (start < 0 || end < 0 || end <= start) {
    throw new Error(`index.html is missing the ${label} markers.`);
  }
  if (homePage.indexOf(startMarker, start + startMarker.length) >= 0
    || homePage.indexOf(endMarker, end + endMarker.length) >= 0) {
    throw new Error(`index.html must contain exactly one ${label} marker pair.`);
  }

  return `${homePage.slice(0, start)}${startMarker}\n${content}\n${endMarker}${homePage.slice(end + endMarker.length)}`;
}

async function writeIfChanged(url: URL, expected: string): Promise<void> {
  let current = "";
  try {
    current = await readFile(url, "utf8");
  } catch (error) {
    if (!(error instanceof Error) || !("code" in error) || error.code !== "ENOENT") {
      throw error;
    }
  }
  if (current !== expected) {
    await writeFile(url, expected, "utf8");
  }
}

/// HTML renders runs of whitespace, and whitespace beside a tag, the same, so
/// an editor that rewraps the generated markup on save does not make it stale.
function renderedText(html: string): string {
  return html.replace(/\s+/g, " ").replace(/\s*(<[^>]*>)\s*/g, "$1").trim();
}

function assertGeneratedFileIsCurrent(actual: string, expected: string, label: string): void {
  if (renderedText(actual) !== renderedText(expected)) {
    throw new Error(`${label} is stale. Run npm run build:timeline.`);
  }
}

const timelineJson = await readFile(timelineDataUrl, "utf8");
const data: unknown = JSON.parse(timelineJson);
validateTimeline(data);

const currentHomePage = await readFile(homePageUrl, "utf8");
const expectedHomePage = replaceMarkedRegion(
  replaceMarkedRegion(
    currentHomePage,
    homeStructuredDataStart,
    homeStructuredDataEnd,
    renderStructuredData("  ", "home"),
    "generated structured data",
  ),
  homeTimelineStart,
  homeTimelineEnd,
  renderHomeTimeline(data),
  "generated timeline",
);
const expectedTimelinePage = renderTimelinePage(data);
const { milestoneCount } = timelineFacts(data);

if (checkOnly) {
  const currentTimelinePage = await readFile(timelinePageUrl, "utf8");
  assertGeneratedFileIsCurrent(currentHomePage, expectedHomePage, "The homepage timeline");
  assertGeneratedFileIsCurrent(currentTimelinePage, expectedTimelinePage, "The static timeline page");
  console.log(`Verified ${milestoneCount} timeline milestones from data/timeline.json.`);
} else {
  await mkdir(timelineDirectoryUrl, { recursive: true });
  await writeIfChanged(homePageUrl, expectedHomePage);
  await writeIfChanged(timelinePageUrl, expectedTimelinePage);
  console.log(`Generated both HTML timelines from ${milestoneCount} canonical milestones.`);
}
