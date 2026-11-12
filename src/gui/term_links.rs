//! Links in a terminal, found where the pointer is: a link a program put on its text (OSC 8),
//! or a URL or a file path written out in the text.

/// A link on screen and where it goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub url: String,
    /// `url` is a path on the session's host, as programs print them, rather than a web link.
    pub file: bool,
    /// After a file's path, the `:line` or `:line:column` it was printed with, if any.
    pub place: String,
    /// Where it's drawn, on each row it wraps onto: the row, its first column and the column
    /// after its last.
    pub spans: Vec<(u16, u16, u16)>,
}

/// A character on screen: it, its row and its column.
pub type Cell = (char, u16, u16);

/// The link over (`row`, `col`), given the line there as it was printed: its characters
/// with their rows and columns, over every row it wraps onto (`Grid::line_cells`).
pub fn link_at(cells: &[Cell], (row, col): (u16, u16)) -> Option<Link> {
    let here = cells.iter().rposition(|&(_, r, c)| (r, c) <= (row, col))?;
    let text: Vec<char> = cells.iter().map(|(c, ..)| *c).collect();
    let (start, stop, url, file, place) = match url_at(&text, here) {
        Some((start, stop)) => (start, stop, text[start..stop].iter().collect(), false, String::new()),
        None => {
            let (start, end, stop) = path_at(&text, here)?;
            (start, stop, text[start..end].iter().collect(), true, text[end..stop].iter().collect())
        }
    };
    Some(Link { url, file, place, spans: spans(cells, start, stop) })
}

/// The link `uri` a program put on cell `here` of the line `cells` (OSC 8), over the cells
/// around it `same` says link there too. Any program's output sets these, with text that
/// needn't match: only web links open, as for a URL written out.
pub fn hyperlinked(uri: String, cells: &[Cell], here: usize, same: impl Fn(u16, u16) -> bool) -> Option<Link> {
    if !is_web(&uri) {
        return None;
    }
    let linked = |i: &usize| cells.get(*i).is_some_and(|&(_, r, c)| same(r, c));
    let start = (0..here).rev().take_while(linked).last().unwrap_or(here);
    let stop = (here..cells.len()).take_while(linked).last()? + 1;
    Some(Link { url: uri, file: false, place: String::new(), spans: spans(cells, start, stop) })
}

/// Where cells `start..stop` of a line are drawn, row by row.
fn spans(cells: &[Cell], start: usize, stop: usize) -> Vec<(u16, u16, u16)> {
    let mut spans: Vec<(u16, u16, u16)> = vec![];
    for (i, &(_, row, col)) in cells.iter().enumerate().take(stop).skip(start) {
        // To the next cell on its row: a wide character takes two columns.
        let end = cells.get(i + 1).filter(|(_, r, _)| *r == row).map_or(col + 1, |&(_, _, c)| c);
        match spans.last_mut() {
            Some((r, _, e)) if *r == row => *e = end,
            _ => spans.push((row, col, end)),
        }
    }
    spans
}

const SCHEMES: [&str; 2] = ["https://", "http://"];

/// Whether `uri` is a web link, whatever the case of its scheme.
fn is_web(uri: &str) -> bool {
    SCHEMES.iter().any(|scheme| {
        uri.get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    })
}

/// The URL written out in `text` over the character at `at`: its first
/// character and the one after its last.
pub fn url_at(text: &[char], at: usize) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some((start, scheme)) = next_scheme(text, from) {
        let mut end = start + scheme;
        while end < text.len() && !ends_url(text[end]) {
            end += 1;
        }
        let end = start + trimmed(&text[start..end]);
        if end > start + scheme && (start..end).contains(&at) {
            return Some((start, end));
        }
        from = end.max(start + 1);
    }
    None
}

/// Where the next `http://` or `https://` from `from` starts, and its length.
fn next_scheme(text: &[char], from: usize) -> Option<(usize, usize)> {
    (from..text.len()).find_map(|i| {
        SCHEMES.iter().find_map(|scheme| {
            let len = scheme.chars().count();
            let matches =
                text.len() >= i + len && text[i..i + len].iter().copied().eq(scheme.chars());
            matches.then_some((i, len))
        })
    })
}

/// Whether `c` can't be in a URL written out: spaces, quotes, and the
/// lines programs draw boxes with.
fn ends_url(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || "<>\"'`{}|\\^".contains(c)
        || ('\u{2500}'..='\u{257f}').contains(&c)
}

/// The file path written out in `text` over the character at `at`, as
/// programs print them: `src/main.rs`, `/etc/hosts`, `~/notes.md` or
/// `Cargo.toml`, maybe with `:line` or `:line:column` after it. Its first
/// character, the one after the path, and the one after its line and
/// column.
pub fn path_at(text: &[char], at: usize) -> Option<(usize, usize, usize)> {
    if ends_path(*text.get(at)?) {
        return None;
    }
    let start = (0..at)
        .rev()
        .take_while(|&i| !ends_path(text[i]))
        .last()
        .unwrap_or(at);
    let mut stop = (at..text.len())
        .take_while(|&i| !ends_path(text[i]))
        .last()?
        + 1;
    while stop > start && ".,:;!?".contains(text[stop - 1]) {
        stop -= 1;
    }
    // `:line`, then `:column`, from the end.
    let mut end = stop;
    for _ in 0..2 {
        let digits = text[start..end]
            .iter()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if digits == 0 || end - digits <= start + 1 || text[end - digits - 1] != ':' {
            break;
        }
        end -= digits + 1;
    }
    let path: String = text[start..end].iter().collect();
    (at < stop && looks_like_path(&path)).then_some((start, end, stop))
}

/// Whether `c` can't be in a path written out: what `ends_url` says, and
/// the brackets and separators prose puts around one, as in `Read(a.rs)`.
fn ends_path(c: char) -> bool {
    ends_url(c) || "()[],;".contains(c)
}

/// Whether `word` reads as a file's path: folders and a name (`src/a.rs`,
/// `/etc/hosts`), or a name with an extension that has a letter in it
/// (`Cargo.toml`, not `1.5.0`).
fn looks_like_path(word: &str) -> bool {
    if word.contains("://") {
        return false;
    }
    let name = word.rsplit('/').next().unwrap_or(word);
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    if word.contains('/') {
        return true;
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && ext.len() <= 10
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// How much of `url` is left without what trails it in prose: a full stop
/// or comma, or a closing bracket that opens before it, as in Markdown's
/// `[text](url)`.
fn trimmed(url: &[char]) -> usize {
    let mut end = url.len();
    while let Some(&last) = url[..end].last() {
        let count = |c: char| url[..end].iter().filter(|&&x| x == c).count();
        let unopened = |open: char, close: char| last == close && count(open) < count(close);
        if ".,;:!?".contains(last) || unopened('(', ')') || unopened('[', ']') {
            end -= 1;
        } else {
            break;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str, at: &str) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        let at = text[..text.find(at).expect(at)].chars().count();
        url_at(&chars, at).map(|(start, end)| chars[start..end].iter().collect())
    }

    #[test]
    fn urls_end_where_prose_does() {
        let text = "See https://example.com/a?b=c. Or (http://x.io/y), or [docs](https://d.dev/p).";
        assert_eq!(
            url(text, "example").as_deref(),
            Some("https://example.com/a?b=c")
        );
        assert_eq!(url(text, "x.io").as_deref(), Some("http://x.io/y"));
        assert_eq!(url(text, "d.dev").as_deref(), Some("https://d.dev/p"));
        assert_eq!(url(text, "See"), None);
        assert_eq!(url(text, "docs"), None);
        // Brackets a URL opens are its own.
        let wiki = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
        assert_eq!(url(wiki, "wiki").as_deref(), Some(wiki));
        assert_eq!(
            url("│ https://a.b/c│", "a.b").as_deref(),
            Some("https://a.b/c")
        );
        assert_eq!(url("https:// nothing", "https"), None);
    }

    /// The path in `text` at the first character of `at`, and how much of
    /// the text it underlines.
    fn path(text: &str, at: &str) -> Option<(String, String)> {
        let chars: Vec<char> = text.chars().collect();
        let at = text[..text.find(at).expect(at)].chars().count();
        path_at(&chars, at).map(|(start, end, stop)| {
            (
                chars[start..end].iter().collect(),
                chars[start..stop].iter().collect(),
            )
        })
    }

    #[test]
    fn paths_end_where_prose_does() {
        let both = |p: &str| Some((p.to_string(), p.to_string()));
        assert_eq!(
            path("Read(src/gui/links.rs)", "links"),
            both("src/gui/links.rs")
        );
        assert_eq!(
            path("see src/main.rs:42:7, then", "main"),
            Some(("src/main.rs".into(), "src/main.rs:42:7".into()))
        );
        assert_eq!(path("Edit Cargo.toml.", "Cargo"), both("Cargo.toml"));
        assert_eq!(
            path("in `~/notes/todo.md`", "todo"),
            both("~/notes/todo.md")
        );
        assert_eq!(path("at /etc/hosts:", "hosts"), both("/etc/hosts"));
        // Words, versions and folders aren't files.
        assert_eq!(path("a word here", "word"), None);
        assert_eq!(path("version 1.5.0 is out", "1.5"), None);
        assert_eq!(path("look in src/ first", "src"), None);
        // Nor is the full stop after one.
        assert_eq!(
            path("Edit Cargo.toml.", "."),
            Some(("Cargo.toml".into(), "Cargo.toml".into()))
        );
        assert_eq!(path("Edit a.rs. Then", ". Then"), None);
    }

    /// What a shell printing `script` shows in a 30-column terminal; it
    /// must show `shown`.
    #[test]
    fn a_link_knows_its_columns() {
        let row = |text: &str, y: u16| -> Vec<Cell> { text.chars().zip(0..).map(|(c, x)| (c, y, x)).collect() };
        let cells = row("see https://example.com/a now", 3);
        let link = link_at(&cells, (3, 10)).unwrap();
        assert_eq!((link.url.as_str(), link.file, link.spans.as_slice()), ("https://example.com/a", false, &[(3, 4, 25)][..]));
        let cells = row("Read(src/main.rs:12)", 0);
        let link = link_at(&cells, (0, 8)).unwrap();
        assert_eq!((link.url.as_str(), link.file, link.place.as_str()), ("src/main.rs", true, ":12"));
        assert!(link_at(&cells, (0, 1)).is_none());
    }

    /// A path the terminal wrapped onto the next row is one link, from either row.
    #[test]
    fn a_link_wraps_onto_the_next_row() {
        let mut cells: Vec<Cell> = "see /srv/app/src/le".chars().zip(0..).map(|(c, x)| (c, 4, x)).collect();
        cells.extend("mur/main.rs:7 now".chars().zip(0..).map(|(c, x)| (c, 5, x)));
        for at in [(4, 6), (5, 2)] {
            let link = link_at(&cells, at).unwrap();
            assert_eq!(link.url, "/srv/app/src/lemur/main.rs");
            assert_eq!(link.place, ":7");
            assert_eq!(link.spans, [(4, 4, 19), (5, 0, 13)]);
        }
    }
}
