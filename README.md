# Lemur

A macOS app for Claude Code sessions that live on a remote host: a sidebar of sessions next to a built-in terminal. [herdr-core](https://github.com/herdrdev/herdr-core) runs the terminals, so sessions keep running when your laptop sleeps.

[![Lemur in use: Claude threads by project and worktree, answering one that waits, ⌘J for a new shell, ⌃Tab between threads, a pasted screenshot, and notes](docs/demo.avif)](docs/demo.avif)

## Install

Download `lemur-macos-arm64.dmg` from the [latest release](https://github.com/CodeGradox/lemur/releases/latest), open it and drag Lemur into Applications. It's signed and notarized, and needs Apple Silicon and macOS 11 or newer (dictation, macOS 26).

Lemur was called lemen up to 1.0.0. Coming from lemen, move `~/.config/lemen` to `~/.config/lemur` (Lemur won't start without its config) and `~/.lemen` to `~/.lemur` (your notes). `lemen.app` can go once Lemur runs.

Lemur runs its terminals in herdr-core (`hc`), here and on each host, where it needs `git` too. Install herdr-core on this Mac and on each host:

```sh
curl -fsSL https://releases.herdr.dev/core/install.sh | sh
```

herdr-core reaches the hosts over SSH itself; Lemur adds each host from the config to it the first time (`hc machine ls` lists them). A host whose SSH login asks for a password needs `hc machine login <name>` in a terminal first. Lemur shares herdr-core with Herdr unless a host sets `hc_session`. Lemur 1.8 and older ran sessions in tmux: those don't show up, but Claude threads Lemur kept resume in herdr-core.

From source:

```sh
sh scripts/build-app.sh  # build/Lemur.app; drag it into Applications
cargo install --path . --locked  # the lemur command, for `lemur ls`, `lemur paste` and the rest
```

Building needs Xcode 26 or newer (GPUI compiles Metal shaders, and dictation's helper is Swift for macOS 26), and the tests need herdr-core. [docs/releases.md](docs/releases.md) covers the disk image and signed releases.

## Config

`~/.config/lemur/config.toml` (or `$LEMUR_CONFIG`):

```toml
# theme = "system"                # or "light" / "dark"; must come before the tables
# dictation_language = "en-US"    # or "nb-NO", what ⌘D listens for; before the tables too
# sidebar_width = 255             # pixels; dragging the sidebar's edge sets it
# notes_width = 360               # pixels; dragging the notes panel's edge sets it

# [terminal]
# font_family = "Menlo"
# font_size = 13                  # points, 6-72

# [terminal.dark]                 # colors while Lemur is dark, as WezTerm names them;
# background = "#282c34"          # what's left out is Lemur's own (One Half Dark)
# foreground = "#dcdfe4"
# cursor = "#979db4"
# ansi = ["#282c34", "#e17079", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#dcdfe4"]
# brights = ["#8c939f", "#e17079", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff"]
# [terminal.light]                # the same while Lemur is light (One Half Light)

[[hosts]]
name = "agent"
ssh = "agent"                     # user@host or a ~/.ssh/config alias; "local" runs here
# upload_dir = "~/.lemur/uploads"
# hc_session = "lemur"            # herdr-core session its terminals run in; herdr-core's default (Herdr's too) if unset
# claude = "claude"               # command typed into new sessions; Lemur adds --session-id, or --resume

[[bases]]
host = "agent"
path = "~/projects/api"
# name = "api"                    # defaults to the last path component
# worktree_dir = ".claude/worktrees"
# collapsed = true                # folded in the sidebar
# worktree_order = [...]          # worktree paths, in the order they were dragged into
# worktree_colors = { "/path/to/worktree" = "blue" }  # red orange yellow green teal cyan blue purple pink gray
# hidden_worktrees = [...]        # worktree paths in the project's Hidden section
# collapsed_worktrees = [...]     # worktree paths whose threads are folded
# hidden_open = true              # the Hidden section is open
```

Settings (`⌘,`) writes `theme`, `dictation_language`, `[terminal]`, `[terminal.dark]` and `[terminal.light]` for you, dragging a side panel's edge writes `sidebar_width` or `notes_width`, and the sidebar writes a project's `collapsed`, `worktree_order`, `worktree_colors`, `hidden_worktrees`, `collapsed_worktrees` and `hidden_open`, and the order of the `[[bases]]`. Lemur also keeps a `[[threads]]` entry for each Claude thread (its `host`, its terminal's address as `session`, `path`, `claude_session` ID and `label`) to resume it. The rest of the file, comments included, stays as it is.

New worktrees go in `worktree_dir` (relative to the repo, absolute, or `~/...`): Lemur runs `git worktree add` there, then starts Claude in it.

## Use

Run `lemur`. The sidebar lists each base's worktrees and their sessions, in the config's order whatever their host, then the sessions outside them, in one **Other** section, each host's under its name. Threads come and go, and their dots change, as herdr-core tells Lemur; worktrees refresh every 3 s. A worktree shows as its branch (`worktree-` dropped from the branches `claude -w` and older lemurs made), or its folder's name when detached: a linked worktree with a branch icon, the main checkout with a folder.

- **+** at the top of the sidebar adds a folder, as in Zed: pick a host (yours from the config, an alias from `~/.ssh/config`, or This Mac; or type `user@host`), then type the path while its folders are suggested (↑↓ to choose, Tab to go into one, Enter to add). A git repository shows each of its worktrees; a folder without git is a single row, with no **New worktree**. A folder inside a repository adds the repository. It's saved to the config as a `[[bases]]` entry, with a `[[hosts]]` entry for a new host.
- Click a session to show it. Each opened session keeps its own terminal, so switching is instant. Lemur reopens the last session on launch. The terminal sits in a frame whose header says what runs in it (Claude Code or a shell) and where; the ring around it says it has the keyboard.
- The tabs above the terminal are the threads of its worktree (outside every worktree, its host's other threads): click one to show it, double-click to rename it, and its **×**, clicked twice, kills it. **+** starts another, as `⌘T` does.
- The status bar under the terminal names its host (This Mac, or the host's name), its worktree's branch, and the servers running in that worktree, like a Rails app overmind started: click `:3020` to open it in your browser, on another host by its [Tailscale](https://tailscale.com) name when it has one. A server that listens only on another host's own localhost is shown but not linked. It counts the agents (Claude, for now), and how many need you.
- At the foot of the sidebar, each Linux host shows its CPU and memory as bars, and its load; hover one for its cores and swap.
- **New worktree** starts a thread in a new worktree; **+** on a worktree, or `⌘T` for the worktree of the thread on show (a new tab), starts another session in it: **Claude**, or a plain **Shell**, picked in the spotlight with ↑↓ or a click. A spotlight asks for a label (`claude` or `shell` to start with, following the pick until you type one; a free random name for a new worktree): press Enter to take it, or type your own; Esc or a click outside cancels. For a new worktree it also asks for a **Branch** to create, empty for a detached worktree, and what to start **From**, picked from the host's branches; that starts at the default branch (`origin/HEAD`, like `origin/main`). A remote's branch is fetched first; if that fails, the worktree starts where it was last fetched, and the header says so. Several sessions can share a label. Claude sessions get Claude's spark as their icon, shells a terminal.
- `⌘T` lists the threads to go to as well, under the new one's name: the worktree's first, then its project's, then the rest. Typing narrows them by name, project, worktree and host; ↓ goes past Claude and Shell to them, and Enter opens the one picked. `⌘P`, or the magnifier at the top of the sidebar, lists all the threads to go to, the ones opened last first.
- **×** on a session, clicked twice, kills it; `⌘W` asks first, then closes the one on show. The worktree stays. When the thread on show ends, however it ends, the one you opened before it in its worktree shows instead, else the tab next to it.
- When a thread's terminal is gone, as after the host restarts, Lemur still knows its Claude conversation: the thread stays under its worktree, dimmed. Click it, or its **▶**, to start a terminal for it again and resume the conversation there (`claude --resume`); a project's **⋯** → **Resume all threads** brings back each of its threads, and right-click → **Forget** drops one instead. New Claude threads start with `claude --session-id`, and while threads run Lemur keeps their IDs up to date from `claude agents`, after a `/clear` too; a thread started before Lemur kept IDs gets one once Lemur sees it running. Killing a thread in Lemur, or removing its worktree, forgets it.
- Right-click a worktree → **Remove worktree…** to take it out of git; its branch stays. If threads are running in it, the dialog names them: **Close and remove** closes them first, and **Cancel** keeps them and the worktree. Lemur checks the host too, so a thread the sidebar hasn't shown yet, or one whose terminal has `cd`'d into the worktree, is asked about the same way. Its folder stays on the host as plain files unless you tick **Also delete**, which deletes it, read-only parts included. One that's locked (Claude Code locks the worktree its session runs in) asks again before **Remove anyway**, which unlocks it; so does deleting one with uncommitted changes, which drops them. These asks all come before any thread closes, so cancelling one keeps the threads running. The main checkout and plain folders can't be removed this way.
- Click a project's name to fold it away, or open it again, and a worktree to fold its threads. What's folded stays so, across restarts too. Opening a thread some other way (a popup's **Show**, `⌃Tab`, or on launch) unfolds its project and worktree, and the **Hidden** section it's in, so you can see where it is.
- Right-click a worktree → **Color** to give its folder icon one of ten colors.
- Right-click a worktree → **Hide** to move it into its project's **Hidden** section, at the bottom; click **Hidden** to fold it open and shut. Right-click it there → **Unhide** to move it back to its place.
- Drag a worktree onto another in the same project to move it there.
- Drag a project's name onto another's to move it there, whatever their hosts; the config's `[[bases]]` take the new order, each entry with its comments.
- A project's **⋯** (on hover) or right-click → **Remove project…** takes it out of Lemur and the config. Nothing on the host changes, and its running threads move to **Other**.
- Double-click a session, or right-click → **Rename…**, to change its label. Claude keeps running and its worktree and branch keep their names.
- Drag a thread onto another in the same worktree to move it to that place. The order is stored on the terminals in herdr-core (their `lemur.order` label), so it survives restarts and is the same from any Mac; new threads start at the bottom.
- `⌃Tab` shows the worktree's next thread: the one you opened before this one. Hold `⌃` and press `Tab` again to go further back, as `⌘Tab` does with apps; letting go of `⌃` stays there. `⌃⇧Tab` goes the other way.
- `⌘B` or the header button collapses the sidebar. `⌘Q` quits; sessions keep running.
- The menu bar has them too: **File** → **Add Project…** (`⌘O`), **New Thread in Worktree…** (`⌘T`), **Go to Thread…** (`⌘P`), **Close Thread…** (`⌘W`), **Dictate** (`⌘D`) and **Refresh**; **View** → **Toggle Sidebar** (`⌘B`), **Toggle Notes** (`⌘R`, as Zed's right dock), and **Next** and **Previous Thread in Worktree** (`⌃Tab`, `⌃⇧Tab`).
- `⌘,` (**Lemur → Settings…**) opens Settings: the theme (System, Light or Dark), the terminal's font and size, its colors for a dark and a light Lemur (One Half Dark and Light to start), and the language dictation listens for. Changes apply at once and are saved to the config. **Shortcuts** lists every shortcut.
- A terminal whose connection dropped (laptop slept) says so; click it to reconnect.

**Notes:** every worktree has a notepad. `⌘R` or the button at the right of the header opens it beside the terminal, on the worktree of the thread you opened last; a worktree's notes icon opens that one's. Type straight into it: it's Markdown, highlighted as you go. `⌘`-click a URL to open it; holding `⌘` underlines it first. Pasting a URL over selected text makes it a link, `[text](url)`, and so does `⌘K`, with the URL on the clipboard; `⌘K` in a link selects its URL. The two buttons at the top of the notes, or `⇧⌘L` for the checklist, make the cursor's line, or the selected lines, a bullet list or a checklist, and plain lines again when they are one already. `- [ ] ` starts an item with a box: `⌘`-click the box, or press `⌘↩` or `⇧⌘U` on its line, to tick it, and again to untick it; ticked items fade and are struck through. Enter at the end of a list item starts the next, and Enter on an empty one ends the list; Tab and `⇧Tab` indent and outdent an item, and `⌃⌘↑` and `⌃⌘↓` move it, with its nested items. These are Apple Notes' keys. Notes are saved as you type, on this Mac, in `~/.lemur/notes/<host>/<worktree path>.md`.

**Dictation:** `⌘D` opens a bar under the terminal and listens: what you say shows there as you say it, transcribed on your Mac by macOS's own speech recognition, so no audio leaves it. `⌘D` again stops listening; the text is then yours to correct, and `⌘D` once more adds to it. `⌘⏎` or **Paste** pastes it into the thread on show without sending it, and closes the bar; pressed while Lemur listens, it waits for your last words. The bar stays while you switch threads, so you can dictate first and pick the thread after: open it, then `⌘⏎`. Esc or **×** throws the text away. The button named after the language switches between English and Norwegian Bokmål from the next recording on, as Settings does. The first time, macOS asks whether Lemur may use the microphone. Dictation needs macOS 26 or newer.

**Images:** drop files on the terminal, or `⌘V` with an image or copied files on the clipboard. Lemur uploads them to the host's `upload_dir` and pastes the remote paths. Text pastes as text.

**Links:** hold `⌘` over a link in the terminal to underline it, and `⌘`-click to open it in your browser. That's an `http(s)://` URL written out in the text, or a link whose text hides its URL (OSC 8). A URL runs on across rows the terminal wrapped, but not across lines Claude Code broke itself, which a long one in its replies may be.

A file path written out in the terminal, like `src/main.rs:42` or `/etc/hosts`, is a link too: `⌘`-click it to copy the file from the host to a temporary folder on your Mac and preview it with Quick Look, which never runs it. A relative path is from the session's current folder; a `:line` after it is left out.

**Scrolling and selection:** the wheel scrolls back through what the terminal showed; opening a thread brings its last 2000 lines from herdr-core. Drag to select, double-click for a word and triple-click for a line, a drag off the terminal too; `⌘C` copies it. A program that asks for the mouse gets the clicks and the wheel instead; hold `⇧` to select anyway. Text a program copies (OSC 52) goes on your Mac clipboard.

CLI: `lemur ls`, `lemur new <base> <name> [--branch <branch>] [--from <branch>]` (a new worktree; detached without `--branch`) or `lemur new <base> <name> --worktree <path> [--shell]`, `lemur attach <host> <session>` (herdr-core's own attach; `ctrl+\` detaches), `lemur rename <host> <session> <name>`, `lemur upload <files>...`, `lemur paste` (clipboard image to the last session Lemur showed). A session is its terminal's address in herdr-core, as `lemur ls` prints it, like `agent/t7`.

### Status dots

Each session's dot comes from Claude Code itself: Claude reports what it's doing to the terminal it runs in, and herdr-core tells Lemur at once. Where it doesn't, each refresh runs `claude agents --json` on the host, which lists each running Claude with its process and status, and Lemur matches the process to its terminal; that's also where Lemur learns each Claude's conversation, to resume it. No setup needed; `claude agents` needs Claude Code 2.1.139 or later, on `PATH` or in `~/.local/bin`.

- **Green, breathing:** Claude is working.
- **Yellow:** Claude needs input (a question or permission prompt); hover the dot for what it's asking.
- **Blue:** it finished while you were looking at another session; opening it clears that.
- **Grey:** idle, or no Claude running.

### Notifications

When a session finishes or starts needing input, a popup appears at the top-right of the main screen, like Zed's, unless it's the session you're looking at in the active window. It names the project and worktree under the session's name. **Show** brings Lemur forward and opens that session; **Dismiss** closes the popup. Opening the session from the sidebar closes its popup too, and so does coming back to Lemur while it's the session on show. A popup never keeps the keyboard: if `⌘Tab` back to Lemur hands it to one, it goes on to the main window.

What Claude reports arrives at once. The refresh, for worktrees, servers, hosts' load and `claude agents`, runs every 3 seconds while Lemur is in front, and every 15 seconds while it's in the background.

## Trademarks

Lemur is an independent project, not affiliated with or endorsed by Anthropic. Claude, Claude Code and the Claude logo are trademarks of Anthropic.
