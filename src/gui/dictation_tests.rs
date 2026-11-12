//! Dictation, with a stand-in for lemur-dictate, into threads that echo what's pasted.

use super::super::core::wait_for;
use super::super::Lemur;
use super::STAND_IN;
use crate::config::Language;
use crate::test_support::{HcServer, TempDir, fake_dictate};
use rune::{Key as K, Mods, Runtime};
use crate::gui::tests::{cmd, key, press, type_text, window};

/// What the stand-in for lemur-dictate hears: two words while it listens, and two more once
/// stopped.
const HEARS: &str = r#"echo '{"listening": true}'
echo '{"finalized": "", "volatile": "Hello"}'
echo '{"finalized": "Hello world.", "volatile": ""}'
cat >/dev/null
echo '{"finalized": "Hello world. Bye.", "volatile": ""}'
echo '{"done": true}'
"#;

/// A window on `server`'s core with `HEARS` for lemur-dictate, and threads `a` and `b` that
/// echo what's pasted into them, once listed.
fn dictating(server: &HcServer, dir: &TempDir) -> ([String; 2], Runtime<Lemur>) {
    let threads = ["a", "b"].map(|_| server.run("/", &[], &["cat"]));
    STAND_IN.with(|h| *h.borrow_mut() = Some(fake_dictate(dir, HEARS)));
    let mut rt = window(server.config());
    rt.state.refresh();
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded), "the first snapshot");
    (threads, rt)
}

/// Opens a thread, and waits for its terminal to attach.
fn attached(rt: &mut Runtime<Lemur>, session: &str) {
    rt.update(|s, _| s.open("local".into(), session.into()));
    let k = key(session);
    assert!(wait_for(rt, 10.0, |s| s.terminals.get(&k).is_some_and(|v| v.is_attached() && v.painted)), "{session} never attached");
}

fn dictated(rt: &Runtime<Lemur>) -> Option<(String, bool)> {
    let bar = rt.state.panels.dictation.as_ref()?;
    Some((bar.input.text.clone(), bar.is_listening()))
}

/// Waits for the bar to hold `text`, listening or not.
fn heard(rt: &mut Runtime<Lemur>, text: &str, listening: bool) {
    let want = Some((text.to_string(), listening));
    let done = wait_for(rt, 5.0, |s| s.panels.dictation.as_ref().map(|b| (b.input.text.clone(), b.is_listening())) == want);
    assert!(done, "{:?}", dictated(rt));
}

/// Whether `session`'s screen comes to show `text`; the window runs meanwhile, as a paste
/// waits for its terminal.
fn shows(rt: &mut Runtime<Lemur>, server: &HcServer, session: &str, text: &str) -> bool {
    wait_for(rt, 5.0, |_| server.hc(&["read", session]).contains(text))
}

/// ⌘D listens and shows what's heard; ⌘⏎ from another thread stops, waits for the rest,
/// and pastes it all there.
#[test]
fn dictation_pastes_into_the_thread_on_show() {
    let server = HcServer::new("rdictate");
    let dir = TempDir::new("dictate");
    let ([a, b], mut rt) = dictating(&server, &dir);
    attached(&mut rt, &a);
    press(&mut rt, K::Command('d'), cmd());
    heard(&mut rt, "Hello world.", true);
    assert!(rt.find("dictation-text").is_some(), "the bar shows");

    // Pasted the moment it's opened: it may still be attaching.
    rt.update(|s, _| s.open("local".into(), b.clone()));
    press(&mut rt, K::Enter, cmd());
    assert!(wait_for(&mut rt, 5.0, |s| s.panels.dictation.is_none()), "{:?}", dictated(&rt));
    assert!(shows(&mut rt, &server, &b, "Hello world. Bye."));
    assert!(!server.hc(&["read", &a]).contains("Hello"));
}

/// Stopped, the text can be changed before ⌘⏎ pastes it, and ⌘D adds to it; Esc throws it
/// away.
#[test]
fn dictation_can_be_corrected_or_thrown_away() {
    let server = HcServer::new("rdictedit");
    let dir = TempDir::new("dictate-edit");
    let ([a, _], mut rt) = dictating(&server, &dir);
    attached(&mut rt, &a);

    press(&mut rt, K::Command('d'), cmd());
    heard(&mut rt, "Hello world.", true);
    // Words typed while it listens would be overwritten, so they aren't taken.
    type_text(&mut rt, "x");
    press(&mut rt, K::Command('d'), cmd());
    heard(&mut rt, "Hello world. Bye.", false);
    type_text(&mut rt, " Ciao.");
    press(&mut rt, K::Command('d'), cmd());
    heard(&mut rt, "Hello world. Bye. Ciao. Hello world.", true);
    press(&mut rt, K::Command('d'), cmd());
    heard(&mut rt, "Hello world. Bye. Ciao. Hello world. Bye.", false);
    press(&mut rt, K::Enter, cmd());
    assert!(shows(&mut rt, &server, &a, "Hello world. Bye. Ciao. Hello world. Bye."));
    assert_eq!(dictated(&rt), None);

    press(&mut rt, K::Command('d'), cmd());
    heard(&mut rt, "Hello world.", true);
    press(&mut rt, K::Escape, Mods::default());
    assert_eq!(dictated(&rt), None);
}

/// A plain Enter in the bar is a new line, not a paste; the language button switches what
/// the next recording listens for.
#[test]
fn enter_is_a_new_line_and_the_language_switches() {
    let dir = TempDir::new("dictate-lang");
    // Says which language it was started for.
    let said = r#"echo "{\"finalized\": \"$2\", \"volatile\": \"\"}"
cat >/dev/null
echo '{"done": true}'
"#;
    let helper = fake_dictate(&dir, said);
    STAND_IN.with(|h| *h.borrow_mut() = Some(helper));
    let mut rt = window(toml::from_str("").unwrap());
    press(&mut rt, K::Command('d'), cmd());
    heard(&mut rt, "en-US", true);
    press(&mut rt, K::Command('d'), cmd());
    heard(&mut rt, "en-US", false);
    press(&mut rt, K::Enter, Mods::default());
    heard(&mut rt, "en-US\n", false);

    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find("dictation-language").expect("the language button");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.prefs.dictation_language, Language::Norwegian);
    press(&mut rt, K::Command('d'), cmd());
    heard(&mut rt, "en-US\nnb-NO", true);

    // With no thread to paste into, the text stays.
    press(&mut rt, K::Command('d'), cmd());
    heard(&mut rt, "en-US\nnb-NO", false);
    press(&mut rt, K::Enter, cmd());
    assert!(rt.state.panels.dictation.is_some());
    assert_eq!(rt.state.status.as_ref().map(|(s, _)| s.as_str()), Some("Open a thread to paste into"));
}
