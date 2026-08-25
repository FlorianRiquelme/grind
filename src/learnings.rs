//! `~/.grind/learnings/lessons.tsv` — the append-only receipt log — and the two other formats
//! the design's storage section names alongside it: `notes.md`'s 200-line hard cap, and the
//! ledger's `---`-fenced frontmatter. Pure, no `world`, no `std::fs` (`tests/topology.rs`):
//! every effect here is a `&str` in, a value out.

/// One row of `lessons.tsv`. `route` and `status` stay plain `String`s rather than enums — the
/// schema is a receipt log a human or a future lens can extend with a new word, and this module
/// only ever filters on the two words [`applicable_lessons`] excludes (`superseded`, `rejected`),
/// so an enum would buy nothing beyond a match arm for whichever a future word forgets to name.
#[derive(Debug, Clone, PartialEq)]
pub struct Lesson {
    pub ts: String,
    pub source_run: String,
    pub lens: String,
    pub lesson_id: String,
    pub statement: String,
    pub route: String,
    pub status: String,
}

const FIELD_COUNT: usize = 7;

/// Tolerant TSV parse: a row with the wrong field count is skipped rather than refusing the
/// whole file — one malformed receipt must not blind every lesson before and after it in an
/// append-only log nothing else can rebuild.
pub fn parse_lessons(tsv: &str) -> Vec<Lesson> {
    tsv.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(parse_lesson_line)
        .collect()
}

fn parse_lesson_line(line: &str) -> Option<Lesson> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != FIELD_COUNT {
        return None;
    }
    Some(Lesson {
        ts: fields[0].to_string(),
        source_run: fields[1].to_string(),
        lens: fields[2].to_string(),
        lesson_id: fields[3].to_string(),
        statement: fields[4].to_string(),
        route: fields[5].to_string(),
        status: fields[6].to_string(),
    })
}

/// One TSV line for a [`Lesson`]. Tab-safe: a tab or newline embedded in a field would shift
/// every column after it (and a bare newline would forge a second, malformed row), so both are
/// replaced with a single space before joining on real tabs.
pub fn compose_lesson_row(lesson: &Lesson) -> String {
    [
        &lesson.ts,
        &lesson.source_run,
        &lesson.lens,
        &lesson.lesson_id,
        &lesson.statement,
        &lesson.route,
        &lesson.status,
    ]
    .iter()
    .map(|field| tab_safe(field))
    .collect::<Vec<_>>()
    .join("\t")
}

fn tab_safe(field: &str) -> String {
    field.replace(['\t', '\n'], " ")
}

/// Stop-words excluded from the path-keyword match: common English words and path segments so
/// generic they would match nearly every forecast path (`src`, `the`, `a`, ...).
const STOP_WORDS: [&str; 6] = ["the", "a", "an", "to", "src", "of"];

/// Lowercase, alphanumeric-only words, stop-words dropped.
fn keywords(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| !w.is_empty() && !STOP_WORDS.contains(&w.as_str()))
        .collect()
}

/// The composition-time path-keyword match: a lesson applies when any lowercase word of its
/// statement overlaps any lowercase path-segment word of a forecast path, stop-words on both
/// sides excluded. Kept simple and documented rather than fuzzy — this is the pre-Plan match
/// the design names, not a search engine. Superseded and rejected lessons never apply,
/// regardless of overlap.
pub fn applicable_lessons<'a>(lessons: &'a [Lesson], forecast_paths: &[String]) -> Vec<&'a Lesson> {
    let path_words: Vec<String> = forecast_paths.iter().flat_map(|p| keywords(p)).collect();
    lessons
        .iter()
        .filter(|lesson| lesson.status != "superseded" && lesson.route != "rejected")
        .filter(|lesson| {
            keywords(&lesson.statement)
                .iter()
                .any(|word| path_words.contains(word))
        })
        .collect()
}

/// The marker convention that names a promoted line: a line containing `[L-` carries a ledger
/// id (e.g. `[L-042]`) and survives eviction; every other line is an unpromoted note.
const PROMOTED_MARKER: &str = "[L-";

/// `notes.md`'s 200-line hard cap. Over the cap, oldest **unpromoted** lines are evicted from
/// the top first — a promoted line (one carrying [`PROMOTED_MARKER`]) survives eviction even if
/// it is old, since it has already earned its place in the target repo's own ledger. Line order
/// is otherwise preserved.
pub fn bounded_notes(notes: &str) -> String {
    const CAP: usize = 200;
    let lines: Vec<&str> = notes.lines().collect();
    if lines.len() <= CAP {
        return notes.to_string();
    }
    let mut to_drop = lines.len() - CAP;
    let mut kept: Vec<&str> = Vec::with_capacity(CAP);
    for line in lines {
        if to_drop > 0 && !line.contains(PROMOTED_MARKER) {
            to_drop -= 1;
            continue;
        }
        kept.push(line);
    }
    kept.join("\n")
}

/// A `---`-fenced frontmatter block, parsed as tolerant `key: value` pairs. Degrade-don't-abort:
/// no opening fence returns an empty list rather than an error, and a line inside the fence with
/// no `:` is skipped rather than refusing the block.
pub fn ledger_frontmatter(text: &str) -> Vec<(String, String)> {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return Vec::new();
    };
    if first.trim() != "---" {
        return Vec::new();
    }
    let mut pairs = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            pairs.push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lesson(statement: &str, route: &str, status: &str) -> Lesson {
        Lesson {
            ts: "2026-08-22T00:00:00Z".to_string(),
            source_run: "run-1".to_string(),
            lens: "tooling".to_string(),
            lesson_id: "L-001".to_string(),
            statement: statement.to_string(),
            route: route.to_string(),
            status: status.to_string(),
        }
    }

    #[test]
    fn a_lesson_round_trips_through_compose_and_parse() {
        let original = lesson("verify entrypoint check missing", "observe", "applied");
        let row = compose_lesson_row(&original);
        let parsed = parse_lessons(&row);
        assert_eq!(parsed, vec![original]);
    }

    #[test]
    fn tabs_and_newlines_in_a_field_are_replaced_before_composing() {
        let dirty = lesson("has\ta tab\nand a newline", "notes", "proposed");
        let row = compose_lesson_row(&dirty);
        assert_eq!(row.split('\t').count(), FIELD_COUNT);
        let parsed = parse_lessons(&row);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].statement, "has a tab and a newline");
    }

    #[test]
    fn a_malformed_row_is_skipped_without_losing_its_neighbours() {
        let tsv = format!(
            "{good1}\ntoo\tfew\tfields\n{good2}\n",
            good1 = compose_lesson_row(&lesson("first lesson", "skill", "proposed")),
            good2 = compose_lesson_row(&lesson("second lesson", "template", "applied")),
        );
        let parsed = parse_lessons(&tsv);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].statement, "first lesson");
        assert_eq!(parsed[1].statement, "second lesson");
    }

    #[test]
    fn blank_lines_are_ignored() {
        let tsv = format!(
            "\n{row}\n\n",
            row = compose_lesson_row(&lesson("only lesson", "skill", "proposed"))
        );
        assert_eq!(parse_lessons(&tsv).len(), 1);
    }

    #[test]
    fn superseded_and_rejected_lessons_never_apply() {
        let lessons = vec![
            lesson("decide tier selection", "skill", "superseded"),
            lesson("decide tier selection", "rejected", "proposed"),
            lesson("decide tier selection", "skill", "applied"),
        ];
        let matches = applicable_lessons(&lessons, &["src/decide.rs".to_string()]);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].status, "applied");
    }

    #[test]
    fn a_statement_sharing_a_path_word_applies() {
        let lessons = vec![lesson(
            "attempt classification must key on work done, not cause",
            "skill",
            "applied",
        )];
        let matches = applicable_lessons(&lessons, &["src/attempt.rs".to_string()]);
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn a_statement_with_no_shared_word_does_not_apply() {
        let lessons = vec![lesson(
            "ship should append ledger candidates",
            "skill",
            "applied",
        )];
        let matches = applicable_lessons(&lessons, &["src/decide.rs".to_string()]);
        assert!(matches.is_empty());
    }

    #[test]
    fn over_the_cap_oldest_unpromoted_lines_are_evicted_first() {
        let mut lines: Vec<String> = (0..205).map(|n| format!("line {n}")).collect();
        lines[0] = "line 0 [L-001] promoted".to_string();
        lines[2] = "line 2 [L-002] promoted".to_string();
        let notes = lines.join("\n");
        let bounded = bounded_notes(&notes);
        let kept: Vec<&str> = bounded.lines().collect();
        assert_eq!(kept.len(), 200);
        assert!(kept.iter().any(|l| l.contains("[L-001]")));
        assert!(kept.iter().any(|l| l.contains("[L-002]")));
        assert!(kept.contains(&"line 204"));
        assert!(!kept.contains(&"line 1"));
    }

    #[test]
    fn at_or_under_the_cap_nothing_is_evicted() {
        let notes = (0..200)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(bounded_notes(&notes), notes);
    }

    #[test]
    fn a_fenced_frontmatter_block_parses_tolerantly() {
        let text = "---\ndate: 2026-08-22\nrun: run-42\npaths: src/attempt.rs\nstatus: candidate\n---\nbody text here";
        let pairs = ledger_frontmatter(text);
        assert_eq!(
            pairs,
            vec![
                ("date".to_string(), "2026-08-22".to_string()),
                ("run".to_string(), "run-42".to_string()),
                ("paths".to_string(), "src/attempt.rs".to_string()),
                ("status".to_string(), "candidate".to_string()),
            ]
        );
    }

    #[test]
    fn no_fence_degrades_to_an_empty_list() {
        assert!(ledger_frontmatter("just a plain markdown file\nwith no fence at all").is_empty());
    }

    #[test]
    fn garbage_inside_a_fence_skips_lines_with_no_colon() {
        let text = "---\nnot a pair at all\ndate: 2026-08-22\n---\n";
        let pairs = ledger_frontmatter(text);
        assert_eq!(pairs, vec![("date".to_string(), "2026-08-22".to_string())]);
    }
}
