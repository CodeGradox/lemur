//! What the notes' keys and buttons do to their text: each takes the text and the
//! selection, and gives the edit to make, if any.

use std::ops::Range;

/// An edit to the notes: the bytes it replaces, what with, and where the
/// selection goes then.
pub type Edit = (Range<usize>, String, Range<usize>);

/// Where a list item's words start in `line`: after its indent, its
/// marker (`-`, `*`, `+`, or a number and `.` or `)`) and a space.
pub fn list_item(line: &str) -> Option<usize> {
    let blank = |s: &str| s.len() - s.trim_start_matches([' ', '\t']).len();
    let indent = blank(line);
    let item = &line[indent..];
    let digits = item.len() - item.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let marker = match digits {
        0 if item.starts_with(['-', '*', '+']) => 1,
        1..=9 if item[digits..].starts_with(['.', ')']) => digits + 1,
        _ => return None,
    };
    let gap = blank(&item[marker..]);
    (gap > 0).then_some(indent + marker + gap)
}

/// Where the box of a task-list item starts in `line`: `[ ]`, or `[x]`
/// once ticked, where the item's words would.
pub fn checkbox(line: &str) -> Option<usize> {
    let at = list_item(line)?;
    let rest = &line.as_bytes()[at..];
    let tickable = rest.len() >= 3
        && rest[0] == b'['
        && matches!(rest[1], b' ' | b'x' | b'X')
        && rest[2] == b']'
        && rest.get(3).is_none_or(u8::is_ascii_whitespace);
    tickable.then_some(at)
}

/// The start of the item after one that starts with `start`, up to its
/// box: the same indent and marker, with the next number on a numbered
/// list.
pub fn next_marker(start: &str) -> String {
    let indent = start.len() - start.trim_start_matches([' ', '\t']).len();
    let item = &start[indent..];
    let digits = item.len() - item.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    match item[..digits].parse::<u64>() {
        Ok(n) => format!("{}{}{}", &start[..indent], n + 1, &item[digits..]),
        Err(_) => start.to_string(),
    }
}

/// The ticked items in `text`: each one's line, to fade, and the words
/// after its box, to strike through.
pub fn ticked(text: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let mut items = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        if let Some(at) = checkbox(line)
            && line.as_bytes()[at + 1] != b' '
        {
            let end = start + line.trim_end().len();
            let words = end - line[at + 3..].trim().len();
            items.push((start..end, words..end));
        }
        start += line.len() + 1;
    }
    items
}

/// The boxes in `text`: where each starts.
pub fn boxes(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        found.extend(checkbox(line).map(|at| start + at));
        start += line.len() + 1;
    }
    found
}

/// The line around byte `at` in `text`, without its newline.
fn line_at(text: &str, at: usize) -> Range<usize> {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    start..end
}

/// Ticks the box on the line at byte `at`, or unticks it: the byte that
/// changes and what to. None when the line has none.
pub fn tick(text: &str, at: usize) -> Option<(Range<usize>, &'static str)> {
    let line = line_at(text, at);
    let box_at = checkbox(&text[line.clone()])?;
    let mark = line.start + box_at + 1;
    let tick = if text.as_bytes()[mark] == b' ' { "x" } else { " " };
    Some((mark..mark + 1, tick))
}

/// What Enter does at the end of a list item: starts the item below, with a
/// box if this one has one; an empty one is cleared instead, which ends the
/// list. None when the cursor isn't at the end of an item.
pub fn next_item(text: &str, selected: Range<usize>) -> Option<Edit> {
    if !selected.is_empty() {
        return None;
    }
    let line = line_at(text, selected.start);
    if selected.start != line.end {
        return None;
    }
    let words = &text[line.clone()];
    let at = list_item(words)?;
    let (past, next) = match checkbox(words) {
        Some(_) => (at + 3, "[ ] "),
        None => (at, ""),
    };
    if words[past..].trim().is_empty() {
        return Some((line.clone(), String::new(), line.start..line.start));
    }
    let item = format!("\n{}{next}", next_marker(&words[..at]));
    let end = line.end + item.len();
    Some((line.end..line.end, item, end..end))
}

/// The lines `selected` touches in `text`, by their bytes: a selection that
/// ends where a line starts stops at the line before.
fn lines_of(text: &str, selected: &Range<usize>) -> Range<usize> {
    let last = match selected.end {
        end if end > selected.start && text[..end].ends_with('\n') => end - 1,
        end => end,
    };
    let start = text[..selected.start].rfind('\n').map_or(0, |i| i + 1);
    let end = text[last..].find('\n').map_or(text.len(), |i| last + i);
    start..end
}

/// What a list button makes of the lines `selected` touches in `text`:
/// plain bullets, or items with a box with `boxed`, or plain lines again
/// when they all are that already. Blank lines among several stay as they
/// are. Gives the lines' range, what replaces them, and where the
/// selection goes.
pub fn bulleted(text: &str, selected: Range<usize>, boxed: bool) -> Edit {
    let Range { start, end } = lines_of(text, &selected);
    let lines: Vec<&str> = text[start..end].split('\n').collect();
    let several = lines.len() > 1;
    let counts = |line: &str| !several || !line.trim().is_empty();
    let undo = lines.iter().filter(|line| counts(line)).all(|line| list_item(line).is_some() && checkbox(line).is_some() == boxed);
    // Each line's edit: the bytes it replaces in `text`, and with what.
    let mut edits = Vec::new();
    let mut from = start;
    for line in &lines {
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        let past_box = |at: usize| line.len() - line[at + 3..].trim_start_matches([' ', '\t']).len();
        let boxed_now = checkbox(line).is_some();
        let edit = match list_item(line) {
            _ if !counts(line) => None,
            Some(at) if undo => Some((indent, if boxed_now { past_box(at) } else { at }, "")),
            Some(at) if boxed_now && !boxed => Some((at, past_box(at), "")),
            Some(at) if !boxed_now && boxed => Some((at, at, "[ ] ")),
            Some(_) => None,
            None => Some((indent, indent, if boxed { "- [ ] " } else { "- " })),
        };
        edits.extend(edit.map(|(a, b, with)| (from + a, from + b, with)));
        from += line.len() + 1;
    }
    spliced(text, start..end, &edits, selected)
}

/// Tab on the lines `selected` touches in `text`: each indented two spaces,
/// or with `out` two spaces (or a tab) fewer. Blank lines among several stay
/// as they are. With nothing selected on a line that isn't a list item, Tab
/// puts two spaces at the cursor. None when nothing changes.
pub fn indented(text: &str, selected: Range<usize>, out: bool) -> Option<Edit> {
    let lines = lines_of(text, &selected);
    if !out && selected.is_empty() && list_item(&text[lines.clone()]).is_none() {
        let at = selected.start;
        return Some((at..at, "  ".into(), at + 2..at + 2));
    }
    let several = text[lines.clone()].contains('\n');
    let mut edits = Vec::new();
    let mut from = lines.start;
    for line in text[lines.clone()].split('\n') {
        let spaces = line.len() - line.trim_start_matches(' ').len();
        let edit = match () {
            _ if several && line.trim().is_empty() => None,
            _ if !out => Some((0, 0, "  ")),
            _ if line.starts_with('\t') => Some((0, 1, "")),
            _ if spaces > 0 => Some((0, spaces.min(2), "")),
            _ => None,
        };
        edits.extend(edit.map(|(a, b, with)| (from + a, from + b, with)));
        from += line.len() + 1;
    }
    (!edits.is_empty()).then(|| spliced(text, lines, &edits, selected))
}

/// The edit that makes `edits` (bytes of `text` and what replaces them, in
/// order) in the lines `block`, and moves `selected` along with the text
/// around it. A cursor sits past what's put in where it is.
fn spliced(text: &str, block: Range<usize>, edits: &[(usize, usize, &str)], selected: Range<usize>) -> Edit {
    let mut lines = String::new();
    let mut copied = block.start;
    for (a, b, with) in edits {
        lines.push_str(&text[copied..*a]);
        lines.push_str(with);
        copied = *b;
    }
    lines.push_str(&text[copied..block.end]);
    // Where an offset goes: past what's put in at it, with `after`.
    let moved = |at: usize, after: bool| {
        let mut shift = 0isize;
        for (a, b, with) in edits {
            if at < *a || at == *a && !after {
                break;
            }
            if at <= *b {
                return (*a as isize + shift) as usize + with.len();
            }
            shift += with.len() as isize - (b - a) as isize;
        }
        (at as isize + shift) as usize
    };
    let after = selected.is_empty();
    let selected = moved(selected.start, after)..moved(selected.end, true);
    (block, lines, selected)
}

/// Moves the lines `selected` touches in `text` up past the line above,
/// or down past the one below. A list item takes the items nested under
/// it along, and passes a neighbor with its nested items in one go. None
/// at the top or the bottom.
pub fn moved(text: &str, selected: Range<usize>, up: bool) -> Option<Edit> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    // A last newline ends the last line rather than starting another.
    if text.ends_with('\n') {
        lines.pop();
    }
    let row = |at: usize| text[..at].matches('\n').count();
    // A selection that ends where a line starts stops at the line before.
    let end = match selected.end {
        end if end > selected.start && text[..end].ends_with('\n') => end - 1,
        end => end,
    };
    let (first, mut last) = (row(selected.start), row(end));
    if last >= lines.len() {
        return None;
    }
    let indent = |line: &str| line.len() - line.trim_start_matches([' ', '\t']).len();
    let nested = |i: usize, under: usize| !lines[i].trim().is_empty() && indent(lines[i]) > under;
    let base = indent(lines[first]);
    while last + 1 < lines.len() && nested(last + 1, base) {
        last += 1;
    }
    let (from, to, order) = if up {
        let mut above = first.checked_sub(1)?;
        while above > 0 && nested(above, base) {
            above -= 1;
        }
        (above, last, [first..last + 1, above..first])
    } else {
        let below = last + 1;
        if below == lines.len() {
            return None;
        }
        let mut end = below;
        let under = indent(lines[below]);
        while !lines[below].trim().is_empty() && end + 1 < lines.len() && nested(end + 1, under) {
            end += 1;
        }
        (first, end, [below..end + 1, first..last + 1])
    };
    let start_of = |i: usize| lines[..i].iter().map(|line| line.len() + 1).sum::<usize>();
    let range = start_of(from)..start_of(to) + lines[to].len();
    let block = start_of(first)..start_of(last) + lines[last].len();
    let lines: Vec<&str> = order.into_iter().flat_map(|r| lines[r].to_vec()).collect();
    let moved_to = if up { range.start } else { range.end - block.len() };
    let at = |offset: usize| offset - block.start + moved_to;
    Some((range, lines.join("\n"), at(selected.start)..at(selected.end)))
}

/// Pasting `url` over `selected` in `text`, when that's text on one line
/// and not a URL itself: `[text](url)`, with the cursor after it.
pub fn pasted_link(text: &str, selected: Range<usize>, url: &str) -> Option<Edit> {
    let words = &text[selected.clone()];
    if words.is_empty() || words.contains('\n') || is_url(words) {
        return None;
    }
    let link = format!("[{words}]({url})");
    let end = selected.start + link.len();
    Some((selected, link, end..end))
}

/// What ⌘K does at `selected` in `text`, given the clipboard's URL if it
/// holds one. Inside a `[text](url)` link it selects the URL, to edit it.
/// Over text on one line it links that, and with nothing selected it
/// starts a link with the cursor in its text. The clipboard's URL comes
/// selected, so typing replaces it; without one, the cursor waits where
/// the URL goes.
pub fn link(text: &str, selected: Range<usize>, url: Option<&str>) -> Option<Edit> {
    let words = &text[selected.clone()];
    if words.contains('\n') {
        return None;
    }
    let start = text[..selected.start].rfind('\n').map_or(0, |i| i + 1);
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    if let Some(url) = link_url_at(&text[start..end], selected.start - start) {
        let url = start + url.start..start + url.end;
        return Some((selected, words.to_string(), url));
    }
    let url = url.unwrap_or("");
    let link = format!("[{words}]({url})");
    let after = match words.len() {
        0 => selected.start + 1..selected.start + 1,
        n if url.is_empty() => selected.start + n + 3..selected.start + n + 3,
        n => selected.start + n + 3..selected.start + n + 3 + url.len(),
    };
    Some((selected, link, after))
}

/// The URL of the `[text](url)` link in `line` around byte `at`.
fn link_url_at(line: &str, at: usize) -> Option<Range<usize>> {
    links(line).into_iter().find(|(whole, _)| (whole.start + 1..=whole.end - 1).contains(&at)).map(|(_, url)| url)
}

/// The `[text](url)` links in `line`: each whole, and its URL.
pub fn links(line: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(mid) = line[from..].find("](").map(|i| from + i) {
        let open = line[..mid].rfind('[');
        let close = line[mid + 2..].find(')').map(|i| mid + 2 + i);
        if let (Some(open), Some(close)) = (open, close) {
            found.push((open..close + 1, mid + 2..close));
        }
        from = mid + 2;
    }
    found
}

/// Whether `text` is one web URL and nothing else.
pub fn is_url(text: &str) -> bool {
    !text.is_empty() && urls(text).first() == Some(&(0..text.len()))
}

/// The URL on the clipboard, when that's all it holds.
pub fn clipboard_url(text: &str) -> Option<String> {
    let url = text.trim();
    is_url(url).then(|| url.to_string())
}

const SCHEMES: [&str; 2] = ["https://", "http://"];

/// The web URLs written out in `text`, by their bytes.
pub fn urls(text: &str) -> Vec<Range<usize>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let byte = |i: usize| chars.get(i).map_or(text.len(), |c| c.0);
    let mut found = Vec::new();
    let mut from = 0;
    while let Some((start, scheme)) = next_scheme(&chars, from) {
        let mut end = start + scheme;
        while end < chars.len() && !ends_url(chars[end].1) {
            end += 1;
        }
        let end = start + trimmed(&chars[start..end]);
        if end > start + scheme {
            found.push(byte(start)..byte(end));
        }
        from = end.max(start + 1);
    }
    found
}

/// Where the next `http://` or `https://` from `from` starts, and its length.
fn next_scheme(text: &[(usize, char)], from: usize) -> Option<(usize, usize)> {
    (from..text.len()).find_map(|i| {
        SCHEMES.iter().find_map(|scheme| {
            let len = scheme.len();
            let matches = text.len() >= i + len && text[i..i + len].iter().map(|c| c.1).eq(scheme.chars());
            matches.then_some((i, len))
        })
    })
}

/// Whether `c` can't be in a URL written out: spaces, quotes, and the
/// lines programs draw boxes with.
fn ends_url(c: char) -> bool {
    c.is_whitespace() || c.is_control() || "<>\"'`{}|\\^".contains(c) || ('\u{2500}'..='\u{257f}').contains(&c)
}

/// How much of `url` is the URL: prose's punctuation after it, and a
/// bracket it didn't open, aren't.
fn trimmed(url: &[(usize, char)]) -> usize {
    let mut end = url.len();
    while let Some(&(_, last)) = url[..end].last() {
        let count = |c: char| url[..end].iter().filter(|x| x.1 == c).count();
        let unopened = |open: char, close: char| last == close && count(open) < count(close);
        if ".,;:!?".contains(last) || unopened('(', ')') || unopened('[', ']') {
            end -= 1;
        } else {
            break;
        }
    }
    end
}
