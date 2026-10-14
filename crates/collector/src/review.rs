// Copyright Alexandre D. Díaz
//! `pr_review`: a unified diff (usually an AI's) as review suggestions on a
//! PR's head, the kind its author applies in one click. The forges only take
//! line comments inside the PR's own diff, and an AI's hunk headers are often
//! off: each hunk is found by its text in the file at the head, and what
//! can't be placed that way goes in the review's text, as a diff.
use std::collections::{BTreeMap, HashMap};

use crate::gitclient::git_run;

/// Lines `start..=end` (1-based, at the PR's head) of `path` become `lines`.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub lines: Vec<String>,
    /// `end`'s number before the PR when the PR leaves that line alone
    /// (GitLab anchors those by both).
    pub old_end: Option<usize>,
    /// The hunk as given, for the text fallback.
    pub text: String,
}

impl Suggestion {
    /// The comment: a `suggestion` block (GitLab: `suggestion:-N+0`, the
    /// lines above the one it's anchored on, `end`).
    pub fn body(&self, gitlab: bool) -> String {
        let content = self.lines.join("\n");
        let fence = fence(&content);
        let tag = match gitlab {
            true => format!("suggestion:-{}+0", self.end - self.start),
            false => "suggestion".to_string(),
        };
        match content.is_empty() {
            true => format!("{fence}{tag}\n{fence}"),
            false => format!("{fence}{tag}\n{content}\n{fence}"),
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub suggestions: Vec<Suggestion>,
    /// Hunks that can't be suggested (outside the PR's changes, not found at
    /// its head, new or deleted files), as diff text.
    pub leftovers: Vec<String>,
}

/// GitHub refuses bodies past 65,536 characters.
const MAX_TEXT: usize = 60_000;

impl Plan {
    /// The review's text: `body`, then `extra` hunks (what couldn't be
    /// suggested, or whatever the forge didn't take) as one diff.
    pub fn text(&self, body: &str, extra: &[String]) -> String {
        let hunks: String = self.leftovers.iter().chain(extra).cloned().collect();
        let mut text = body.trim().to_string();
        if !hunks.is_empty() {
            let fence = fence(&hunks);
            text.push_str(&format!(
                "\n\nChanges outside this PR's diff (to apply by hand):\n\n{fence}diff\n{hunks}{fence}"
            ));
        }
        match text.char_indices().nth(MAX_TEXT) {
            Some((at, _)) => format!("{}\n\n[... truncated]", &text[..at]),
            None => text.trim().to_string(),
        }
    }

    /// What it would do, for dry-run logs.
    pub fn describe(&self, body: &str) -> String {
        let mut files: Vec<&str> = self.suggestions.iter().map(|s| s.path.as_str()).collect();
        files.dedup();
        let mut out = format!("{} suggestion(s)", self.suggestions.len());
        if !files.is_empty() {
            out.push_str(&format!(" on {}", files.join(", ")));
        }
        if !self.leftovers.is_empty() {
            out.push_str(&format!(", {} change(s) as text", self.leftovers.len()));
        }
        format!("{out}: {}", body.trim())
    }
}

/// A code fence longer than any run of backticks in `content`.
fn fence(content: &str) -> String {
    let longest = content.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

/// One hunk of a unified diff.
#[derive(Debug, Default)]
struct Hunk {
    /// The file (the old path when it's deleted).
    path: String,
    /// A new or deleted file: no lines to suggest on.
    whole_file: bool,
    old_start: usize,
    new_start: usize,
    /// ' ' (both sides), '-' (old) or '+' (new), and the line.
    lines: Vec<(char, String)>,
    /// As given, with its file header, for the text fallback.
    text: String,
}

impl Hunk {
    fn side(&self, skip: char) -> Vec<&str> {
        self.lines
            .iter()
            .filter(|(kind, _)| *kind != skip)
            .map(|(_, line)| line.as_str())
            .collect()
    }
}

fn file_path(raw: &str) -> String {
    let raw = raw.split('\t').next().unwrap_or_default().trim();
    match raw {
        "/dev/null" => raw.to_string(),
        _ => raw
            .strip_prefix("a/")
            .or_else(|| raw.strip_prefix("b/"))
            .unwrap_or(raw)
            .to_string(),
    }
}

/// `@@ -a,b +c,d @@` → (a, c); 0 for numbers it lacks (an AI's `@@ @@`).
fn hunk_header(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@")?;
    let number = |sign: char| {
        rest.split_whitespace()
            .find_map(|w| w.strip_prefix(sign))
            .and_then(|w| w.split(',').next()?.parse().ok())
            .unwrap_or(0)
    };
    Some((number('-'), number('+')))
}

/// The hunks of a unified diff, as forgiving as AI output needs: `a/`/`b/`
/// prefixes or not, empty context lines that lost their space, `\ No
/// newline`, prose between hunks (it ends a hunk).
fn parse(diff: &str) -> Vec<Hunk> {
    let lines: Vec<&str> = diff.lines().collect();
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut old_path, mut new_path, mut header) = (String::new(), String::new(), String::new());
    let mut in_hunk = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let (Some(old), Some(new)) = (
            line.strip_prefix("--- "),
            lines.get(i + 1).and_then(|n| n.strip_prefix("+++ ")),
        ) {
            (old_path, new_path) = (file_path(old), file_path(new));
            header = format!("{line}\n{}\n", lines[i + 1]);
            in_hunk = false;
            i += 2;
            continue;
        }
        if let Some((old_start, new_start)) = hunk_header(line) {
            let deleted = new_path == "/dev/null";
            hunks.push(Hunk {
                path: if deleted { &old_path } else { &new_path }.clone(),
                whole_file: deleted || old_path == "/dev/null",
                old_start,
                new_start,
                lines: Vec::new(),
                text: format!("{header}{line}\n"),
            });
            in_hunk = true;
        } else if let (true, Some(hunk)) = (in_hunk, hunks.last_mut()) {
            match line.chars().next() {
                Some(kind @ (' ' | '-' | '+')) => hunk.lines.push((kind, line[1..].to_string())),
                None => hunk.lines.push((' ', String::new())),
                Some('\\') => {}
                _ => in_hunk = false,
            }
            if in_hunk {
                hunk.text.push_str(line);
                hunk.text.push('\n');
            }
        }
        i += 1;
    }
    // A blank line after a hunk is the end of the text, not context.
    for hunk in &mut hunks {
        while hunk.lines.last() == Some(&(' ', String::new())) {
            hunk.lines.pop();
        }
    }
    hunks
}

/// The PR's diff, per file: each hunk's lines on the head side, with their
/// number before the PR when the PR leaves them alone.
type Shown = HashMap<String, Vec<BTreeMap<usize, Option<usize>>>>;

fn shown(pr_diff: &str) -> Shown {
    let mut shown: Shown = HashMap::new();
    for hunk in parse(pr_diff) {
        let (mut old, mut new) = (hunk.old_start, hunk.new_start);
        let mut lines = BTreeMap::new();
        for (kind, _) in &hunk.lines {
            match kind {
                ' ' => {
                    lines.insert(new, Some(old));
                    (old, new) = (old + 1, new + 1);
                }
                '+' => {
                    lines.insert(new, None);
                    new += 1;
                }
                _ => old += 1,
            }
        }
        shown.entry(hunk.path).or_default().push(lines);
    }
    shown
}

/// Where `needle` is in `file` (0-based), the place nearest `near` when it's
/// in several; ignoring trailing whitespace (CRLF files, AI trimming) only
/// when it isn't anywhere as is.
fn find(file: &[&str], needle: &[&str], near: usize) -> Option<usize> {
    let nearest = |same: &dyn Fn(&str, &str) -> bool| {
        (0..=file.len().checked_sub(needle.len())?)
            .filter(|&at| needle.iter().zip(&file[at..]).all(|(a, b)| same(a, b)))
            .min_by_key(|&at| at.abs_diff(near))
    };
    nearest(&|a, b| a == b).or_else(|| nearest(&|a, b| a.trim_end() == b.trim_end()))
}

fn suggest(
    hunk: &Hunk,
    shown: &Shown,
    read: &dyn Fn(&str) -> Option<String>,
) -> Option<Suggestion> {
    if hunk.whole_file {
        return None;
    }
    let (old, new) = (hunk.side('+'), hunk.side('-'));
    if old.is_empty() {
        return None;
    }
    let file = read(&hunk.path)?;
    let file: Vec<&str> = file.lines().collect();
    let at = find(&file, &old, hunk.old_start.saturating_sub(1))?;
    // Only what changes: the context both sides share stays out.
    let lead = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let trail = old[lead..]
        .iter()
        .rev()
        .zip(new[lead..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (mut from, mut to) = (lead, old.len() - trail);
    let (mut new_from, mut new_to) = (lead, new.len() - trail);
    if from == to {
        // Only added lines: a suggestion replaces lines, so one comes along.
        if from > 0 {
            (from, new_from) = (from - 1, new_from - 1);
        } else if to < old.len() {
            (to, new_to) = (to + 1, new_to + 1);
        } else {
            return None;
        }
    }
    let (start, end) = (at + from + 1, at + to);
    let hunk_lines = shown
        .get(&hunk.path)?
        .iter()
        .find(|lines| lines.contains_key(&start) && lines.contains_key(&end))?;
    Some(Suggestion {
        path: hunk.path.clone(),
        start,
        end,
        lines: new[new_from..new_to]
            .iter()
            .map(|l| l.to_string())
            .collect(),
        old_end: hunk_lines[&end],
        text: hunk.text.clone(),
    })
}

/// `patch`'s hunks as suggestions on the PR's head (`read`: a file there),
/// where `pr_diff` (the PR's own) lets the forge take them; the rest as text.
/// Hunks that change nothing are dropped.
pub fn plan(patch: &str, pr_diff: &str, read: &dyn Fn(&str) -> Option<String>) -> Plan {
    let shown = shown(pr_diff);
    let mut plan = Plan::default();
    for hunk in parse(patch) {
        if hunk.lines.iter().all(|(kind, _)| *kind == ' ') {
            continue;
        }
        match suggest(&hunk, &shown, read) {
            Some(suggestion) => plan.suggestions.push(suggestion),
            None => plan.leftovers.push(hunk.text),
        }
    }
    plan
}

/// `plan` on a PR of the clone: its head `head`, onto `origin/<base>`.
pub fn plan_on(clone: &str, base: &str, head: &str, patch: &str) -> Result<Plan, String> {
    git_run(clone, &["cat-file", "-e", &format!("{head}^{{commit}}")])
        .map_err(|_| format!("the PR's head {head} isn't in the clone"))?;
    let range = format!("origin/{base}...{head}");
    let pr_diff = git_run(clone, &["diff", "--no-renames", "-U3", &range])?;
    let read = |path: &str| git_run(clone, &["show", &format!("{head}:{path}")]).ok();
    Ok(plan(patch, &pr_diff, &read))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The head of a PR that added `b2` and changed `d` → `D` in `a..f`.
    const FILE: &str = "a\nb\nb2\nc\nD\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\np\nq\nr\n";
    const PR: &str = "\
diff --git a/x.py b/x.py
--- a/x.py
+++ b/x.py
@@ -1,8 +1,9 @@
 a
 b
+b2
 c
-d
+D
 e
 f
 g
";

    fn read(path: &str) -> Option<String> {
        (path == "x.py").then(|| FILE.to_string())
    }

    #[test]
    fn hunks_are_found_by_their_text_not_their_numbers() {
        // Wrong numbers, no a/ b/, an empty context line without its space,
        // prose around: one suggestion, lines 4-5 → "C", "DD".
        let patch =
            "Sure:\n--- x.py\n+++ x.py\n@@ -40,4 +40,4 @@\n b2\n-c\n-D\n+C\n+DD\n\nThat's it.\n";
        let found = plan(patch, PR, &read);
        assert!(found.leftovers.is_empty(), "{found:?}");
        let s = &found.suggestions[0];
        assert_eq!(
            (s.start, s.end, s.lines.clone()),
            (4, 5, vec!["C".into(), "DD".into()])
        );
        // Line 5 is the PR's own: no number before it.
        assert_eq!(s.old_end, None);
        assert_eq!(s.body(false), "```suggestion\nC\nDD\n```");
        assert_eq!(s.body(true), "```suggestion:-1+0\nC\nDD\n```");
        // Shared context stays out: only "c" changes here.
        let one = "--- a/x.py\n+++ b/x.py\n@@ -3,3 +3,3 @@\n b2\n-c\n+C\n D\n";
        let s = &plan(one, PR, &read).suggestions[0];
        assert_eq!((s.start, s.end, s.old_end), (4, 4, Some(3)));
    }

    #[test]
    fn insertions_take_a_neighbour_and_deletions_suggest_nothing() {
        let insert = "--- a/x.py\n+++ b/x.py\n@@ -5,2 +5,3 @@\n D\n+D2\n e\n";
        let s = &plan(insert, PR, &read).suggestions[0];
        assert_eq!(
            (s.start, s.end, s.lines.clone()),
            (5, 5, vec!["D".into(), "D2".into()])
        );
        let delete = "--- a/x.py\n+++ b/x.py\n@@ -2,3 +2,2 @@\n b\n-b2\n c\n";
        let s = &plan(delete, PR, &read).suggestions[0];
        assert_eq!((s.start, s.end, s.lines.len()), (3, 3, 0));
        assert_eq!(s.body(false), "```suggestion\n```");
        // A context line keeps its number before the PR (GitLab needs it).
        let context = "--- a/x.py\n+++ b/x.py\n@@ -6 +6 @@\n-e\n+E\n";
        assert_eq!(plan(context, PR, &read).suggestions[0].old_end, Some(5));
    }

    #[test]
    fn what_the_forge_wont_take_stays_as_text() {
        let patch = "\
--- a/x.py
+++ b/x.py
@@ -15 +15 @@
-n
+N
--- a/x.py
+++ b/x.py
@@ -8,4 +8,4 @@
 g
-h
+H
 i
--- /dev/null
+++ b/new.py
@@ -0,0 +1 @@
+x = 1
--- a/x.py
+++ b/x.py
@@ -1 +1 @@
-zzz
+yyy
--- a/x.py
+++ b/x.py
@@ -1 +1 @@
 a
";
        let plan = plan(patch, PR, &read);
        // Outside the PR's diff (n, and h on line 9), a new file, not in the
        // file; the context-only hunk is dropped.
        assert!(plan.suggestions.is_empty(), "{plan:?}");
        assert_eq!(plan.leftovers.len(), 4, "{plan:?}");
        let text = plan.text("Please look.", &[]);
        assert!(
            text.starts_with("Please look.\n\nChanges outside"),
            "{text}"
        );
        assert!(
            text.contains("```diff\n--- a/x.py\n+++ b/x.py\n@@ -15 +15 @@\n-n\n+N\n"),
            "{text}"
        );
    }

    #[test]
    fn a_suggestion_spanning_two_pr_hunks_stays_as_text() {
        let pr = format!("{PR}@@ -15,3 +16,3 @@\n o\n-p\n+P\n q\n");
        let patch = "--- a/x.py\n+++ b/x.py\n@@ -8,10 +8,10 @@\n-g\n-h\n-i\n-j\n-k\n-l\n-m\n-n\n-o\n+gone\n";
        let plan = plan(patch, &pr, &read);
        assert_eq!((plan.suggestions.len(), plan.leftovers.len()), (0, 1));
    }

    #[test]
    fn fences_outgrow_the_content() {
        let s = Suggestion {
            path: "x".into(),
            start: 1,
            end: 1,
            lines: vec!["doc = '''```py'''".into()],
            old_end: None,
            text: String::new(),
        };
        assert_eq!(s.body(false), "````suggestion\ndoc = '''```py'''\n````");
    }
}
