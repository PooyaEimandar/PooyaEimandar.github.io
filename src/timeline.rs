use serde::Deserialize;

const TIMELINE_JSON: &str = include_str!("../data/timeline.json");
// Phone transcripts use fewer columns so the responsive transform can enlarge
// every WebGPU glyph without cropping the canonical timeline content.
const MAX_ENTRIES_PER_SLIDE: usize = 2;
const MAX_TERMINAL_LINES: usize = 22;
const INITIAL_TYPE_DELAY: f32 = 0.28;
const CHARACTER_SECONDS: f32 = 0.0115;
const SPACE_SECONDS: f32 = 0.006;
const PUNCTUATION_SECONDS: f32 = 0.021;
const LINE_PAUSE_SECONDS: f32 = 0.100;
const MAX_TERMINAL_COLUMNS: usize = 42;
const MOBILE_MAX_TERMINAL_COLUMNS: usize = 27;
pub const MOBILE_MAX_TERMINAL_LINES: usize = 19;
pub const MOBILE_MIN_TERMINAL_LINES: usize = 14;
const MOBILE_FOOTER_LINES: usize = 3;
const DESKTOP_HEADER_LINES: usize = 5;
const DESKTOP_FOOTER_LINES: usize = 3;
const DESKTOP_BODY_LINES: usize = MAX_TERMINAL_LINES - DESKTOP_HEADER_LINES - DESKTOP_FOOTER_LINES;
// A continued slide spends one body line on `| # continued`.
const _: () = assert!(DESKTOP_BODY_LINES > 1);

#[derive(Debug, Deserialize)]
struct TimelineDocument {
    title: String,
    sections: Vec<TimelineSection>,
}

#[derive(Debug, Deserialize)]
struct TimelineSection {
    id: String,
    title: String,
    range: String,
    entries: Vec<TimelineEntry>,
}

#[derive(Debug, Deserialize)]
struct TimelineEntry {
    id: String,
    year: String,
    title: String,
    text: String,
    links: Vec<TimelineLink>,
}

#[derive(Debug, Deserialize)]
struct TimelineLink {
    label: String,
    url: String,
}

#[derive(Clone, Debug)]
pub struct TimelineLinkRange {
    pub url: String,
    pub line: usize,
    pub start_column: usize,
    pub end_column: usize,
    pub start_character: usize,
    pub end_character: usize,
}

#[derive(Clone, Debug)]
pub struct TimelineSlide {
    pub eyebrow: String,
    pub heading: String,
    pub summary: String,
    pub terminal: String,
    pub links: Vec<TimelineLinkRange>,
    entry_ids: Vec<String>,
    source_line_start: usize,
    line_count: usize,
    reveal_times: Vec<f32>,
}

impl TimelineSlide {
    pub fn character_count(&self) -> usize {
        self.reveal_times.len().saturating_sub(1)
    }

    pub fn typing_duration(&self) -> f32 {
        self.reveal_times.last().copied().unwrap_or_default()
    }

    pub fn line_count(&self) -> usize {
        self.line_count
    }

    pub fn primary_entry_id(&self) -> &str {
        self.entry_ids
            .first()
            .map(String::as_str)
            .unwrap_or_default()
    }

    pub fn contains_entry(&self, entry_id: &str) -> bool {
        self.entry_ids.iter().any(|candidate| candidate == entry_id)
    }

    pub fn source_line_start(&self) -> usize {
        self.source_line_start
    }

    pub fn typed_characters_at(&self, elapsed: f32) -> usize {
        self.reveal_times
            .partition_point(|time| *time <= elapsed.max(0.0))
            .saturating_sub(1)
            .min(self.character_count())
    }
}

pub fn load_slides() -> Result<Vec<TimelineSlide>, String> {
    let document: TimelineDocument = serde_json::from_str(TIMELINE_JSON)
        .map_err(|error| format!("invalid data/timeline.json: {error}"))?;
    let section_drafts = document
        .sections
        .iter()
        .enumerate()
        .map(|(section_index, section)| paginate_desktop_section(section_index, section))
        .collect::<Vec<_>>();
    let slide_count = section_drafts.iter().map(Vec::len).sum::<usize>();
    let mut slides = Vec::with_capacity(slide_count);

    for (section, drafts) in document.sections.iter().zip(section_drafts) {
        let page_count = drafts.len();
        for (page_index, draft) in drafts.into_iter().enumerate() {
            let slide_index = slides.len();
            let entries = draft
                .entry_indices
                .iter()
                .filter_map(|&entry_index| section.entries.get(entry_index))
                .collect::<Vec<_>>();
            let terminal = build_terminal_stream(
                section,
                &draft.lines,
                page_index,
                page_count,
                slide_index,
                slide_count,
            );
            let line_count = terminal.text.lines().count();
            slides.push(TimelineSlide {
                eyebrow: format!(
                    "{}  ·  {:02}/{:02}",
                    document.title,
                    slide_index + 1,
                    slide_count
                ),
                heading: format!("{}  /  {}", section.title, section.range),
                summary: slide_summary(&entries, draft.continuation),
                reveal_times: typing_schedule(&terminal.text),
                links: terminal.links,
                terminal: terminal.text,
                entry_ids: entries.iter().map(|entry| entry.id.clone()).collect(),
                source_line_start: draft.source_line_start,
                line_count,
            });
        }
    }

    if slides.is_empty() {
        return Err("data/timeline.json has no entries".to_owned());
    }

    for slide in &slides {
        report_over_budget(slide, MAX_TERMINAL_LINES, MAX_TERMINAL_COLUMNS);
    }
    Ok(slides)
}

pub fn load_mobile_slides(max_terminal_lines: usize) -> Result<Vec<TimelineSlide>, String> {
    if !(MOBILE_MIN_TERMINAL_LINES..=MOBILE_MAX_TERMINAL_LINES).contains(&max_terminal_lines) {
        return Err(format!(
            "mobile terminal line limit must be between {MOBILE_MIN_TERMINAL_LINES} and {MOBILE_MAX_TERMINAL_LINES}"
        ));
    }
    let document: TimelineDocument = serde_json::from_str(TIMELINE_JSON)
        .map_err(|error| format!("invalid data/timeline.json: {error}"))?;
    let mut drafts = Vec::new();

    for (section_index, section) in document.sections.iter().enumerate() {
        let header_lines = mobile_header_line_count(section);
        let first_page_capacity = max_terminal_lines
            .checked_sub(header_lines + MOBILE_FOOTER_LINES)
            .filter(|capacity| *capacity > 0)
            .ok_or_else(|| format!("mobile terminal header is too tall for {}", section.id))?;
        let continued_page_capacity = first_page_capacity
            .checked_sub(1)
            .filter(|capacity| *capacity > 0)
            .ok_or_else(|| {
                format!(
                    "mobile terminal continuation is too tall for {}",
                    section.id
                )
            })?;

        for (entry_index, entry) in section.entries.iter().enumerate() {
            push_entry_chunks(
                &mut drafts,
                section_index,
                entry_index,
                &build_mobile_entry_lines(entry),
                first_page_capacity,
                continued_page_capacity,
            );
        }
    }

    if drafts.is_empty() {
        return Err("data/timeline.json has no entries".to_owned());
    }

    let slide_count = drafts.len();
    let mut slides = Vec::with_capacity(slide_count);
    for (slide_index, draft) in drafts.into_iter().enumerate() {
        let Some(section) = document.sections.get(draft.section_index) else {
            crate::log_error(&format!(
                "timeline: mobile slide {} references missing section {}; skipping it.",
                slide_index + 1,
                draft.section_index,
            ));
            continue;
        };
        let entries = draft
            .entry_indices
            .iter()
            .filter_map(|&entry_index| section.entries.get(entry_index))
            .collect::<Vec<_>>();
        let terminal = build_mobile_terminal_stream(section, &draft, slide_index, slide_count);
        let line_count = terminal.text.lines().count();
        slides.push(TimelineSlide {
            eyebrow: format!(
                "{}  ·  {:02}/{:02}",
                document.title,
                slide_index + 1,
                slide_count
            ),
            heading: format!("{}  /  {}", section.title, section.range),
            summary: slide_summary(&entries, draft.continuation),
            reveal_times: typing_schedule(&terminal.text),
            links: terminal.links,
            terminal: terminal.text,
            entry_ids: entries.iter().map(|entry| entry.id.clone()).collect(),
            source_line_start: draft.source_line_start,
            line_count,
        });
    }

    for slide in &slides {
        report_over_budget(slide, max_terminal_lines, MOBILE_MAX_TERMINAL_COLUMNS);
    }
    Ok(slides)
}

/// Reports a slide that outgrew its layout budget without discarding it. The
/// renderer shrinks an over-tall transcript to fit and clips an over-wide one,
/// so a console error is more useful than dropping the milestone.
fn report_over_budget(slide: &TimelineSlide, max_lines: usize, max_columns: usize) {
    if slide.line_count() > max_lines {
        crate::log_error(&format!(
            "timeline: '{}' fills {} lines; the budget is {max_lines}. The slide renders smaller than the rest.",
            slide.primary_entry_id(),
            slide.line_count(),
        ));
    }
    for line in slide
        .terminal
        .lines()
        .filter(|line| line.chars().count() > max_columns)
    {
        crate::log_error(&format!(
            "timeline: '{}' has a {}-column line; the budget is {max_columns}. It may clip: {line}",
            slide.primary_entry_id(),
            line.chars().count(),
        ));
    }
}

fn slide_summary(entries: &[&TimelineEntry], continuation: usize) -> String {
    format!(
        "{} milestone{} — {}{}",
        entries.len(),
        if entries.len() == 1 { "" } else { "s" },
        entries
            .iter()
            .map(|entry| entry.title.as_str())
            .collect::<Vec<_>>()
            .join(" · "),
        if continuation > 0 { " (continued)" } else { "" }
    )
}

/// The body of one slide, between its header and footer.
#[derive(Clone, Debug)]
struct SlideDraft {
    section_index: usize,
    entry_indices: Vec<usize>,
    continuation: usize,
    source_line_start: usize,
    lines: Vec<TerminalLine>,
}

/// Splits one entry across consecutive slides. Each later slide opens with
/// `| # continued`, so it gets its own, one-line-smaller capacity.
fn push_entry_chunks(
    drafts: &mut Vec<SlideDraft>,
    section_index: usize,
    entry_index: usize,
    entry_lines: &[TerminalLine],
    first_page_capacity: usize,
    continued_page_capacity: usize,
) {
    let mut line_start = 0;
    let mut continuation = 0;
    while line_start < entry_lines.len() {
        let capacity = if continuation == 0 {
            first_page_capacity
        } else {
            continued_page_capacity
        };
        let line_end = (line_start + capacity).min(entry_lines.len());
        let mut lines = Vec::with_capacity(line_end - line_start + usize::from(continuation > 0));
        if continuation > 0 {
            lines.push(TerminalLine::plain("| # continued"));
        }
        lines.extend_from_slice(&entry_lines[line_start..line_end]);
        drafts.push(SlideDraft {
            section_index,
            entry_indices: vec![entry_index],
            continuation,
            source_line_start: line_start,
            lines,
        });
        line_start = line_end;
        continuation += 1;
    }
}

fn mobile_header_line_count(section: &TimelineSection) -> usize {
    const HEADING_PREFIX_COLUMNS: usize = 13; // `| > 00/00 :: `
    4 + wrap_terminal_words(
        &terminal_ascii(&section.title),
        MOBILE_MAX_TERMINAL_COLUMNS - HEADING_PREFIX_COLUMNS,
    )
    .len()
}

/// `| $ 2026 :: AINU` opens a milestone. A wrapped title continues aligned
/// under its first word; `rail` keeps the mobile `|` gutter.
fn append_entry_heading(
    lines: &mut Vec<TerminalLine>,
    entry: &TimelineEntry,
    rail: &str,
    max_columns: usize,
) {
    let prefix = format!("| $ {} :: ", terminal_ascii(&entry.year));
    let continuation = format!(
        "{rail}{}",
        " ".repeat(prefix.chars().count() - rail.chars().count())
    );
    append_wrapped_body_with_columns(
        lines,
        &prefix,
        &continuation,
        &terminal_ascii(&entry.title),
        max_columns,
    );
}

fn build_mobile_entry_lines(entry: &TimelineEntry) -> Vec<TerminalLine> {
    let mut lines = Vec::new();
    append_entry_heading(&mut lines, entry, "|", MOBILE_MAX_TERMINAL_COLUMNS);
    append_wrapped_body_with_columns(
        &mut lines,
        "| > ",
        "|   ",
        &terminal_ascii(&entry.text),
        MOBILE_MAX_TERMINAL_COLUMNS,
    );
    append_mobile_link_lines(&mut lines, &entry.links);
    lines
}

fn build_mobile_terminal_stream(
    section: &TimelineSection,
    draft: &SlideDraft,
    slide_index: usize,
    slide_count: usize,
) -> TerminalBuild {
    let mut lines = vec![
        TerminalLine::plain("| $ pooya.timeline"),
        TerminalLine::plain(format!("| # {}", terminal_ascii(&section.id))),
    ];
    let heading_prefix = format!("| > {:02}/{:02} :: ", slide_index + 1, slide_count);
    append_wrapped_body_with_columns(
        &mut lines,
        &heading_prefix,
        "|   ",
        &terminal_ascii(&section.title),
        MOBILE_MAX_TERMINAL_COLUMNS,
    );
    lines.extend([
        TerminalLine::plain(format!("| > RANGE :: {}", terminal_ascii(&section.range))),
        TerminalLine::plain("|"),
    ]);
    lines.extend(draft.lines.iter().cloned());
    lines.extend([
        TerminalLine::plain("|"),
        TerminalLine::plain("| $ Click/Touch/⏎"),
        TerminalLine::plain("| > to continue :: READY"),
    ]);
    finish_terminal(lines)
}

fn build_desktop_entry_lines(entry: &TimelineEntry) -> Vec<TerminalLine> {
    let mut lines = Vec::new();
    append_entry_heading(&mut lines, entry, "", MAX_TERMINAL_COLUMNS);
    append_wrapped_body(&mut lines, "| > ", &terminal_ascii(&entry.text));
    append_link_lines(&mut lines, &entry.links);
    lines
}

/// Pairs up to `MAX_ENTRIES_PER_SLIDE` whole entries per slide. An entry too
/// tall for one slide gets consecutive slides of its own, split like the
/// mobile transcript, and the next entry starts on a fresh slide.
fn paginate_desktop_section(section_index: usize, section: &TimelineSection) -> Vec<SlideDraft> {
    let mut drafts = Vec::new();
    for (entry_index, entry) in section.entries.iter().enumerate() {
        let entry_lines = build_desktop_entry_lines(entry);
        if entry_lines.len() > DESKTOP_BODY_LINES {
            push_entry_chunks(
                &mut drafts,
                section_index,
                entry_index,
                &entry_lines,
                DESKTOP_BODY_LINES,
                DESKTOP_BODY_LINES - 1,
            );
            continue;
        }

        // A continued slide never takes another entry. Otherwise a blank rail
        // separates the entries; the header already ends with one.
        if let Some(draft) = drafts.last_mut().filter(|draft: &&mut SlideDraft| {
            draft.continuation == 0
                && draft.entry_indices.len() < MAX_ENTRIES_PER_SLIDE
                && draft.lines.len() + 1 + entry_lines.len() <= DESKTOP_BODY_LINES
        }) {
            draft.lines.push(TerminalLine::plain("|"));
            draft.lines.extend(entry_lines);
            draft.entry_indices.push(entry_index);
        } else {
            drafts.push(SlideDraft {
                section_index,
                entry_indices: vec![entry_index],
                continuation: 0,
                source_line_start: 0,
                lines: entry_lines,
            });
        }
    }
    drafts
}

fn build_terminal_stream(
    section: &TimelineSection,
    body: &[TerminalLine],
    page_index: usize,
    page_count: usize,
    slide_index: usize,
    slide_count: usize,
) -> TerminalBuild {
    let mut lines = vec![
        TerminalLine::plain("| $ pooya.timeline"),
        TerminalLine::plain(format!(
            "| # {} {}/{}",
            terminal_ascii(&section.id),
            page_index + 1,
            page_count
        )),
        TerminalLine::plain(format!(
            "| > {:02}/{:02} :: {}",
            slide_index + 1,
            slide_count,
            terminal_ascii(&section.title)
        )),
        TerminalLine::plain(format!("| > RANGE :: {}", terminal_ascii(&section.range))),
        TerminalLine::plain("|"),
    ];
    lines.extend(body.iter().cloned());
    lines.extend([
        TerminalLine::plain("|"),
        TerminalLine::plain("| $ Click/Touch/⏎ to continue"),
        TerminalLine::plain("| > READY"),
    ]);
    finish_terminal(lines)
}

struct TerminalBuild {
    text: String,
    links: Vec<TimelineLinkRange>,
}

#[derive(Clone, Debug)]
struct TerminalLine {
    text: String,
    links: Vec<LineLinkRange>,
}

impl TerminalLine {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            links: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct LineLinkRange {
    url: String,
    start_column: usize,
    end_column: usize,
}

fn append_wrapped_body(lines: &mut Vec<TerminalLine>, prefix: &str, body: &str) {
    let continuation = " ".repeat(prefix.chars().count().min(8));
    append_wrapped_body_with_columns(lines, prefix, &continuation, body, MAX_TERMINAL_COLUMNS);
}

fn append_wrapped_body_with_columns(
    lines: &mut Vec<TerminalLine>,
    prefix: &str,
    continuation: &str,
    body: &str,
    max_columns: usize,
) {
    let prefix_columns = prefix.chars().count();
    let continuation_columns = continuation.chars().count();
    let content_columns = max_columns.saturating_sub(prefix_columns.max(continuation_columns));
    for (index, content) in wrap_terminal_words(body, content_columns)
        .into_iter()
        .enumerate()
    {
        let line_prefix = if index == 0 { prefix } else { continuation };
        lines.push(TerminalLine::plain(format!("{line_prefix}{content}")));
    }
}

fn wrap_terminal_words(body: &str, max_columns: usize) -> Vec<String> {
    let max_columns = max_columns.max(1);
    let mut wrapped = Vec::new();
    let mut line = String::new();

    for word in body.split_whitespace() {
        let word_columns = word.chars().count();
        let separator = usize::from(!line.is_empty());
        if !line.is_empty() && line.chars().count() + separator + word_columns > max_columns {
            wrapped.push(std::mem::take(&mut line));
        }

        if word_columns <= max_columns {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
            continue;
        }

        if !line.is_empty() {
            wrapped.push(std::mem::take(&mut line));
        }
        let mut remaining = word.chars().peekable();
        while remaining.peek().is_some() {
            let chunk = remaining.by_ref().take(max_columns).collect::<String>();
            if remaining.peek().is_some() {
                wrapped.push(chunk);
            } else {
                line = chunk;
            }
        }
    }

    if !line.is_empty() || wrapped.is_empty() {
        wrapped.push(line);
    }
    wrapped
}

fn append_link_lines(lines: &mut Vec<TerminalLine>, links: &[TimelineLink]) {
    if links.is_empty() {
        return;
    }

    let prefix = "@ ";
    let mut line = TerminalLine::plain(prefix);
    for link in links {
        let label = terminal_ascii(&link.label);
        let token = format!("[{label}]");
        let separator = usize::from(!line.links.is_empty()) * 2;
        if !line.links.is_empty()
            && line.text.chars().count() + separator + token.chars().count() > MAX_TERMINAL_COLUMNS
        {
            lines.push(line);
            line = TerminalLine::plain(prefix);
        }
        if !line.links.is_empty() {
            line.text.push_str("  ");
        }
        let start_column = line.text.chars().count();
        line.text.push_str(&token);
        let end_column = line.text.chars().count();
        line.links.push(LineLinkRange {
            url: link.url.clone(),
            start_column,
            end_column,
        });
    }
    lines.push(line);
}

fn append_mobile_link_lines(lines: &mut Vec<TerminalLine>, links: &[TimelineLink]) {
    const LINK_CHROME_COLUMNS: usize = 4; // `@ [` + `]`
    let label_columns = MOBILE_MAX_TERMINAL_COLUMNS - LINK_CHROME_COLUMNS;
    for link in links {
        for label in wrap_terminal_words(&terminal_ascii(&link.label), label_columns) {
            let text = format!("@ [{label}]");
            lines.push(TerminalLine {
                links: vec![LineLinkRange {
                    url: link.url.clone(),
                    start_column: 2,
                    end_column: text.chars().count(),
                }],
                text,
            });
        }
    }
}

fn finish_terminal(lines: Vec<TerminalLine>) -> TerminalBuild {
    let mut text = String::new();
    let mut links = Vec::new();

    for (line_index, line) in lines.into_iter().enumerate() {
        if line_index > 0 {
            text.push('\n');
        }
        let line_start = text.chars().count();
        text.push_str(&line.text);
        for link in line.links {
            links.push(TimelineLinkRange {
                url: link.url,
                line: line_index,
                start_column: link.start_column,
                end_column: link.end_column,
                start_character: line_start + link.start_column,
                end_character: line_start + link.end_column,
            });
        }
    }
    TerminalBuild { text, links }
}

fn typing_schedule(terminal: &str) -> Vec<f32> {
    let mut elapsed = INITIAL_TYPE_DELAY;
    let mut reveal_times = Vec::with_capacity(terminal.chars().count() + 1);
    reveal_times.push(0.0);

    for character in terminal.chars() {
        elapsed += match character {
            '\n' => LINE_PAUSE_SECONDS,
            ' ' => SPACE_SECONDS,
            ':' | ';' | ',' | '.' | '/' | '[' | ']' | '"' => PUNCTUATION_SECONDS,
            _ => CHARACTER_SECONDS,
        };
        reveal_times.push(elapsed);
    }
    reveal_times
}

fn terminal_ascii(text: &str) -> String {
    text.replace(['\r', '\n', '\t'], " ")
        .replace(['–', '—'], "-")
        .replace(['‘', '’'], "'")
        .replace(['“', '”'], "\"")
        .replace('ó', "o")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `| $` prompt after the opening one starts a new block, so it sits
    /// after exactly one blank `|` rail line.
    fn assert_prompts_follow_one_blank_rail(terminal: &str) {
        let lines = terminal.lines().collect::<Vec<_>>();
        for (index, line) in lines.iter().enumerate().skip(1) {
            if line.starts_with("| $ ") {
                assert_eq!(lines[index - 1], "|", "no blank rail before: {line}");
                assert!(
                    index < 2 || lines[index - 2] != "|",
                    "more than one blank rail before: {line}"
                );
            }
        }
    }

    /// `| $ 1987 :: Born` opens every milestone; `| $ pooya.timeline` and the
    /// footer prompts never look like a year heading.
    fn is_entry_heading_line(line: &str) -> bool {
        line.strip_prefix("| $ ")
            .and_then(|rest| rest.split_once(" :: "))
            .is_some_and(|(year, _)| {
                year.len() == 4 && year.chars().all(|digit| digit.is_ascii_digit())
            })
    }

    #[test]
    fn canonical_timeline_keeps_all_entries_in_responsive_terminal_sessions() {
        let document: TimelineDocument = serde_json::from_str(TIMELINE_JSON).unwrap();
        assert!(!document.sections.is_empty());
        assert!(
            document
                .sections
                .iter()
                .map(|section| section.entries.len())
                .sum::<usize>()
                > 0
        );

        let slides = load_slides().unwrap();
        assert!(slides.len() >= document.sections.len());
        let expected_links = document
            .sections
            .iter()
            .flat_map(|section| &section.entries)
            .map(|entry| entry.links.len())
            .sum::<usize>();
        assert_eq!(
            slides.iter().map(|slide| slide.links.len()).sum::<usize>(),
            expected_links
        );
        for (index, slide) in slides.iter().enumerate() {
            assert!(slide.terminal.starts_with("| $ pooya.timeline"));
            assert_prompts_follow_one_blank_rail(&slide.terminal);
            assert!(slide.terminal.ends_with("| > READY"));
            assert!(!slide.summary.is_empty());
            assert!(
                slide
                    .terminal
                    .chars()
                    .all(|character| character.is_ascii() || character == '⏎')
            );
            assert!(slide.terminal.contains("| $ Click/Touch/⏎ to continue"));
            assert_eq!(slide.line_count(), slide.terminal.lines().count());
            assert!(
                slide.line_count() <= MAX_TERMINAL_LINES,
                "desktop slide {} has {} lines",
                index + 1,
                slide.line_count()
            );
            assert!(
                !slide.terminal.contains("..."),
                "terminal bodies must never be abbreviated"
            );
            for line in slide.terminal.lines() {
                assert!(
                    line.chars().count() <= MAX_TERMINAL_COLUMNS,
                    "terminal line has {} columns: {line}",
                    line.chars().count()
                );
            }
            assert_eq!(slide.character_count(), slide.terminal.chars().count());
            assert!(slide.typing_duration() > INITIAL_TYPE_DELAY);
            assert_eq!(slide.typed_characters_at(0.0), 0);
            assert_eq!(
                slide.typed_characters_at(slide.typing_duration() + 1.0),
                slide.character_count()
            );
            assert!(slide.reveal_times.windows(2).all(|pair| pair[0] < pair[1]));
            println!(
                "terminal {:02}/{:02}: {} lines, {} characters, {} links, {:.2}s typing",
                index + 1,
                slides.len(),
                slide.terminal.lines().count(),
                slide.character_count(),
                slide.links.len(),
                slide.typing_duration()
            );
            for link in &slide.links {
                assert!(link.url.starts_with("https://"));
                assert!(link.start_column < link.end_column);
                assert!(link.start_character < link.end_character);
                let visible = slide
                    .terminal
                    .chars()
                    .skip(link.start_character)
                    .take(link.end_character - link.start_character)
                    .collect::<String>();
                assert!(visible.starts_with('[') && visible.ends_with(']'));
            }
        }

        // Rebuild every entry from the slides it spans: drop each slide's
        // header, footer, and `| # continued` marker, then split the body at
        // the blank rails that separate paired entries.
        let entries = document
            .sections
            .iter()
            .flat_map(|section| &section.entries)
            .collect::<Vec<_>>();
        let mut rebuilt = vec![Vec::<String>::new(); entries.len()];
        for slide in &slides {
            let lines = slide.terminal.lines().collect::<Vec<_>>();
            let mut body = &lines[DESKTOP_HEADER_LINES..lines.len() - DESKTOP_FOOTER_LINES];
            if body.first() == Some(&"| # continued") {
                body = &body[1..];
            }
            let segments = body.split(|line| *line == "|").collect::<Vec<_>>();
            assert_eq!(segments.len(), slide.entry_ids.len());
            for (entry_id, segment) in slide.entry_ids.iter().zip(segments) {
                let index = entries
                    .iter()
                    .position(|entry| &entry.id == entry_id)
                    .expect("slide entry exists in the document");
                rebuilt[index].extend(segment.iter().map(|line| (*line).to_owned()));
            }
        }
        for (entry, rebuilt_lines) in entries.iter().zip(&rebuilt) {
            let expected = build_desktop_entry_lines(entry)
                .into_iter()
                .map(|line| line.text)
                .collect::<Vec<_>>();
            assert_eq!(
                *rebuilt_lines, expected,
                "desktop pagination changed the rendered lines for {}",
                entry.id
            );

            let flattened = rebuilt_lines
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                flattened.starts_with(&format!(
                    "| $ {} :: {}",
                    terminal_ascii(&entry.year),
                    terminal_ascii(&entry.title)
                )),
                "desktop transcript omitted or changed the heading for {}",
                entry.id
            );
            assert!(
                flattened.contains(&terminal_ascii(&entry.text)),
                "desktop transcript omitted or changed the body for {}",
                entry.id
            );
            for link in &entry.links {
                let token = format!("[{}]", terminal_ascii(&link.label));
                assert!(
                    rebuilt_lines.iter().any(|line| line.contains(&token)),
                    "desktop transcript omitted or truncated link token {token}"
                );
            }
        }
    }

    #[test]
    fn desktop_splits_an_entry_taller_than_one_slide() {
        let entry = |id: &str, words: usize| TimelineEntry {
            id: id.to_owned(),
            year: "2026".to_owned(),
            title: id.to_owned(),
            text: vec!["milestone"; words].join(" "),
            links: Vec::new(),
        };
        let section = TimelineSection {
            id: "split".to_owned(),
            title: "Split".to_owned(),
            range: "2026".to_owned(),
            entries: vec![
                entry("first", 3),
                entry("second", 3),
                entry("tall", 300),
                entry("after", 3),
            ],
        };
        let tall_lines = build_desktop_entry_lines(&section.entries[2]);
        assert!(tall_lines.len() > DESKTOP_BODY_LINES * 2);

        let drafts = paginate_desktop_section(0, &section);
        for draft in &drafts {
            assert!(draft.lines.len() <= DESKTOP_BODY_LINES);
        }

        // Short entries still pair; the tall one never shares a slide.
        assert_eq!(drafts[0].entry_indices, [0, 1]);
        let tall = drafts
            .iter()
            .filter(|draft| draft.entry_indices == [2])
            .collect::<Vec<_>>();
        assert!(tall.len() >= 3);
        assert_eq!(drafts.last().unwrap().entry_indices, [3]);
        assert_eq!(drafts.len(), 1 + tall.len() + 1);

        let mut rejoined = Vec::new();
        for (continuation, draft) in tall.iter().enumerate() {
            assert_eq!(draft.continuation, continuation);
            assert_eq!(draft.source_line_start, rejoined.len());
            let mut lines = draft.lines.iter().map(|line| line.text.as_str());
            if continuation > 0 {
                assert_eq!(lines.next(), Some("| # continued"));
            }
            rejoined.extend(lines);
        }
        assert_eq!(
            rejoined,
            tall_lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn mobile_timeline_reflows_every_entry_without_clipping_or_omission() {
        let document: TimelineDocument = serde_json::from_str(TIMELINE_JSON).unwrap();
        let slides = load_mobile_slides(MOBILE_MAX_TERMINAL_LINES).unwrap();
        let entry_count = document
            .sections
            .iter()
            .map(|section| section.entries.len())
            .sum::<usize>();

        assert!(slides.len() >= entry_count);
        assert_eq!(
            slides
                .iter()
                .map(|slide| slide
                    .terminal
                    .lines()
                    .filter(|line| is_entry_heading_line(line))
                    .count())
                .sum::<usize>(),
            entry_count
        );
        for slide in &slides {
            assert!(slide.line_count() <= MOBILE_MAX_TERMINAL_LINES);
            assert_prompts_follow_one_blank_rail(&slide.terminal);
            assert!(slide.terminal.ends_with("| > to continue :: READY"));
            for line in slide.terminal.lines() {
                assert!(
                    line.chars().count() <= MOBILE_MAX_TERMINAL_COLUMNS,
                    "mobile terminal line has {} columns: {line}",
                    line.chars().count()
                );
            }
            for link in &slide.links {
                assert!(link.start_column < link.end_column);
                assert!(link.end_column <= MOBILE_MAX_TERMINAL_COLUMNS);
                assert!(link.end_character <= slide.character_count());
            }
        }

        let rendered_urls = slides
            .iter()
            .flat_map(|slide| slide.links.iter().map(|link| link.url.as_str()))
            .collect::<Vec<_>>();
        for section in &document.sections {
            for entry in &section.entries {
                let entry_lines = build_mobile_entry_lines(entry);
                assert!(
                    entry_lines
                        .iter()
                        .all(|line| line.text.chars().count() <= MOBILE_MAX_TERMINAL_COLUMNS)
                );

                let mut paginated_lines = Vec::new();
                let mut paginated_links = Vec::new();
                for slide in slides
                    .iter()
                    .filter(|slide| slide.contains_entry(&entry.id))
                {
                    let terminal_lines = slide.terminal.lines().collect::<Vec<_>>();
                    let content_start = mobile_header_line_count(section);
                    let content_end = terminal_lines.len() - MOBILE_FOOTER_LINES;
                    let mut content = &terminal_lines[content_start..content_end];
                    if content.first() == Some(&"| # continued") {
                        content = &content[1..];
                    }
                    paginated_lines.extend(content.iter().map(|line| (*line).to_owned()));
                    paginated_links.extend(slide.links.iter().map(|link| {
                        (
                            link.url.clone(),
                            slide
                                .terminal
                                .chars()
                                .skip(link.start_character)
                                .take(link.end_character - link.start_character)
                                .collect::<String>(),
                        )
                    }));
                }
                assert_eq!(
                    paginated_lines,
                    entry_lines
                        .iter()
                        .map(|line| line.text.clone())
                        .collect::<Vec<_>>(),
                    "mobile pagination changed the rendered lines for {}",
                    entry.id
                );
                let expected_links = entry_lines
                    .iter()
                    .flat_map(|line| {
                        line.links.iter().map(|link| {
                            (
                                link.url.clone(),
                                line.text
                                    .chars()
                                    .skip(link.start_column)
                                    .take(link.end_column - link.start_column)
                                    .collect::<String>(),
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                assert_eq!(paginated_links, expected_links);

                // Continuation lines must align under the title's first word,
                // so stripping the heading-prefix width from every line
                // rebuilds the title exactly.
                let heading_prefix = format!("| $ {} :: ", terminal_ascii(&entry.year));
                assert!(
                    entry_lines
                        .first()
                        .is_some_and(|line| line.text.starts_with(&heading_prefix)),
                    "mobile reflow changed the heading prompt for {}",
                    entry.id
                );
                let title = entry_lines
                    .iter()
                    .take_while(|line| !line.text.starts_with("| > "))
                    .map(|line| {
                        line.text
                            .chars()
                            .skip(heading_prefix.chars().count())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                assert_eq!(
                    title,
                    terminal_ascii(&entry.title),
                    "mobile reflow changed the title for {}",
                    entry.id
                );

                let body_start = entry_lines
                    .iter()
                    .position(|line| line.text.starts_with("| > "))
                    .expect("entry body line");
                let body = entry_lines[body_start..]
                    .iter()
                    .take_while(|line| !line.text.starts_with("@ ["))
                    .map(|line| line.text.chars().skip(4).collect::<String>())
                    .collect::<Vec<_>>()
                    .join(" ");
                assert_eq!(
                    body,
                    terminal_ascii(&entry.text),
                    "mobile reflow changed the body for {}",
                    entry.id
                );

                for link in &entry.links {
                    assert!(rendered_urls.contains(&link.url.as_str()));
                    let rendered_label = entry_lines
                        .iter()
                        .filter(|line| line.links.iter().any(|range| range.url == link.url))
                        .map(|line| {
                            line.text
                                .strip_prefix("@ [")
                                .and_then(|text| text.strip_suffix(']'))
                                .expect("mobile link token")
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    assert_eq!(rendered_label, terminal_ascii(&link.label));
                }
            }
        }
    }
}
