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
    let section_pages = document
        .sections
        .iter()
        .map(paginate_section_entries)
        .collect::<Vec<_>>();
    let slide_count = section_pages.iter().map(Vec::len).sum::<usize>();
    let mut slides = Vec::with_capacity(slide_count);

    for (section, pages) in document.sections.iter().zip(section_pages) {
        let page_count = pages.len();
        for (page_index, entries) in pages.into_iter().enumerate() {
            let slide_index = slides.len();
            let terminal = build_terminal_stream(
                section,
                entries,
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
                summary: format!(
                    "{} milestone{} — {}",
                    entries.len(),
                    if entries.len() == 1 { "" } else { "s" },
                    entries
                        .iter()
                        .map(|entry| entry.title.as_str())
                        .collect::<Vec<_>>()
                        .join(" · ")
                ),
                reveal_times: typing_schedule(&terminal.text),
                links: terminal.links,
                terminal: terminal.text,
                entry_ids: entries.iter().map(|entry| entry.id.clone()).collect(),
                source_line_start: 0,
                line_count,
            });
        }
    }

    if slides.is_empty() {
        return Err("data/timeline.json has no entries".to_owned());
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
        let mut section_drafts = Vec::new();
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
            let entry_lines = build_mobile_entry_lines(entry);
            let mut line_start = 0;
            let mut continuation = 0;
            while line_start < entry_lines.len() {
                let capacity = if continuation == 0 {
                    first_page_capacity
                } else {
                    continued_page_capacity
                };
                let line_end = (line_start + capacity).min(entry_lines.len());
                let mut lines =
                    Vec::with_capacity(line_end - line_start + usize::from(continuation > 0));
                if continuation > 0 {
                    lines.push(TerminalLine::plain("| # continued"));
                }
                lines.extend_from_slice(&entry_lines[line_start..line_end]);
                section_drafts.push(MobileSlideDraft {
                    section_index,
                    entry_index,
                    continuation,
                    source_line_start: line_start,
                    lines,
                });
                line_start = line_end;
                continuation += 1;
            }
        }
        drafts.extend(section_drafts);
    }

    if drafts.is_empty() {
        return Err("data/timeline.json has no entries".to_owned());
    }

    let slide_count = drafts.len();
    let mut slides = Vec::with_capacity(slide_count);
    for (slide_index, draft) in drafts.into_iter().enumerate() {
        let section = &document.sections[draft.section_index];
        let entry = &section.entries[draft.entry_index];
        let terminal = build_mobile_terminal_stream(section, &draft, slide_index, slide_count);
        let line_count = terminal.text.lines().count();
        debug_assert!(line_count <= max_terminal_lines);
        debug_assert!(
            terminal
                .text
                .lines()
                .all(|line| line.chars().count() <= MOBILE_MAX_TERMINAL_COLUMNS)
        );
        slides.push(TimelineSlide {
            eyebrow: format!(
                "{}  ·  {:02}/{:02}",
                document.title,
                slide_index + 1,
                slide_count
            ),
            heading: format!("{}  /  {}", section.title, section.range),
            summary: format!(
                "1 milestone — {}{}",
                entry.title,
                if draft.continuation > 0 {
                    " (continued)"
                } else {
                    ""
                }
            ),
            reveal_times: typing_schedule(&terminal.text),
            links: terminal.links,
            terminal: terminal.text,
            entry_ids: vec![entry.id.clone()],
            source_line_start: draft.source_line_start,
            line_count,
        });
    }

    Ok(slides)
}

#[derive(Clone, Debug)]
struct MobileSlideDraft {
    section_index: usize,
    entry_index: usize,
    continuation: usize,
    source_line_start: usize,
    lines: Vec<TerminalLine>,
}

fn mobile_header_line_count(section: &TimelineSection) -> usize {
    const HEADING_PREFIX_COLUMNS: usize = 13; // `| > 00/00 :: `
    4 + wrap_terminal_words(
        &terminal_ascii(&section.title),
        MOBILE_MAX_TERMINAL_COLUMNS - HEADING_PREFIX_COLUMNS,
    )
    .len()
}

fn build_mobile_entry_lines(entry: &TimelineEntry) -> Vec<TerminalLine> {
    let mut lines = vec![TerminalLine::plain(format!(
        "| $ history.show {}",
        terminal_ascii(&entry.year)
    ))];
    append_wrapped_body_with_columns(
        &mut lines,
        "| # ",
        "|   ",
        &terminal_ascii(&entry.id),
        MOBILE_MAX_TERMINAL_COLUMNS,
    );
    let result = format!(
        "{} :: {}",
        terminal_ascii(&entry.title),
        terminal_ascii(&entry.text)
    );
    append_wrapped_body_with_columns(
        &mut lines,
        "| > ",
        "|   ",
        &result,
        MOBILE_MAX_TERMINAL_COLUMNS,
    );
    append_mobile_link_lines(&mut lines, &entry.links);
    lines
}

fn build_mobile_terminal_stream(
    section: &TimelineSection,
    draft: &MobileSlideDraft,
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

fn paginate_section_entries(section: &TimelineSection) -> Vec<&[TimelineEntry]> {
    let mut pages = Vec::new();
    let mut start = 0;
    while start < section.entries.len() {
        let mut end = (start + MAX_ENTRIES_PER_SLIDE).min(section.entries.len());
        while end > start + 1
            && build_terminal_stream(section, &section.entries[start..end], 0, 1, 0, 99)
                .text
                .lines()
                .count()
                > MAX_TERMINAL_LINES
        {
            end -= 1;
        }
        pages.push(&section.entries[start..end]);
        start = end;
    }
    pages
}

fn build_terminal_stream(
    section: &TimelineSection,
    entries: &[TimelineEntry],
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

    for entry in entries {
        lines.push(TerminalLine::plain(format!(
            "| $ history.show {}",
            terminal_ascii(&entry.year)
        )));
        lines.push(TerminalLine::plain(format!(
            "| # {}",
            terminal_ascii(&entry.id)
        )));
        let result = format!(
            "{} :: {}",
            terminal_ascii(&entry.title),
            terminal_ascii(&entry.text)
        );
        append_wrapped_body(&mut lines, "| > ", &result);
        append_link_lines(&mut lines, &entry.links);
    }

    lines.extend([
        TerminalLine::plain("|"),
        TerminalLine::plain("| $ input.listen Click/Touch/⏎ to continue"),
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
    let line_count = lines.len();

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
    debug_assert_eq!(text.lines().count(), line_count);
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
            assert!(slide.terminal.ends_with("| > READY"));
            assert!(!slide.summary.is_empty());
            assert!(
                slide
                    .terminal
                    .chars()
                    .all(|character| character.is_ascii() || character == '⏎')
            );
            assert!(
                slide
                    .terminal
                    .contains("| $ input.listen Click/Touch/⏎ to continue")
            );
            assert_eq!(slide.line_count(), slide.terminal.lines().count());
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

        let mut slide_index = 0;
        for section in &document.sections {
            for entries in paginate_section_entries(section) {
                let slide = &slides[slide_index];
                let flattened = slide
                    .terminal
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                for entry in entries {
                    assert!(slide.contains_entry(&entry.id));
                    let full_body = terminal_ascii(&entry.text);
                    assert!(
                        flattened.contains(&full_body),
                        "slide {} omitted or changed the body for {}",
                        slide_index + 1,
                        entry.id
                    );
                    for link in &entry.links {
                        let full_token = format!("[{}]", terminal_ascii(&link.label));
                        assert!(
                            slide.terminal.contains(&full_token),
                            "slide {} omitted or truncated link token {full_token}",
                            slide_index + 1
                        );
                    }
                }
                slide_index += 1;
            }
        }
        assert_eq!(slide_index, slides.len());
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
                .map(|slide| slide.terminal.matches("| $ history.show ").count())
                .sum::<usize>(),
            entry_count
        );
        for slide in &slides {
            assert!(slide.line_count() <= MOBILE_MAX_TERMINAL_LINES);
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
                    format!(
                        "{} :: {}",
                        terminal_ascii(&entry.title),
                        terminal_ascii(&entry.text)
                    ),
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
