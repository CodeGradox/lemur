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
