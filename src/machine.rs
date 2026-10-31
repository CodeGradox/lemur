//! What a host is up to besides its terminals: how busy it is (CPU,
//! memory, load), and the servers listening on it, each with the folder it
//! runs in, so a worktree's dev server can be found from the worktree. Read
//! by the same script that lists the host's worktrees.

/// Reads the load (Linux's /proc only), then the servers listening (`ss`
/// and /proc on Linux, `lsof` on macOS): port, address, pid, program and
/// folder, a tab between each. Herdr's projects mod reads them the same way.
pub const SCRIPT: &str = r#"echo '@@load'
if [ -r /proc/stat ]; then
  head -1 /proc/stat
  grep -E '^(MemTotal|MemAvailable|SwapTotal|SwapFree):' /proc/meminfo
  cat /proc/loadavg
  nproc 2>/dev/null
fi
echo '@@ports'
if [ "$(uname -s)" = Linux ]; then
  ss -ltnpH 2>/dev/null | while read -r st rq sq local peer users; do
    [ -n "$users" ] || continue
    for pid in $(printf '%s' "$users" | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u); do
      printf '%s\t%s\t%s\t%s\t%s\n' "${local##*:}" "${local%:*}" "$pid" "$(cat /proc/$pid/comm 2>/dev/null)" "$(readlink /proc/$pid/cwd 2>/dev/null)"
    done
  done
else
  L=$(lsof -nP -iTCP -sTCP:LISTEN -Fpcn 2>/dev/null | awk '/^p/{p=substr($0,2)} /^c/{c=substr($0,2)} /^n/{a=substr($0,2); port=a; sub(/.*:/,"",port); addr=a; sub(/:[^:]*$/,"",addr); print port "\t" addr "\t" p "\t" c}' | sort -u)
  P=$(printf '%s\n' "$L" | cut -f3 | sort -u | paste -sd, -)
  C=""
  [ -n "$P" ] && C=$(lsof -a -d cwd -p "$P" -Fpn 2>/dev/null | awk '/^p/{p=substr($0,2)} /^n/{print p "\t" substr($0,2)}')
  { printf '%s\n' "$C" | awk '{print "C\t" $0}'; printf '%s\n' "$L" | awk '{print "L\t" $0}'; } |
    awk -F'\t' -v OFS='\t' '$1=="C"{cwd[$2]=$3; next} $1=="L" && $2!=""{print $2,$3,$4,$5,cwd[$4]}'
fi
"#;

/// Asks the host's name on its tailnet, which reaches its servers from
/// here (`agents` in `http://agents:3020`). A host without Tailscale has
/// none, and that fails nothing else.
pub const TAILNET: &str = "echo '@@tailnet'\ntailscale status --self --json 2>/dev/null || true\n";

/// How busy a host was when read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Load {
    /// /proc/stat's CPU counters since boot: those busy, and all.
    pub busy: u64,
    pub total: u64,
    pub mem_total_kb: u64,
    pub mem_used_kb: u64,
    pub swap_total_kb: u64,
    pub swap_used_kb: u64,
    /// The 1, 5 and 15 minute load averages.
    pub load: Vec<String>,
    pub cores: Option<u32>,
}

impl Load {
    /// The share of the CPU busy since `before`, in percent.
    pub fn cpu_since(&self, before: &Load) -> Option<f32> {
        let total = self.total.checked_sub(before.total).filter(|t| *t > 0)?;
        let busy = self.busy.saturating_sub(before.busy);
        Some(100.0 * busy as f32 / total as f32)
    }

    /// Memory in use, in percent.
    pub fn memory(&self) -> Option<f32> {
        (self.mem_total_kb > 0).then(|| 100.0 * self.mem_used_kb as f32 / self.mem_total_kb as f32)
    }
}

/// A server listening on a host.
#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    pub port: u16,
    /// Where it listens: `127.0.0.1` can't be reached from another host.
    pub addr: String,
    /// Its program's name, and the folder it runs in.
    pub name: String,
    pub cwd: String,
}

impl Server {
    /// Listens on the host's own loopback only.
    pub fn local_only(&self) -> bool {
        let addr = self.addr.trim_start_matches('[').trim_end_matches(']');
        addr.starts_with("127.") || addr == "::1" || addr == "localhost"
    }
}

/// What the script's `@@load`, `@@ports` and `@@tailnet` sections say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    /// None without /proc, as on macOS.
    pub load: Option<Load>,
    pub servers: Vec<Server>,
    /// The host's name on its tailnet, when asked and it has one.
    pub tailnet: Option<String>,
}

/// Feeds one line of the script's output in; `section` is the `@@` line
/// it's under.
pub fn parse_line(reading: &mut Reading, section: &str, line: &str) {
    match section {
        "load" => parse_load(reading.load.get_or_insert_with(Load::default), line),
        "ports" => reading.servers.extend(parse_server(line)),
        "tailnet" => {
            // JSON across lines; the name is in `"DNSName": "agents.tail1234.ts.net."`.
            if reading.tailnet.is_none()
                && let Some(rest) = line.trim().strip_prefix("\"DNSName\":")
            {
                let name = rest.trim().trim_matches([',', '"']).split('.').next();
                reading.tailnet = name.filter(|n| !n.is_empty()).map(str::to_string);
            }
        }
        _ => {}
    }
}

fn parse_load(load: &mut Load, line: &str) {
    let kb = |rest: &str| -> u64 {
        rest.split_whitespace()
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    if let Some(rest) = line.strip_prefix("cpu ") {
        // user nice system idle iowait irq softirq steal
        let n: Vec<u64> = rest
            .split_whitespace()
            .take(8)
            .filter_map(|v| v.parse().ok())
            .collect();
        load.total = n.iter().sum();
        load.busy = load.total - n.get(3).unwrap_or(&0) - n.get(4).unwrap_or(&0);
    } else if let Some(rest) = line.strip_prefix("MemTotal:") {
        load.mem_total_kb = kb(rest);
    } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
        load.mem_used_kb = load.mem_total_kb.saturating_sub(kb(rest));
    } else if let Some(rest) = line.strip_prefix("SwapTotal:") {
        load.swap_total_kb = kb(rest);
    } else if let Some(rest) = line.strip_prefix("SwapFree:") {
        load.swap_used_kb = load.swap_total_kb.saturating_sub(kb(rest));
    } else if line.split_whitespace().count() >= 3 && line.contains('.') && line.contains('/') {
        load.load = line
            .split_whitespace()
            .take(3)
            .map(str::to_string)
            .collect();
    } else if let Ok(cores) = line.trim().parse() {
        load.cores = Some(cores);
    }
}

fn parse_server(line: &str) -> Option<Server> {
    let mut f = line.split('\t');
    let port = f.next()?.trim().parse().ok().filter(|p| *p > 0)?;
    let addr = f.next().unwrap_or_default().to_string();
    let _pid = f.next();
    Some(Server {
        port,
        addr,
        name: f.next().unwrap_or_default().to_string(),
        cwd: f.next().unwrap_or_default().to_string(),
    })
}

/// The servers whose folder is in `worktree` and in none of `deeper`
/// (worktrees inside it, as `.claude/worktrees/` sits in the main
/// checkout), by port, each port once.
pub fn servers_in<'a>(servers: &'a [Server], worktree: &str, deeper: &[&str]) -> Vec<&'a Server> {
    let inside = |path: &str, dir: &str| {
        path.strip_prefix(dir)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    let mut found: Vec<&Server> = servers
        .iter()
        // A deleted folder keeps no worktree's servers.
        .filter(|s| !s.cwd.is_empty() && !s.cwd.ends_with(" (deleted)"))
        .filter(|s| inside(&s.cwd, worktree) && !deeper.iter().any(|d| inside(&s.cwd, d)))
        .collect();
    found.sort_by_key(|s| s.port);
    found.dedup_by_key(|s| s.port);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(out: &str) -> Reading {
        let mut reading = Reading::default();
        let mut section = "";
        for line in out.lines() {
            match line.strip_prefix("@@") {
                Some(s) => section = s,
                None => parse_line(&mut reading, section, line),
            }
        }
        reading
    }

    #[test]
    fn reads_linux_load_and_servers() {
        let r = read(
            "@@load\n\
             cpu  100 0 50 800 50 0 0 0 0 0\n\
             MemTotal:       32000000 kB\n\
             MemAvailable:   12000000 kB\n\
             SwapTotal:       2000000 kB\n\
             SwapFree:        1500000 kB\n\
             0.23 0.30 0.40 1/500 1234\n\
             8\n\
             @@ports\n\
             3020\t0.0.0.0\t41\truby\t/srv/api/.claude/worktrees/auth\n\
             5432\t127.0.0.1\t42\tpostgres\t/var/lib/postgresql\n\
             3021\t[::]\t43\tnode\t/srv/api\n\
             @@tailnet\n\
             {\n  \"Self\": {\n    \"DNSName\": \"agents.tail1234.ts.net.\",\n    \"HostName\": \"agent\"\n  }\n}\n",
        );
        let load = r.load.expect("a load");
        assert_eq!((load.busy, load.total), (150, 1000));
        assert_eq!(
            (load.mem_total_kb, load.mem_used_kb),
            (32_000_000, 20_000_000)
        );
        assert_eq!(load.swap_used_kb, 500_000);
        assert_eq!(load.load, ["0.23", "0.30", "0.40"]);
        assert_eq!(load.cores, Some(8));
        assert_eq!(load.memory(), Some(62.5));
        let later = Load {
            busy: 250,
            total: 1400,
            ..load.clone()
        };
        assert_eq!(later.cpu_since(&load), Some(25.0));
        assert_eq!(load.cpu_since(&load), None, "no time between");
        assert_eq!(r.servers.len(), 3);
        assert!(r.servers[1].local_only() && !r.servers[0].local_only());
        assert!(!r.servers[2].local_only());
        assert_eq!(r.tailnet.as_deref(), Some("agents"));
    }

    /// macOS has no /proc: no load, servers still.
    #[test]
    fn no_proc_no_load() {
        let r = read("@@load\n@@ports\n8080\t*\t7\tnode\t/Users/me/app\n");
        assert_eq!(r.load, None);
        assert_eq!(r.servers[0].port, 8080);
    }

    #[test]
    fn a_worktrees_servers_are_those_in_its_folder() {
        let server = |port, cwd: &str| Server {
            port,
            addr: "0.0.0.0".into(),
            name: "ruby".into(),
            cwd: cwd.into(),
        };
        let servers = [
            server(3021, "/srv/api/.claude/worktrees/auth/web"),
            server(3020, "/srv/api/.claude/worktrees/auth"),
            server(3020, "/srv/api/.claude/worktrees/auth/tmp"),
            server(3000, "/srv/api"),
            server(4000, "/srv/api/.claude/worktrees/gone (deleted)"),
            server(9000, "/srv/apis"),
        ];
        let ports = |wt: &str, deeper: &[&str]| -> Vec<u16> {
            servers_in(&servers, wt, deeper)
                .iter()
                .map(|s| s.port)
                .collect()
        };
        let auth = "/srv/api/.claude/worktrees/auth";
        assert_eq!(ports(auth, &[]), [3020, 3021]);
        assert_eq!(ports("/srv/api", &[auth]), [3000]);
    }

    /// The script runs, here too, and succeeds without Tailscale.
    #[test]
    fn the_script_runs() {
        let out = std::process::Command::new("sh")
            .args(["-c", &format!("{SCRIPT}{TAILNET}")])
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.starts_with("@@load\n") && text.contains("@@ports\n"),
            "{text}"
        );
    }
}
