use serde::Deserialize;

const TIMELINE_JSON: &str = include_str!("../data/timeline.json");
// The WebGPU terminal font is 1.5x its original size. Reflow and paginate at
// the reciprocal density so glyphs remain at that true scale on narrow screens
// instead of being shrunk back down by the responsive transform.
const MAX_ENTRIES_PER_SLIDE: usize = 2;
const MAX_TERMINAL_LINES: usize = 22;
const INITIAL_TYPE_DELAY: f32 = 0.28;
const CHARACTER_SECONDS: f32 = 0.0115;
const SPACE_SECONDS: f32 = 0.006;
const PUNCTUATION_SECONDS: f32 = 0.021;
const LINE_PAUSE_SECONDS: f32 = 0.100;
const MAX_TERMINAL_COLUMNS: usize = 42;

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
                line_count,
            });
        }
    }

    if slides.is_empty() {
        return Err("data/timeline.json has no entries".to_owned());
    }

    Ok(slides)
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

struct LineLinkRange {
    url: String,
    start_column: usize,
    end_column: usize,
}

fn append_wrapped_body(lines: &mut Vec<TerminalLine>, prefix: &str, body: &str) {
    let prefix_columns = prefix.chars().count();
    let continuation = " ".repeat(prefix_columns.min(8));
    let mut line = String::from(prefix);
    let mut content_start = prefix_columns;

    for word in body.split_whitespace() {
        let separator = usize::from(line.chars().count() > content_start);
        if line.chars().count() + separator + word.chars().count() > MAX_TERMINAL_COLUMNS
            && line.chars().count() > content_start
        {
            lines.push(TerminalLine::plain(line));
            line = continuation.clone();
            content_start = continuation.chars().count();
        }
        if line.chars().count() > content_start {
            line.push(' ');
        }
        line.push_str(word);
    }

    lines.push(TerminalLine::plain(line));
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
}
