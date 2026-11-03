# Lemur

A macOS app for Claude Code and Codex sessions that live on a remote host: a sidebar of sessions next to a built-in terminal. [herdr-core](https://github.com/herdrdev/herdr-core) runs the terminals, so sessions keep running when your laptop sleeps.

[![Lemur in use: Claude threads by project and worktree, answering one that waits, ⌘J for a new shell, ⌃Tab between threads, a pasted screenshot, and notes](docs/demo.avif)](docs/demo.avif)

## Install

Download `lemur-macos-arm64.dmg` from the [latest release](https://github.com/CodeGradox/lemur/releases/latest), open it and drag Lemur into Applications. It's signed and notarized, and needs Apple Silicon and macOS 11 or newer (dictation, macOS 26).

Lemur was called lemen up to 1.0.0. Coming from lemen, move `~/.config/lemen` to `~/.config/lemur` (Lemur won't start without its config) and `~/.lemen` to `~/.lemur` (your notes). `lemen.app` can go once Lemur runs.

Lemur runs its terminals in herdr-core (`hc`), here and on each host, where it needs `git` too. Install herdr-core on this Mac and on each host:

```sh
curl -fsSL https://releases.herdr.dev/core/install.sh | sh
```

herdr-core reaches the hosts over SSH itself; Lemur adds each host from the config to it the first time (`hc machine ls` lists them). When herdr-core needs you to add a host (to install herdr-core there or update it, a password, a new host key), picking the host in the folder picker, or **Set up** under its error in the sidebar, opens `hc machine add` in a terminal over the window, where you answer it, as Herdr's Add a machine does (`hc machine login` once a host added before asks for a login again); **×** or `⌘W` once it's done, and Lemur asks herdr-core again. Lemur shares herdr-core with Herdr unless a host sets `hc_session`. Lemur 1.8 and older ran sessions in tmux: those don't show up, but Claude threads Lemur kept resume in herdr-core, once their tmux session has ended (Lemur won't start a second Claude on a conversation still running there).

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
# thread_order = "tabs"           # or "recent", the order ⌃Tab and ⌃⇧Tab go in; before the tables too
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
# codex = "codex"                 # command typed into new Codex threads, as it is

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

Settings (`⌘,`) writes `theme`, `dictation_language`, `claude_usage`, `codex_usage`, `[terminal]`, `[terminal.dark]` and `[terminal.light]` for you, dragging a side panel's edge writes `sidebar_width` or `notes_width`, minimizing the sidebar's foot writes `minimized_usage`, `minimized_codex_usage` and `minimized_hosts`, and the sidebar writes a project's `collapsed`, `worktree_order`, `worktree_colors`, `hidden_worktrees`, `collapsed_worktrees` and `hidden_open`, and the order of the `[[bases]]`. Lemur also keeps a `[[threads]]` entry for each Claude thread (its `host`, its terminal's address as `session`, `path`, `claude_session` ID, `label`, and the Linear `issue` pinned to it) to resume it. The rest of the file, comments included, stays as it is.

New worktrees go in `worktree_dir` (relative to the repo, absolute, or `~/...`): Lemur runs `git worktree add` there, then starts what you picked in it.

## Use

Run `lemur`. The sidebar lists each base's worktrees and their sessions, in the config's order whatever their host, then one **Other** section with a row for every host and its sessions outside them under it; a host's **+** (on hover) or right-click starts a terminal or Claude in its home folder. Threads come and go, and their dots change, as herdr-core tells Lemur; worktrees refresh every 3 s. A worktree shows as its branch (`worktree-` dropped from the branches `claude -w` and older lemurs made), or its folder's name when detached: a linked worktree with a branch icon, the main checkout with a folder.

- **+** at the top of the sidebar adds a folder, as in Zed: pick a host (yours from the config, an alias from `~/.ssh/config`, or This Mac; or type `user@host`), then type the path while its folders are suggested (↑↓ to choose, Tab to go into one, Enter to add). A git repository shows each of its worktrees; a folder without git is a single row, with no **New worktree**. A folder inside a repository adds the repository. It's saved to the config as a `[[bases]]` entry, with a `[[hosts]]` entry for a new host.
- Click a session to show it. Each opened session keeps its own terminal, so switching is instant. Lemur reopens the last session on launch. The terminal sits in a frame whose header says what runs in it (Claude Code or a shell), its name and where; the ring around it says it has the keyboard.
- The tabs above the terminal are the threads of its worktree (outside every worktree, its host's other threads), kept narrow, with the name showing when you hover one: click one to show it, double-click to rename it, and its **×** closes it, as `⌘W` does. Right-click one to rename it, start a terminal beside it or close it. Drag one onto another to move it there; its thread moves in the sidebar too, as they're one order. **+** offers what can start there.
- With more tabs than fit, the bar scrolls to keep the one on show in view. Scroll it yourself to see the others; it snaps back the next time a thread is shown.
- `⌘T` starts a terminal where the thread on show runs, in a new tab: its worktree, or its folder outside every worktree.
- The status bar under the terminal names its host (This Mac, or the host's name), its worktree's branch and the lines added and removed there since its last commit (`+12 -3`, staged or not; files git doesn't track yet aren't counted; hover for how many files), and the servers running in that worktree, like a Rails app overmind started: click `:3020` to open it in your browser, on another host by its [Tailscale](https://tailscale.com) name when it has one. A server that listens only on another host's own localhost is shown but not linked. It says how many agents (Claude, for now) need you, when any do.
- At the foot of the sidebar, each Linux host shows its CPU and memory as bars, and its load; hover one for its cores and swap. Click a host's name to minimize it to that line, its CPU and memory beside it, and again to open it.
- Above them, once turned on in Settings (**Claude** → **Show plan usage**), how much of your Claude plan is used: a bar for each limit Claude Code's `/usage` lists, as the week and one model's week, and when the next one starts over. Hover for each; click the bars to ask now. Click **Claude** to minimize it to that line, the most used limit beside it, and again to open it; what's minimized stays so across restarts. Lemur runs `claude -p /usage` (the host's `claude` command) every 10 minutes, 30 while Lemur isn't in front and longer after a failure, on the host where Claude ran last. Claude Code does the asking, signed in as it is; it's no model request, and isn't kept as a session. Settings offers it once Lemur has seen Claude on one of your hosts. If Claude Code can't answer, or answers in a way Lemur doesn't know, the meter says it couldn't fetch usage, and why on hover.
- Beside it, once turned on in Settings (**Codex** → **Show plan usage**), the same for Codex on a ChatGPT plan: a bar for each limit it lists. Lemur starts `codex app-server` (the host's `codex` command), asks it `account/rateLimits/read` and lets it go, as often as for Claude, on the host where Codex ran last. Codex does the asking, signed in as it is; it's no model request. Signed in with an API key, Codex has no plan limits, so the meter shows the tokens it used on that host over the last 24 hours and 7 days instead: new input and output, as Codex totals them, then what came from its cache. Lemur adds them up from Codex's session logs there (`~/.codex/sessions`, or under `CODEX_HOME`), and only the counts leave the host. They're tokens, not dollars: cached ones are usually billed at a discount, and what any of them cost depends on the model.
- **+** on a worktree (it shows while the pointer is on the row) or the tab bar, and a worktree's right-click menu, offer **New terminal** (`⌘T` for the thread on show), **New Claude** and **New Codex**, which start at once, and **Claude, Codex or shell, named…** (`⌘J`), as Herdr's menus do. A folded worktree says how many agents and shells run in it, with a dot when one needs you, until the pointer is on it.
- **New worktree** starts a thread in a new worktree; **Claude, Codex or shell, named…**, or `⌘J` for where the thread on show runs (its worktree, or its folder outside every worktree), starts another session there: **Claude**, **Codex**, or a plain **Shell**, picked in the spotlight with ↑↓ or a click. A host without the agent picked says so, and nothing starts: no terminal, no worktree. Lemur asks the host's shell as a thread would, so what your shell's profile sets up counts. A spotlight asks for a label (`claude`, `codex` or `shell` to start with, following the pick until you type one; a free random name for a new worktree, which is the worktree's, its thread showing what its program calls it): press Enter to take it, or type your own; Esc or a click outside cancels. For a new worktree it also asks for a **Branch** to create, empty for a detached worktree, and what to start **From**, picked from the host's branches; that starts at the default branch (`origin/HEAD`, like `origin/main`). A remote's branch is fetched first; if that fails, the worktree starts where it was last fetched, and the header says so. Several sessions can share a label. Claude sessions get Claude's spark as their icon, shells a terminal.
- `⌘J` lists the threads to go to as well, under the new one's name: the worktree's or folder's first, then its project's, then the rest. Typing narrows them by name, project, worktree and host; ↓ goes past Claude, Codex and Shell to them, and Enter opens the one picked. `⌘P`, or the magnifier at the top of the sidebar, lists all the threads to go to, the ones opened last first.
- **×** on a thread, in the sidebar or its tab, closes it, and so does `⌘W` for the one on show; each asks first while something other than its shell runs in it, as Claude or a server, or a job its shell started (sent off with `&`, or stopped with `⌃Z`). The worktree stays. When the thread on show ends, however it ends, the next tab in its worktree shows instead, or the one before it if it was the last. With **Thread order** in **Settings** → **Shortcuts** set to **By last opened**, the one you opened before it shows instead, else the tab next to it.
- When a thread's Claude is gone with its terminal, or with the host restarting (herdr-core then gives the terminal a fresh shell at the same address; Lemur tells by the shell's pid, which it notes as the terminal's `lemur.shell` label), Lemur still knows its Claude conversation: the thread stays under its worktree, dimmed. Click it, or its **▶**, to resume the conversation (`claude --resume`) in that fresh shell, or in a new terminal when the old one is gone; a project's **⋯** → **Resume all threads** brings back each of its threads, and right-click → **Forget** drops one instead. New Claude threads start with `claude --session-id`, and while threads run Lemur keeps their IDs up to date from `claude agents`, after a `/clear` too; a thread started before Lemur kept IDs gets one once Lemur sees it running. Killing a thread in Lemur, or removing its worktree, forgets it.
- Codex threads aren't kept to resume yet: with the host restarting, one comes back as a fresh shell, where `codex resume` picks the conversation.
- Right-click a worktree → **Remove worktree…** to take it out of git; its branch stays. If threads are running in it, the dialog names them: **Close and remove** closes them first, and **Cancel** keeps them and the worktree. Lemur checks the host too, so a thread the sidebar hasn't shown yet, or one whose terminal has `cd`'d into the worktree, is asked about the same way. Its folder stays on the host as plain files unless you tick **Also delete**, which deletes it, read-only parts included. One that's locked (Claude Code locks the worktree its session runs in) asks again before **Remove anyway**, which unlocks it; so does deleting one with uncommitted changes, which drops them. These asks all come before any thread closes, so cancelling one keeps the threads running. The main checkout and plain folders can't be removed this way.
- Click a project's name to fold it away, or open it again, and a worktree to fold its threads. What's folded stays so, across restarts too. Opening a thread some other way (a popup's **Show**, `⌃Tab`, or on launch) unfolds its project and worktree, and the **Hidden** section it's in, so you can see where it is.
- Right-click a worktree → **Color** to give its folder icon one of ten colors.
- Right-click a worktree → **Hide** to move it into its project's **Hidden** section, at the bottom; click **Hidden** to fold it open and shut. Right-click it there → **Unhide** to move it back to its place.
- Drag a worktree onto another in the same project to move it there.
- Drag a project's name onto another's to move it there, whatever their hosts; the config's `[[bases]]` take the new order, each entry with its comments.
- A project's **⋯** (on hover) or right-click → **Remove project…** takes it out of Lemur and the config. Nothing on the host changes, and its running threads move to **Other**.
- A thread shows what its program calls it, as Claude's title for what it's working on, in the sidebar, tabs and panels, as Herdr does. Until the program says something more than its own name or the folder, it shows what it started as (`claude`, `codex` or `shell`). Double-click a session, or right-click → **Rename…**, or type a name when starting it, to name it yourself: that stays, whatever the program says. Claude keeps running and its worktree and branch keep their names.
- Peek at a thread (right-click → **Peek**, or the eye in the agents panel) to see its terminal over the one on show, live, without switching to it, as Herdr peeks at a sub-agent: **↗** shows it instead, and **×**, `⌘W` or a click outside puts the one on show back.
- Right-click a thread, or its tab, → **Put away** (or **File** → **Put Thread Away** for the one on show) to take it out of its list and the tabs while it keeps running, as Herdr puts a tab away: it goes under its list's **Put away**, dimmed (click to open that), and the one you opened before shows instead. It comes back when it needs you, or when you open it (from **Put away**, `⌘P` or the agents panel), or with right-click → **Bring back**. herdr-core keeps it as a label on its terminal, so it stays away across restarts.
- Drag a thread onto another in the same worktree to move it to that place. The order is stored on the terminals in herdr-core (their `lemur.order` label), so it survives restarts and is the same from any Mac; new threads start at the bottom.
- `⌃Tab` shows the worktree's next tab and `⌃⇧Tab` the one before, going round at the ends. Hold `⌃` and press `Tab` again to go further; letting go of `⌃` stays there. **Settings** → **Shortcuts** can have them go by when threads were last opened instead, the one you opened before this one first, as `⌘Tab` does with apps.
- The first nine tabs above the terminal show a number: `⌘1` shows the first, `⌘2` the second, and so on up to `⌘9`, the ninth.
- `⌘B` or the header button collapses the sidebar. `⌘Q` quits; sessions keep running.
- There's no title bar, as in Herdr: the window's buttons sit in the sidebar's top row (or the tab bar's, with the sidebar collapsed). Drag either row where nothing else is to move the window, and double-click it to zoom.
- `⌘⇧P` opens the command palette, as Herdr's: every command by name, with its key; type to narrow them down, ↑↓ to pick, ⏎ to run.
- The menu bar has them too: **File** → **Add Project…** (`⌘O`), **New Terminal** (`⌘T`), **New Thread in Worktree…** (`⌘J`), **Go to Thread…** (`⌘P`), **Close Thread** (`⌘W`), **Dictate** (`⌘D`) and **Refresh**; **View** → **Toggle Sidebar** (`⌘B`), **Toggle Right Panel** (`⌘R`, as Zed's right dock: the notes, or the agents if they showed last), **Toggle Notes**, **Toggle Linear**, **Toggle Changes**, **Toggle Links**, **Toggle Agents** (`⌘⇧A`), **Next** and **Previous Thread in Worktree** (`⌃Tab`, `⌃⇧Tab`), and **Next** and **Previous Agent**.
- `⌘,` (**Lemur → Settings…**) opens Settings: the theme (System, Light or Dark), the terminal's font and size, how many lines each terminal keeps to scroll back through (2,000 to start; herdr-core keeps about 2,600 of its own, which come back when Lemur attaches again; more apply at once, fewer when a terminal attaches again), its colors for a dark and a light Lemur (One Half Dark and Light to start), the language dictation listens for, whether to show your Claude plan's usage, Lemur's Linear sign-in, and herdr-core's version here and on each host. Changes apply at once and are saved to the config. **Shortcuts** lists every shortcut.
- A terminal whose connection dropped (laptop slept) says so, and attaches again on its own, as Herdr's do, once its program runs again: when the line to its host or to herdr-core is back, or a kept program's new shell has started. A click reconnects at once. While a host can't be reached, its threads' terminals stay open.

**Agents:** `⌘⇧A`, or **Agents** at the top right, opens a panel beside the terminal that lists every Claude and Codex thread on every host, in the sidebar's order, which stays the same whatever they're doing. One that needs you says **Needs attention**, and one that finished while you looked away says **Done** until you've opened it. Under each is what it runs to help it, each with a dot for what it's doing: the subagents Claude Code runs, while they work (Claude reports each to herdr-core, its task as its name; a click shows the thread they run in), and terminals the thread's agent started with herdr-core's `hc run` (a click peeks at one). Click a thread to open it, or the eye on hover to peek at it. **Next Agent** and **Previous Agent**, in the **View** menu and the command palette, go to the next agent and the one before, the ones that need you first, then the ones done while you looked away, then the ones working, panel open or not. In the sidebar, a thread running some shows how many beside its age; hover for their names. The notes, Linear, the changes, the links and the agents take turns on the right, as Herdr's Files and Changes do; `⌘R` hides whichever shows, or brings back the one that showed last (the notes, at first).

**herdr-core updates:** at launch Lemur asks herdr-core's release site (`releases.herdr.dev`) for the latest herdr-core, and says so when this Mac or a host runs an older one. **Lemur → Update herdr-core**, the command palette's **Update herdr-core**, or **Update** on Settings' **herdr-core** page (which lists each version) brings them up to it, as Herdr does: `hc update` here first, then `hc machine update` for each host behind. Terminals keep running through it. One installed by a package manager is left to it.

**Changes:** **Changes** at the top right lists the files changed in the worktree on show since its last commit, staged or not, new files too, each with its lines added and removed, as Herdr's Changes panel does. Click one to see its diff (a new file's lines all count as added), and again to hide it. It scrolls both ways, and the button above it shows it over the window, nearly as wide and tall, scrolling too; **×**, `⌘W`, Esc or a click outside closes that. Type a message under them and press Enter or **Commit** to commit the files listed (`git add` for each with changes not staged, then `git commit`, which takes what's staged as it is, removals and renames too). With a file in conflict it says so and waits: committing would commit the conflict markers. Other repositories inside the worktree, such as worktrees kept in the project's folder, aren't listed or committed: git would commit a link to them, not their files. It follows the thread on show, and looks again every 10 seconds.

**In Herdr:** a thread Lemur starts shows in Herdr too, when both use the same herdr-core: a tab of its own in its worktree's space, which Herdr makes if it has none. They're the same terminal, so closing that tab in Herdr ends the thread here. Threads started before stay out of Herdr unless you label them yourself (`hc label t7 herdr.space=<id> herdr.tab=<id>`).

**Links:** **Links** at the top right lists the worktree on show's pull requests and Claude artifacts (outside every worktree, the thread's folder's). Click one to open it in your browser; its **×** takes it off the list for good, and leaves what it links to alone. The list is kept in `~/.lemur/links.json`.

- **Pull requests** each show their state (Open, Draft, Merged or Closed), number and title, the open ones first: those of the worktree's branch, whoever made them, and those agents linked to, of the repository of its `origin` only, not another project's they mention. They come from GitHub's [`gh`](https://cli.github.com) on this Mac, signed in with `gh auth login`; hosts don't need it. A pull request made from a fork, in a clone whose `origin` is the fork, doesn't show as the branch's.
- **Artifacts** are the links to Claude artifacts that agents used or posted, the latest first, under the name the agent gave a link, if it did. Its pencil gives it a name of your own, which stays whatever agents call it; emptied, theirs shows again. A link you pasted shows once an agent uses it.

Links come from Claude Code's session logs on the host (`~/.claude/projects`), not yet Codex's, every session run in that worktree, whether Lemur started it or not, and only from what agents wrote. While the panel shows, Lemur reads the logs and asks `gh` every 30 seconds, after the first look only the logs written since the last time.

**Linear:** **Linear** at the top right opens a panel for the Linear issue a thread works on. **Sign in with Linear** opens Linear in your browser to approve Lemur's Linear app; Lemur only reads, and keeps the sign-in in your Keychain where only Lemur can read it: any other program asking for it gets your Mac's password prompt, and so does a Lemur build macOS hasn't seen yet, once (**Always Allow**). Settings → **Linear** signs out. The panel lists the issues assigned to you that aren't done; type to search them all, or paste an issue's key (`LAFT-3184`) or link. Click one to pin it to the thread on show. The pinned issue shows in the panel, with its state, title and description: **Send to agent** pastes its link at the thread's prompt (Enter is yours), **Open in Linear** opens it, and **Unpin** unpins it. Pasting a link to an issue into a thread (`⌘V`) pins that issue, unless one is pinned already. The issue's key shows on the thread's row, its tab and in the Agents panel. herdr-core keeps the pin as the terminal's `lemur.issue` label, so an agent can pin one itself: `hc label "$HC_TERMINAL_ID" lemur.issue=LAFT-3184`. Lemur asks Linear who it's signed in as when it starts, and otherwise only while the panel shows, at most every 2 minutes for the same issues.

**Notes:** every worktree has a notepad. **Notes** at the top right, or `⌘R` when the notes showed last on the right (as they do at first), opens it beside the terminal, on the worktree of the thread you opened last; **Notes** in a worktree's **+** menu (on hover) or right-click menu opens that one's. Type straight into it: it's Markdown, highlighted as you go. `⌘`-click a URL to open it; holding `⌘` underlines it first. Pasting a URL over selected text makes it a link, `[text](url)`, and so does `⌘K`, with the URL on the clipboard; `⌘K` in a link selects its URL. The two buttons at the top of the notes, or `⇧⌘L` for the checklist, make the cursor's line, or the selected lines, a bullet list or a checklist, and plain lines again when they are one already. `- [ ] ` starts an item with a box: `⌘`-click the box, or press `⌘↩` or `⇧⌘U` on its line, to tick it, and again to untick it; ticked items fade and are struck through. Enter at the end of a list item starts the next, and Enter on an empty one ends the list; Tab and `⇧Tab` indent and outdent an item, and `⌃⌘↑` and `⌃⌘↓` move it, with its nested items. These are Apple Notes' keys. Notes are saved as you type, on this Mac, in `~/.lemur/notes/<host>/<worktree path>.md`.

**Dictation:** `⌘D` opens a bar under the terminal and listens: what you say shows there as you say it, transcribed on your Mac by macOS's own speech recognition, so no audio leaves it. `⌘D` again stops listening; the text is then yours to correct, and `⌘D` once more adds to it. `⌘⏎` or **Paste** pastes it into the thread on show without sending it, and closes the bar; pressed while Lemur listens, it waits for your last words. The bar stays while you switch threads, so you can dictate first and pick the thread after: open it, then `⌘⏎`. Esc or **×** throws the text away. The button named after the language switches between English and Norwegian Bokmål from the next recording on, as Settings does. The first time, macOS asks whether Lemur may use the microphone. Dictation needs macOS 26 or newer.

**Images:** drop files on the terminal, or `⌘V` with an image or copied files on the clipboard. Lemur uploads them to the host's `upload_dir` and pastes the remote paths. Text pastes as text.

**Links:** hold `⌘` over a link in the terminal to underline it, and `⌘`-click to open it in your browser. That's an `http(s)://` URL written out in the text, or a link whose text hides its URL (OSC 8). A URL runs on across rows the terminal wrapped, but not across lines Claude Code broke itself, which a long one in its replies may be.

A file path written out in the terminal, like `src/main.rs:42` or `/etc/hosts`, is a link too: `⌘`-click it to copy the file from the host to a temporary folder on your Mac and preview it with Quick Look, which never runs it. A relative path is from the session's current folder; a `:line` after it is left out.

**Scrolling and selection:** the wheel scrolls back through what the terminal showed; opening a thread brings its last 2000 lines from herdr-core. Drag to select, double-click for a word and triple-click for a line, a drag off the terminal too; `⌘C` copies it. A program that asks for the mouse gets the clicks and the wheel instead; hold `⇧` to select anyway. Text a program copies (OSC 52) goes on your Mac clipboard.

CLI: `lemur ls`, `lemur new <base> <name> [--branch <branch>] [--from <branch>]` (a new worktree; detached without `--branch`) or `lemur new <base> <name> --worktree <path> [--shell]`, `lemur attach <host> <session>` (herdr-core's own attach; `ctrl+\` detaches), `lemur rename <host> <session> <name>`, `lemur upload <files>...`, `lemur paste` (clipboard image to the last session Lemur showed). A session is its terminal's address in herdr-core, as `lemur ls` prints it, like `agent/t7`.

### Status dots

Each session's dot comes from the agent itself: Claude Code and Codex (recent versions) report what they're doing to the terminal they run in, and herdr-core tells Lemur at once. Where Claude doesn't, Lemur runs `claude agents --json` on the host, at most every 30 seconds, which lists each running Claude with its process and status, and Lemur matches the process to its terminal; that's also where Lemur learns each Claude's conversation, to resume it. No setup needed; `claude agents` needs Claude Code 2.1.139 or later, on `PATH` or in `~/.local/bin`.

- **Green, breathing:** Claude is working. It breathes only while Lemur is the active window, as each breath redraws it; otherwise it's a still green.
- **Yellow:** Claude needs input (a question or permission prompt); hover the dot for what it's asking.
- **Blue:** it finished while you were looking at another session; opening it clears that.
- **Grey:** idle, or no Claude running.

### Notifications

When a session finishes or starts needing input, a popup appears at the top-right of the main screen, like Zed's, unless it's the session you're looking at in the active window. It names the project and worktree under the session's name. **Show** brings Lemur forward and opens that session; **Dismiss** closes the popup. Opening the session from the sidebar closes its popup too, and so does coming back to Lemur while it's the session on show. A popup never keeps the keyboard: if `⌘Tab` back to Lemur hands it to one, it goes on to the main window.

To stop a thread's notifications, right-click it or its tab → **Silence notifications**, or use the bell on hover in the agents panel; **Allow notifications** lets them come again. It makes no more popups, and one it already made goes. Its dot and **Needs attention** go on saying what it does, and a crossed-out bell marks it, in the sidebar and the panel. herdr-core keeps it as a label on the terminal (`lemur.silent`), so it stays silent across restarts and from any Mac; a thread resumed in a new terminal starts over.

What Claude reports arrives at once. The refresh, for worktrees, servers and hosts' load, runs every 3 seconds while Lemur is in front, and every 15 seconds while it's in the background; `claude agents`, and the servers listening (`lsof` on a Mac), run with it at most every 30 seconds, as they cost each host a moment of CPU.

## Trademarks

Lemur is an independent project, not affiliated with or endorsed by Anthropic or OpenAI. Claude, Claude Code and the Claude logo are trademarks of Anthropic. Codex and OpenAI are trademarks of OpenAI.
