//! The notes' Markdown as it's typed: headings, lists and their boxes, quotes, rules, code,
//! emphasis and links take their colours and weights over the text, which stays as typed.
//! Ticked items fade and their words are struck through.

use super::edits::{checkbox, links, list_item, ticked, urls};
use rune::Span;
use rune_kit::Kit;
use rune_kit::syntax::{SyntaxTheme, highlight};
use std::ops::Range;

/// How the notes look: colours and weights over the text, and the bytes struck through.
pub struct Look {
    pub spans: Vec<Span>,
    pub strikes: Vec<Range<usize>>,
}

/// How `text` looks as Markdown, in `k`'s colours.
pub fn look(text: &str, k: &Kit) -> Look {
    let c = &k.c;
    let mut look = Look { spans: Vec::new(), strikes: Vec::new() };
    // An open code fence: its marker, its language, and where its code starts.
    let mut fence: Option<(&str, &str, usize)> = None;
    let mut start = 0;
    for line in text.split('\n') {
        let end = start + line.len();
        let bare = line.trim_start_matches(' ');
        let indent = line.len() - bare.len();
        let fenced = (indent < 4).then(|| ["```", "~~~"].into_iter().find(|f| bare.starts_with(f))).flatten();
        if let Some((mark, lang, from)) = fence {
            if fenced == Some(mark) && bare.trim_end().chars().all(|c| mark.starts_with(c)) {
                code(text, from..start.saturating_sub(1).max(from), lang, k, &mut look.spans);
                look.spans.push(Span::new(start, end).color(c.text_faint));
                fence = None;
            }
        } else if let Some(mark) = fenced {
            look.spans.push(Span::new(start, end).color(c.text_faint));
            fence = Some((mark, bare[3..].trim(), end + 1));
        } else {
            block(line, start, indent, k, &mut look.spans);
            inline(line, start, k, &mut look);
        }
        start = end + 1;
    }
    if let Some((_, lang, from)) = fence {
        code(text, from.min(text.len())..text.len(), lang, k, &mut look.spans);
    }
    // Ticked items last, so their fade wins over the rest.
    for (line, words) in ticked(text) {
        look.spans.push(Span::new(line.start, line.end).color(c.text_base.alpha(0.5)));
        look.strikes.push(words);
    }
    look
}

/// The code in `range` of a fence in `lang`, in the syntax colours the kit's code has.
fn code(text: &str, range: Range<usize>, lang: &str, k: &Kit, spans: &mut Vec<Span>) {
    let lang = lang.split_whitespace().next().filter(|l| !l.is_empty());
    let Some(source) = text.get(range.clone()).filter(|s| !s.is_empty()) else { return };
    spans.extend(SyntaxTheme::from_kit(k).spans(&highlight(lang, source), range.start));
}

/// What a line is as a whole: a heading, a quote, a rule or a list item.
fn block(line: &str, start: usize, indent: usize, k: &Kit, spans: &mut Vec<Span>) {
    let c = &k.c;
    let end = start + line.len();
    let bare = &line[indent..];
    let hashes = bare.len() - bare.trim_start_matches('#').len();
    let rule = bare.trim_end().chars().filter(|c| *c != ' ').collect::<String>();
    if indent < 4 && (1..=6).contains(&hashes) && (bare.len() == hashes || bare[hashes..].starts_with(' ')) {
        spans.push(Span::new(start, end).weight(600.0).color(c.text_strong));
        spans.push(Span::new(start + indent, start + indent + hashes).color(c.text_faint));
    } else if indent < 4 && bare.starts_with('>') {
        spans.push(Span::new(start, end).color(c.text_muted));
        spans.push(Span::new(start + indent, start + indent + 1).color(c.text_faint));
    } else if indent < 4 && rule.len() >= 3 && ["-", "*", "_"].iter().any(|m| rule.chars().all(|c| m.starts_with(c))) {
        spans.push(Span::new(start, end).color(c.text_faint));
    } else if let Some(at) = list_item(line) {
        spans.push(Span::new(start + indent, start + at).color(c.text_muted));
        if let Some(at) = checkbox(line) {
            spans.push(Span::new(start + at, start + at + 3).color(c.text_accent));
        }
    }
}

/// What's inside a line: code, links, URLs, and bold, italic or struck-out words.
fn inline(line: &str, start: usize, k: &Kit, look: &mut Look) {
    let c = &k.c;
    // Code first: nothing else counts inside it.
    let code = code_spans(line);
    let outside = |r: &Range<usize>| !code.iter().any(|q| q.start < r.end && r.start < q.end);
    let at = |r: Range<usize>| start + r.start..start + r.end;
    for r in &code {
        let r = at(r.clone());
        look.spans.push(Span::new(r.start, r.end).bg(c.layer_03, 1.0));
    }
    for (pair, struck) in [("**", false), ("__", false), ("~~", true)] {
        for r in delimited(line, pair).into_iter().filter(|r| outside(r)) {
            let (whole, marks) = (at(r.clone()), [at(r.start..r.start + 2), at(r.end - 2..r.end)]);
            look.spans.push(if struck {
                look.strikes.push(whole.start + 2..whole.end - 2);
                Span::new(whole.start, whole.end).color(c.text_muted)
            } else {
                Span::new(whole.start, whole.end).weight(600.0)
            });
            look.spans.extend(marks.into_iter().map(|m| Span::new(m.start, m.end).color(c.text_faint)));
        }
    }
    for r in emphasised(line).into_iter().filter(|r| outside(r)) {
        let r = at(r);
        look.spans.push(Span::new(r.start, r.end).italic());
    }
    let links = links(line);
    for (whole, url) in links.iter().filter(|(whole, _)| outside(whole)) {
        let (whole, url) = (at(whole.clone()), at(url.clone()));
        look.spans.push(Span::new(whole.start, whole.end).color(c.text_accent));
        look.spans.push(Span::new(url.start - 2, whole.end).color(c.text_faint));
    }
    for r in urls(line).into_iter().filter(|r| outside(r) && !links.iter().any(|(_, url)| url.start <= r.start && r.end <= url.end)) {
        let r = at(r);
        look.spans.push(Span::new(r.start, r.end).color(c.text_accent));
    }
}

/// The code spans in `line`: text between runs of as many backticks, the backticks too.
fn code_spans(line: &str) -> Vec<Range<usize>> {
    let bytes = line.as_bytes();
    let run = |at: usize| bytes[at..].iter().take_while(|b| **b == b'`').count();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(open) = line[at..].find('`').map(|i| at + i) {
        let n = run(open);
        let mut close = open + n;
        at = loop {
            let Some(next) = line[close..].find('`').map(|i| close + i) else { break open + n };
            let m = run(next);
            if m == n {
                found.push(open..next + m);
                break next + m;
            }
            close = next + m;
        };
    }
    found
}

/// What `pair` (`**`, `__`, `~~`) encloses in `line`, the pairs too: words that start right
/// after the opening one and end right before the closing one.
fn delimited(line: &str, pair: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(open) = line[at..].find(pair).map(|i| at + i) {
        let inner = open + pair.len();
        let starts = line[inner..].chars().next().is_some_and(|c| !c.is_whitespace());
        let close = line[inner..].find(pair).map(|i| inner + i).filter(|&close| close > inner);
        match close.filter(|&close| starts && !line[..close].ends_with(char::is_whitespace)) {
            Some(close) => {
                found.push(open..close + pair.len());
                at = close + pair.len();
            }
            None => at = inner,
        }
    }
    found
}

/// The words `*` or `_` alone put in italics in `line`, the marks too. A `_` inside a word
/// (snake_case) and a list item's `*` aren't marks.
fn emphasised(line: &str) -> Vec<Range<usize>> {
    let bytes = line.as_bytes();
    let skip = list_item(line).filter(|_| line.trim_start().starts_with('*')).unwrap_or(0);
    let lone = |at: usize, mark: u8| bytes[at] == mark && (at == 0 || bytes[at - 1] != mark) && bytes.get(at + 1) != Some(&mark);
    let word = |at: Option<usize>| at.and_then(|i| bytes.get(i)).is_some_and(|b| b.is_ascii_alphanumeric());
    let mut found = Vec::new();
    let mut at = skip;
    while at < bytes.len() {
        let mark = bytes[at];
        let opens = matches!(mark, b'*' | b'_')
            && lone(at, mark)
            && bytes.get(at + 1).is_some_and(|b| !b.is_ascii_whitespace())
            && !(mark == b'_' && word(at.checked_sub(1)));
        let close = (at + 2..bytes.len()).find(|&i| lone(i, mark) && !bytes[i - 1].is_ascii_whitespace() && !(mark == b'_' && word(Some(i + 1))));
        match close.filter(|_| opens) {
            Some(close) => {
                found.push(at..close + 1);
                at = close + 1;
            }
            None => at += 1,
        }
    }
    found
}
