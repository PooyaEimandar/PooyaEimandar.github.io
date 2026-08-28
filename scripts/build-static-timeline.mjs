import { mkdir, readFile, writeFile } from "node:fs/promises";

const projectRoot = new URL("../", import.meta.url);
const timelineDataUrl = new URL("data/timeline.json", projectRoot);
const homePageUrl = new URL("index.html", projectRoot);
const timelineDirectoryUrl = new URL("timeline/", projectRoot);
const timelinePageUrl = new URL("timeline/index.html", projectRoot);
const homeTimelineStart = "    <!-- timeline:generated:start -->";
const homeTimelineEnd = "    <!-- timeline:generated:end -->";
const argumentsList = process.argv.slice(2);
const checkOnly = argumentsList.length === 1 && argumentsList[0] === "--check";

if (argumentsList.length > 0 && !checkOnly) {
  throw new Error("Usage: node scripts/build-static-timeline.mjs [--check]");
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function assertNonEmptyString(value, label) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${label} must be a non-empty string.`);
  }
  return value;
}

function assertSafeId(value, label) {
  if (typeof value !== "string" || !/^[a-z0-9-]+$/.test(value)) {
    throw new Error(`${label} must contain only lowercase letters, numbers, and hyphens.`);
  }
  return value;
}

function validateUrl(value, label) {
  let url;
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

function validateTimeline(data) {
  if (!data || typeof data !== "object") {
    throw new Error("Timeline data must be an object.");
  }
  assertNonEmptyString(data.title, "Timeline title");
  if (!Array.isArray(data.sections) || data.sections.length === 0) {
    throw new Error("Timeline data must contain at least one section.");
  }

  const sectionIds = new Set();
  const entryIds = new Set();
  data.sections.forEach((section, sectionIndex) => {
    const sectionLabel = `Timeline section ${sectionIndex + 1}`;
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

    section.entries.forEach((entry, entryIndex) => {
      const entryLabel = `${sectionLabel}, entry ${entryIndex + 1}`;
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
      entry.links.forEach((link, linkIndex) => {
        const linkLabel = `${entryLabel}, link ${linkIndex + 1}`;
        assertNonEmptyString(link.label, `${linkLabel} label`);
        validateUrl(link.url, `${linkLabel} URL`);
      });
    });
  });
}

function renderLinks(links) {
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

function renderEntry(entry, target) {
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
              <p>${escapeHtml(entry.text)}</p>${renderLinks(entry.links)}
            </div>
          </li>`;
}

function renderSection(section, target) {
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

function timelineFacts(data) {
  const entries = data.sections.flatMap((section) => section.entries);
  return {
    firstYear: Math.min(...entries.map((entry) => Number(entry.year))),
    milestoneCount: entries.length,
  };
}

function renderHomeTimeline(data, currentYear) {
  const { firstYear, milestoneCount } = timelineFacts(data);
  const sections = data.sections.map((section) => renderSection(section, "home")).join("\n\n");

  return `    <section class="timeline-copy" id="timeline-copy" aria-labelledby="timeline-copy-title">
      <header class="timeline-copy-header">
        <p class="eyebrow">${firstYear} — ${currentYear}</p>
        <h2 id="timeline-copy-title" aria-label="${escapeHtml(data.title)}"><span aria-hidden="true">pooya@timeline:~$ history
            --all</span></h2>
        <p>${milestoneCount} milestones across graphics, games, publishing, teaching, technology leadership, and cloud
          platforms.</p>
      </header>

${sections}
    </section>`;
}

function renderTimelinePage(data, currentYear) {
  const { firstYear, milestoneCount } = timelineFacts(data);
  const sections = data.sections.map((section) => renderSection(section, "page")).join("\n\n");

  return `<!doctype html>
<!-- Generated by scripts/build-static-timeline.mjs from data/timeline.json. -->
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
  <link rel="canonical" href="https://pooya.ai/timeline/">
  <link rel="stylesheet" href="../assets/css/site.css">
  <title>Pooya Eimandar's Timeline</title>
</head>

<body class="timeline-page">
  <a class="skip-link" href="#timeline-content">Skip to the timeline</a>

  <header class="site-header" aria-label="Site header">
    <a class="wordmark" href="/" aria-label="Return to the WebGPU experience">
      <span class="wordmark-name">Stay Hungry, Stay Foolish</span>
    </a>

    <nav class="primary-nav" aria-label="Primary navigation">
      <a href="/">Home</a>
      <a href="https://github.com/PooyaEimandar" target="_blank" rel="noopener noreferrer">GitHub</a>
      <a href="https://www.youtube.com/channel/UC5XZoDB5YHd07WSWeMAYyZQ" target="_blank"
        rel="noopener noreferrer">YouTube</a>
      <a href="https://github.com/sponsors/PooyaEimandar" target="_blank" rel="noopener noreferrer">Sponsor</a>
      <a href="mailto:mail@pooya.ai">Contact</a>
    </nav>
  </header>

  <main id="timeline-content">
    <article class="timeline-copy" aria-labelledby="timeline-page-title">
      <header class="timeline-copy-header">
        <p class="eyebrow">${firstYear} — ${currentYear}</p>
        <h1 id="timeline-page-title">${escapeHtml(data.title)}</h1>
        <p>${milestoneCount} milestones across graphics, games, publishing, teaching, technology leadership, and cloud platforms.</p>
      </header>

${sections}
    </article>
  </main>

  <footer class="site-footer">
    <p><a href="/">Open the interactive WebGPU timeline</a></p>
    <p>© ${currentYear} Pooya Eimandar. All rights reserved.</p>
  </footer>
</body>

</html>
`;
}

function replaceHomeTimeline(homePage, timeline) {
  const start = homePage.indexOf(homeTimelineStart);
  const end = homePage.indexOf(homeTimelineEnd);
  if (start < 0 || end < 0 || end <= start) {
    throw new Error("index.html is missing the generated timeline markers.");
  }
  if (homePage.indexOf(homeTimelineStart, start + homeTimelineStart.length) >= 0
    || homePage.indexOf(homeTimelineEnd, end + homeTimelineEnd.length) >= 0) {
    throw new Error("index.html must contain exactly one generated timeline marker pair.");
  }

  return `${homePage.slice(0, start)}${homeTimelineStart}\n${timeline}\n${homeTimelineEnd}${homePage.slice(end + homeTimelineEnd.length)}`;
}

async function writeIfChanged(url, expected) {
  let current = "";
  try {
    current = await readFile(url, "utf8");
  } catch (error) {
    if (error?.code !== "ENOENT") {
      throw error;
    }
  }
  if (current !== expected) {
    await writeFile(url, expected, "utf8");
  }
}

function assertGeneratedFileIsCurrent(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`${label} is stale. Run npm run build:timeline.`);
  }
}

const timelineJson = await readFile(timelineDataUrl, "utf8");
const data = JSON.parse(timelineJson);
validateTimeline(data);

const currentYear = new Date().getUTCFullYear();
const currentHomePage = await readFile(homePageUrl, "utf8");
const expectedHomePage = replaceHomeTimeline(currentHomePage, renderHomeTimeline(data, currentYear));
const expectedTimelinePage = renderTimelinePage(data, currentYear);
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
