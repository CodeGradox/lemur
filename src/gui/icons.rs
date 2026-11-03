//! Icons lemur draws that gpui-kit doesn't ship with: Lucide's, as
//! gpui-kit's own are (ISC, see `icons/LICENSE-LUCIDE`).

use gpui_kit::component::Icon;

/// A linked worktree: a branch off the project.
pub fn git_branch() -> Icon {
    Icon::empty().data(include_bytes!("icons/git-branch.svg"))
}

/// A host lemur reaches over SSH.
pub fn server() -> Icon {
    Icon::empty().data(include_bytes!("icons/server.svg"))
}

/// This Mac.
pub fn monitor() -> Icon {
    Icon::empty().data(include_bytes!("icons/monitor.svg"))
}

/// A worktree's changes since its last commit.
pub fn file_diff() -> Icon {
    Icon::empty().data(include_bytes!("icons/file-diff.svg"))
}

/// Giving something another name.
pub fn pencil() -> Icon {
    Icon::empty().data(include_bytes!("icons/pencil.svg"))
}

/// Codex's threads: code, not a logo of OpenAI's.
pub fn code_xml() -> Icon {
    Icon::empty().data(include_bytes!("icons/code-xml.svg"))
}

/// A thread's notifications, to silence.
pub fn bell() -> Icon {
    Icon::empty().data(include_bytes!("icons/bell.svg"))
}

/// A thread silenced.
pub fn bell_off() -> Icon {
    Icon::empty().data(include_bytes!("icons/bell-off.svg"))
}

/// A Linear issue.
pub fn ticket() -> Icon {
    Icon::empty().data(include_bytes!("icons/ticket.svg"))
}

/// Links: Claude artifacts and pull requests.
pub fn link() -> Icon {
    Icon::empty().data(include_bytes!("icons/link.svg"))
}

/// A Claude artifact.
pub fn artifact() -> Icon {
    Icon::empty().data(include_bytes!("icons/shapes.svg"))
}

/// The language dictation listens for.
pub fn languages() -> Icon {
    Icon::empty().data(include_bytes!("icons/languages.svg"))
}

/// Stops dictation listening.
pub fn circle_stop() -> Icon {
    Icon::empty().data(include_bytes!("icons/circle-stop.svg"))
}

/// The Shortcuts page.
pub fn keyboard() -> Icon {
    Icon::empty().data(include_bytes!("icons/keyboard.svg"))
}

#[cfg(test)]
mod tests {
    use gpui_kit::AssetSource;
    use gpui_kit::assets::{Assets, IconName, IconNamed};
    use std::path::Path;

    /// `Assets` holds only some of gpui-kit's icons, and one it doesn't hold
    /// draws as nothing, with no error to see: each icon Lemur names by
    /// name must be in it. The rest go in `icons/`, as those above.
    #[test]
    fn every_icon_named_is_one_lemur_ships() {
        fn check(dir: &Path, missing: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    check(&path, missing);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (at, needle) in text.match_indices("IconName::") {
                        let rest = &text[at + needle.len()..];
                        let name: String = rest
                            .chars()
                            .take_while(|c| c.is_ascii_alphanumeric())
                            .collect();
                        // What isn't an icon's name, as `ALL` here.
                        let Some(icon) = IconName::ALL.iter().find(|i| format!("{i:?}") == name)
                        else {
                            continue;
                        };
                        let shipped = Assets.load(&IconNamed::path(*icon));
                        if !shipped.is_ok_and(|bytes| bytes.is_some()) {
                            missing.push(format!("{name} in {}", path.display()));
                        }
                    }
                }
            }
        }
        let mut missing = vec![];
        check(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut missing,
        );
        assert!(missing.is_empty(), "not in the bundle: {missing:#?}");
    }
}
