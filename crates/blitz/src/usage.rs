//! What Claude Code's sessions on this PC would cost at API prices, read
//! from the logs Claude Code keeps in `~/.claude/projects`, by the week the
//! user's plan limits reset on. Nothing is read while `usage_tracking` is
//! off, and nothing is kept on disk.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::hook::Json;

/// The weekly reset: the day, 0 for Monday, and the hour, local time.
pub type Reset = (u8, u8);

/// The time now in Unix seconds; 0 if the clock is before 1970.
pub fn now() -> i64 {
    secs(SystemTime::now())
}

fn secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// How often the logs are read again while tracking is on.
const EVERY: Duration = Duration::from_secs(60);
/// Folders below `projects` a log can be in, as deep as
/// `<project>/<session>/subagents/workflows/wf_<id>/agent-<id>.jsonl`.
const DEPTH: usize = 6;
const DAY: i64 = 86_400;
const WEEK: i64 = 7 * DAY;
/// How long a claude.ai usage window lasts.
const WINDOW: i64 = 5 * 3600;
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Dollars per million tokens at Claude API list prices: input, output
/// and cache reads, the newest model of each family first. Cache writes
/// cost 1.25 times input for five minutes and twice input for an hour.
#[rustfmt::skip]
const PRICES: &[(&str, f64, f64, f64)] = &[
    ("claude-fable-5-1", 10.0, 50.0, 0.25), ("claude-fable-5", 10.0, 50.0, 1.0),
    ("claude-mythos-5-1", 10.0, 50.0, 0.25), ("claude-mythos-5", 10.0, 50.0, 1.0),
    ("claude-opus-5-5", 4.0, 20.0, 0.2), ("claude-opus-5", 5.0, 25.0, 0.5),
    ("claude-opus-4-8", 5.0, 25.0, 0.5), ("claude-opus-4-7", 5.0, 25.0, 0.5),
    ("claude-opus-4-6", 5.0, 25.0, 0.5), ("claude-opus-4-5", 5.0, 25.0, 0.5),
    ("claude-opus-4-1", 15.0, 75.0, 1.5), ("claude-opus-4", 15.0, 75.0, 1.5),
    ("claude-sonnet-5-5", 2.0, 10.0, 0.1), ("claude-sonnet-5", 2.0, 10.0, 0.2),
    ("claude-sonnet-4-6", 3.0, 15.0, 0.3), ("claude-sonnet-4-5", 3.0, 15.0, 0.3),
    ("claude-sonnet-4", 3.0, 15.0, 0.3),
    ("claude-haiku-5-5", 0.1, 0.5, 0.01), ("claude-haiku-4-5", 1.0, 5.0, 0.1),
    ("claude-3-5-haiku", 0.8, 4.0, 0.08),
];
/// Haiku 5.5 charges five times as much for a whole request whose prompt,
/// cache reads and writes included, is over 100,000 tokens.
const LONG: (&str, u64, f64) = ("claude-haiku-5-5", 100_000, 5.0);
/// Dollars per web search.
const SEARCH: f64 = 0.01;

/// Input, output and cache read prices, dollars per million tokens.
type Price = (f64, f64, f64);

/// One response, counted once.
struct Row {
    key: u64,
    at: i64,
    /// Indices into [`Tracker::models`] and [`Tracker::projects`].
    model: u32,
    project: u32,
    /// A subagent's or a workflow agent's.
    agent: bool,
    /// Input, output, 5-minute cache writes, 1-hour cache writes and cache
    /// reads.
    tokens: [u32; 5],
    fast: bool,
    us: bool,
    searches: u16,
}

/// One response as a log line records it.
#[derive(Debug, PartialEq)]
struct Parsed {
    key: u64,
    at: i64,
    model: String,
    cwd: String,
    agent: bool,
    tokens: [u32; 5],
    fast: bool,
    us: bool,
    searches: u16,
}

/// What some responses would cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spend {
    pub cost: f64,
    /// What the cache reads cost less than the same tokens sent as input.
    pub saved: f64,
    pub requests: u32,
    /// Tokens and their cost by kind: input, output, cache writes and
    /// cache reads.
    pub tokens: [(u64, f64); 4],
}

impl Spend {
    fn add(&mut self, s: &Spend) {
        self.cost += s.cost;
        self.saved += s.saved;
        self.requests += s.requests;
        for (a, b) in self.tokens.iter_mut().zip(s.tokens) {
            (a.0, a.1) = (a.0 + b.0, a.1 + b.1);
        }
    }
}

/// What the logs add up to for one week.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// The logs folder is there.
    pub found: bool,
    pub folder: String,
    /// This week's start and the next reset, Unix seconds.
    pub week: (i64, i64),
    /// The start as the clock shows it, as `Thu 5 pm`.
    pub since: String,
    pub this: Spend,
    /// Last week's cost.
    pub last: f64,
    /// Each local day from the start to today, as `Thu 29`, and its cost.
    pub days: Vec<(String, f64)>,
    /// Each model used this week, as people say it, with its cost, `None`
    /// without a price, and whether that price was guessed from its
    /// family's newest; the costliest first.
    pub models: Vec<(String, Option<f64>, bool)>,
    /// Each project's name and cost, the costliest first.
    pub projects: Vec<(String, f64)>,
    /// What subagents and workflow agents spent.
    pub agents: f64,
    /// The 5-hour window under way: its end, as `7:45 pm`, and its cost.
    pub window: Option<(String, f64)>,
    /// Requests this week by a model with no price.
    pub unpriced: u32,
}

/// Tracking while it is on: the thread that reads the logs for the week
/// `reset` begins, and what it last found.
pub struct Tracking {
    ask: mpsc::Sender<Reset>,
    pub reset: Reset,
    pub report: Option<Box<Report>>,
}

impl Tracking {
    /// Starts reading Claude Code's logs on a thread of its own and hands
    /// `send` what it found: at once, then every minute. The thread ends
    /// when this is dropped or `send` returns false.
    pub fn start(reset: Reset, send: impl Fn(Report) -> bool + Send + 'static) -> Tracking {
        let (ask, asked) = mpsc::channel();
        std::thread::spawn(move || {
            let root = crate::hook::claude_dir().map(|d| d.join("projects"));
            let off = &crate::desktop::utc_offset;
            let (mut t, mut reset) = (Tracker::new(root.clone()), reset);
            loop {
                let now = now();
                let since = week_start(week_start(now, reset, off) - 1, reset, off);
                // A reset moved earlier needs rows that were let go.
                if since < t.since {
                    t = Tracker::new(root.clone());
                }
                t.read(since);
                if !send(t.report(now, reset, off)) {
                    return;
                }
                match asked.recv_timeout(EVERY) {
                    Ok(r) => reset = r,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                // A burst of asks costs one read.
                while let Ok(r) = asked.try_recv() {
                    reset = r;
                }
            }
        });
        Tracking {
            ask,
            reset,
            report: None,
        }
    }

    /// Reads again now, for the week `reset` begins; another week drops
    /// the last report.
    pub fn ask(&mut self, reset: Reset) {
        if reset != self.reset {
            self.report = None;
        }
        self.reset = reset;
        let _ = self.ask.send(reset);
    }
}

/// The responses read so far, and how far each log was read.
struct Tracker {
    /// `projects` in Claude Code's folder.
    root: Option<PathBuf>,
    /// The bytes read of each log: the end of its last whole line.
    files: HashMap<PathBuf, u64>,
    rows: Vec<Row>,
    /// Each row's index, by key.
    index: HashMap<u64, u32>,
    /// Model ids, which rows point into.
    models: Vec<String>,
    /// Folders under `root`, each with its name and whether that came from
    /// a session's own row.
    projects: Vec<(String, String, bool)>,
    /// Rows before this are not kept.
    since: i64,
}

impl Tracker {
    fn new(root: Option<PathBuf>) -> Tracker {
        Tracker {
            root,
            files: HashMap::new(),
            rows: Vec::new(),
            index: HashMap::new(),
            models: Vec::new(),
            projects: Vec::new(),
            since: i64::MIN,
        }
    }

    /// Reads what was added to the logs since the last read, keeping the
    /// responses from `since` on.
    // ponytail: the first read takes in two weeks of logs whole, a few
    // seconds of one core for 1.5 GB; keep offsets and rows in a cache file
    // if starts feel it.
    fn read(&mut self, since: i64) {
        if since > self.since {
            self.rows.retain(|r| r.at >= since);
            self.index = (self.rows.iter().enumerate())
                .map(|(i, r)| (r.key, i as u32))
                .collect();
            self.since = since;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        // Only the logs still there, so deleted ones drop out.
        let mut files = HashMap::new();
        let mut line = Vec::new();
        let mut dirs = vec![(root.clone(), 0)];
        while let Some((dir, depth)) = dirs.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                // Not followed through a link, so links are never walked.
                let Ok(m) = e.metadata() else {
                    continue;
                };
                let path = e.path();
                if m.is_dir() && depth < DEPTH {
                    dirs.push((path, depth + 1));
                    continue;
                }
                // Copies named `.orphaned-` are read too, and their
                // responses counted once.
                if !m.is_file() || path.extension().is_none_or(|x| x != "jsonl") {
                    continue;
                }
                let mut off = self.files.get(&path).copied().unwrap_or(0);
                // Shorter than what was read: rewritten. Keys keep the rows
                // read before from counting twice.
                if m.len() < off {
                    off = 0;
                }
                let old = m.modified().map_or(0, secs) < since;
                if m.len() != off && !(off == 0 && old) {
                    let project = self.project(&root, &path);
                    off = self.read_from(&path, off, project, since, &mut line);
                }
                if off > 0 || !old {
                    files.insert(path, off);
                }
            }
        }
        self.files = files;
    }

    /// Reads the whole lines of log `path` from byte `off` on, and returns
    /// where the last one ends. A line without its newline is one Claude
    /// Code is still writing, read next time.
    fn read_from(
        &mut self,
        path: &Path,
        mut off: u64,
        project: u32,
        since: i64,
        line: &mut Vec<u8>,
    ) -> u64 {
        let Ok(mut f) = std::fs::File::open(path) else {
            return off;
        };
        if f.seek(SeekFrom::Start(off)).is_err() {
            return off;
        }
        let mut r = BufReader::with_capacity(1 << 16, f);
        loop {
            line.clear();
            match r.read_until(b'\n', line) {
                Ok(_) if line.ends_with(b"\n") => {}
                _ => return off,
            }
            off += line.len() as u64;
            let parsed = std::str::from_utf8(line).ok().and_then(parse);
            if let Some(p) = parsed.filter(|p| p.at >= since) {
                self.add(project, p);
            }
        }
    }

    /// The index of the project log `path` is in: the folder under `root`.
    fn project(&mut self, root: &Path, path: &Path) -> u32 {
        let folder = (path.strip_prefix(root).ok())
            .and_then(|p| p.components().next())
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        match self.projects.iter().position(|p| p.0 == folder) {
            Some(i) => i as u32,
            None => {
                self.projects.push((folder, String::new(), false));
                (self.projects.len() - 1) as u32
            }
        }
    }

    /// Counts response `p` from a log of project `project`, once however
    /// many copies of it the logs hold.
    fn add(&mut self, project: u32, p: Parsed) {
        let model = match self.models.iter().position(|m| *m == p.model) {
            Some(i) => i as u32,
            None => {
                self.models.push(p.model);
                (self.models.len() - 1) as u32
            }
        };
        // Named from a session's own row: workflow agents run in worktrees,
        // whose folders would scatter one project into many names.
        if let Some(pr) = self.projects.get_mut(project as usize)
            && !p.cwd.is_empty()
            && (pr.1.is_empty() || (!p.agent && !pr.2))
        {
            pr.1 = crate::render::chrome::folder_name(&p.cwd);
            pr.2 = !p.agent;
        }
        let row = Row {
            key: p.key,
            at: p.at,
            model,
            project,
            agent: p.agent,
            tokens: p.tokens,
            fast: p.fast,
            us: p.us,
            searches: p.searches,
        };
        // As ccusage does: the session's own copy, then the largest, which
        // holds the last content block's final output count.
        let rank = |r: &Row| {
            (
                !r.agent,
                r.tokens.iter().map(|&n| u64::from(n)).sum::<u64>(),
            )
        };
        match self.index.get(&p.key) {
            Some(&i) => {
                if let Some(old) = self.rows.get_mut(i as usize)
                    && rank(&row) > rank(old)
                {
                    *old = row;
                }
            }
            None => {
                self.index.insert(p.key, self.rows.len() as u32);
                self.rows.push(row);
            }
        }
    }

    /// What the rows add up to in the week holding `now`, by a clock `off`
    /// gives the offset from UTC of.
    fn report(&self, now: i64, reset: Reset, off: &dyn Fn(i64) -> i64) -> Report {
        let this = week_start(now, reset, off);
        let last = week_start(this - 1, reset, off);
        let next = week_start(this + 8 * DAY, reset, off);
        let (first, today) = (
            (this + off(this)).div_euclid(DAY),
            (now + off(now)).div_euclid(DAY),
        );
        let bounds: Vec<i64> = (first..=today).map(|d| utc(d * DAY, off)).collect();
        let mut out = Report {
            found: self.root.as_ref().is_some_and(|r| r.is_dir()),
            folder: (self.root.as_ref()).map_or_else(String::new, |r| r.display().to_string()),
            week: (this, next),
            since: clock(this, off),
            days: (first..=today).map(|d| (day_label(d), 0.0)).collect(),
            ..Report::default()
        };
        let prices: Vec<Option<(Price, bool)>> = self.models.iter().map(|m| price(m)).collect();
        let cost_of = |r: &Row| {
            let (p, _) = prices[r.model as usize]?;
            Some(cost(r, p, long(&self.models[r.model as usize], r)))
        };
        // What each model and project spent this week; `None` for one with
        // no request.
        let mut models: Vec<Option<f64>> = vec![None; self.models.len()];
        let mut projects: Vec<Option<f64>> = vec![None; self.projects.len()];
        for r in &self.rows {
            let spend = cost_of(r);
            let c = spend.as_ref().map(|s| s.cost);
            if (last..this).contains(&r.at) {
                out.last += c.unwrap_or(0.0);
            }
            if !(this..=now).contains(&r.at) {
                continue;
            }
            match &spend {
                Some(s) => out.this.add(s),
                None => out.unpriced += 1,
            }
            let c = c.unwrap_or(0.0);
            let day = bounds.partition_point(|&b| b <= r.at).saturating_sub(1);
            if let Some(d) = out.days.get_mut(day) {
                d.1 += c;
            }
            let spent = [
                models.get_mut(r.model as usize),
                projects.get_mut(r.project as usize),
            ];
            for s in spent.into_iter().flatten() {
                *s = Some(s.unwrap_or(0.0) + c);
            }
            if r.agent {
                out.agents += c;
            }
        }
        out.models = (models.iter().zip(&self.models).zip(&prices))
            .filter_map(|((c, id), p)| {
                let c = (*c)?;
                Some((model_name(id), p.map(|_| c), p.is_some_and(|p| p.1)))
            })
            .collect();
        out.models.sort_by(|a, b| {
            let c = |m: &(String, Option<f64>, bool)| m.1.unwrap_or(-1.0);
            c(b).total_cmp(&c(a)).then_with(|| a.0.cmp(&b.0))
        });
        // Folders that come to one name are one project.
        let mut named: Vec<(String, f64)> = Vec::new();
        for (c, (folder, name, _)) in projects.into_iter().zip(&self.projects) {
            let Some(c) = c else {
                continue;
            };
            let name = if name.is_empty() { folder } else { name };
            match named.iter_mut().find(|n| n.0 == *name) {
                Some(n) => n.1 += c,
                None => named.push((name.clone(), c)),
            }
        }
        named.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.projects = named;
        let mut times: Vec<i64> = (self.rows.iter())
            .map(|r| r.at)
            .filter(|&t| t <= now)
            .collect();
        times.sort_unstable();
        out.window = window(&times, now).map(|start| {
            let mut spent = 0.0;
            for r in self.rows.iter().filter(|r| (start..=now).contains(&r.at)) {
                spent += cost_of(r).map_or(0.0, |s| s.cost);
            }
            let end = start + WINDOW;
            (clock_time((end + off(end)).rem_euclid(DAY)), spent)
        });
        out
    }
}

/// The response a log line records: an assistant row with usage, an id, a
/// time and a model. `None` for any other line.
fn parse(line: &str) -> Option<Parsed> {
    // Most lines are not responses; this is far quicker than parsing them.
    if !line.contains("\"usage\":") {
        return None;
    }
    let j = Json::parse(line.trim_end_matches(['\n', '\r']))?;
    if j.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let m = j.get("message")?;
    let u = m.get("usage")?;
    let id = m.get("id")?.as_str()?;
    let at = unix(j.get("timestamp")?.as_str()?)?;
    let model = model_id(m.get("model")?.as_str()?)?;
    let request = j.get("requestId").and_then(Json::as_str).unwrap_or("");
    let n = |k: &str| num(u.get(k));
    let writes = n("cache_creation_input_tokens");
    let hour = num(u
        .get("cache_creation")
        .and_then(|c| c.get("ephemeral_1h_input_tokens")));
    // Thinking and the iterations of a turn are in output_tokens already.
    let tokens = [
        n("input_tokens"),
        n("output_tokens"),
        writes.saturating_sub(hour),
        hour,
        n("cache_read_input_tokens"),
    ];
    if tokens == [0; 5] {
        return None;
    }
    let text = |v: Option<&Json>, want: &str| v.and_then(Json::as_str) == Some(want);
    let searches = num(u
        .get("server_tool_use")
        .and_then(|s| s.get("web_search_requests")));
    Some(Parsed {
        key: key(id, request),
        at,
        model,
        cwd: j.get("cwd").and_then(Json::as_str).unwrap_or("").into(),
        agent: j.get("isSidechain") == Some(&Json::Bool(true)),
        tokens,
        fast: text(u.get("speed"), "fast"),
        us: text(u.get("inference_geo"), "us"),
        searches: u16::try_from(searches).unwrap_or(u16::MAX),
    })
}

/// What tells copies of one response apart from other responses. One
/// response is logged once per content block, and resumed or forked
/// sessions copy it into other files.
// ponytail: a 64-bit hash of the key; keep the strings if a collision ever
// shows.
fn key(id: &str, request: &str) -> u64 {
    let mut h = DefaultHasher::new();
    (id, request).hash(&mut h);
    h.finish()
}

/// A count in a log, 0 when it is not there.
fn num(v: Option<&Json>) -> u32 {
    match v {
        // Saturates, and a negative count or NaN is 0.
        Some(Json::Num(n)) => *n as u32,
        _ => 0,
    }
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `2026-10-29T14:03:22.123Z` as Unix seconds. Only UTC times are read.
fn unix(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    let n = |i: usize, len: usize| -> Option<i64> {
        let d = ts.get(i..i + len).filter(|d| digits(d))?;
        d.parse().ok()
    };
    let marks = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')];
    let frac = ts.get(19..)?.strip_suffix('Z')?;
    if !marks.iter().all(|&(i, c)| b.get(i) == Some(&c))
        || !(frac.is_empty() || frac.strip_prefix('.').is_some_and(digits))
    {
        return None;
    }
    let (y, mo, d) = (n(0, 4)?, n(5, 2)?, n(8, 2)?);
    let (h, mi, s) = (n(11, 2)?, n(14, 2)?, n(17, 2)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * DAY + h * 3600 + mi * 60 + s)
}

/// The days from 1970-01-01 to `y`-`m`-`d`, by Howard Hinnant's algorithm.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    // From March, so a leap day ends the year.
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01, as (year, month, day); the inverse of
/// [`days_from_civil`].
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// A model as the price table names it: `claude-sonnet-4-5` for
/// `us.anthropic.claude-sonnet-4-5-20250929-v1:0`, `claude-opus-5-5[1m]`
/// or `claude-sonnet-4-5@20250929`. `None` for Claude Code's own
/// `<synthetic>` rows.
fn model_id(raw: &str) -> Option<String> {
    let id = raw.find("claude-").map_or(raw, |i| &raw[i..]);
    let mut id = id.split(['[', '@']).next().unwrap_or("");
    if let Some((head, v)) = id.rsplit_once("-v") {
        let (major, minor) = v.split_once(':').unwrap_or((v, "0"));
        if digits(major) && digits(minor) {
            id = head;
        }
    }
    if let Some((head, date)) = id.rsplit_once('-')
        && date.len() == 8
        && digits(date)
    {
        id = head;
    }
    (!id.is_empty() && id != "<synthetic>").then(|| id.to_string())
}

/// The family of a Claude model: `opus` for `claude-opus-5-5`, `haiku` for
/// `claude-3-5-haiku`.
fn family(id: &str) -> Option<&str> {
    (id.strip_prefix("claude-")?.split('-'))
        .find(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphabetic()))
}

/// A model as people say it: `Opus 5.5` for `claude-opus-5-5`, `Haiku 3.5`
/// for `claude-3-5-haiku`. Any other id is shown as it is.
fn model_name(id: &str) -> String {
    let parts: Vec<&str> = id
        .strip_prefix("claude-")
        .map_or(Vec::new(), |r| r.split('-').collect());
    let nums: Vec<&str> = parts.iter().copied().filter(|p| digits(p)).collect();
    match family(id) {
        Some(f) if !nums.is_empty() && nums.len() + 1 == parts.len() => {
            format!("{}{} {}", f[..1].to_uppercase(), &f[1..], nums.join("."))
        }
        _ => id.into(),
    }
}

/// The price of model `id`, and whether it was guessed: a model the table
/// does not list costs what the newest of its family does. `None` for a
/// model of no family the table knows.
fn price(id: &str) -> Option<(Price, bool)> {
    let at = |p: &(&str, f64, f64, f64)| (p.1, p.2, p.3);
    if let Some(p) = PRICES.iter().find(|p| p.0 == id) {
        return Some((at(p), false));
    }
    let f = family(id)?;
    let p = PRICES.iter().find(|p| family(p.0) == Some(f))?;
    Some((at(p), true))
}

/// Whether response `r` of model `id` costs more for its long prompt.
fn long(id: &str, r: &Row) -> bool {
    let prompt: u64 = [0, 2, 3, 4].iter().map(|&i| u64::from(r.tokens[i])).sum();
    id == LONG.0 && prompt > LONG.1
}

/// What response `r` costs at price `p`, `long` when its prompt is over
/// [`LONG`]'s.
fn cost(r: &Row, (i, o, cr): Price, long: bool) -> Spend {
    let m = (if r.fast { 2.0 } else { 1.0 })
        * (if r.us { 1.1 } else { 1.0 })
        * (if long { LONG.2 } else { 1.0 });
    let t = r.tokens.map(f64::from);
    let kinds = [
        t[0] * i,
        t[1] * o,
        (t[2] * 1.25 + t[3] * 2.0) * i,
        t[4] * cr,
    ];
    let kinds = kinds.map(|c| m * c / 1e6);
    let n = r.tokens.map(u64::from);
    Spend {
        cost: kinds.iter().sum::<f64>() + f64::from(r.searches) * SEARCH,
        saved: m * t[4] * (i - cr) / 1e6,
        requests: 1,
        tokens: [
            (n[0], kinds[0]),
            (n[1], kinds[1]),
            (n[2] + n[3], kinds[2]),
            (n[4], kinds[3]),
        ],
    }
}

/// The Unix time at which the clock reads `local`, daylight saving
/// included; `off` gives the clock's offset at a Unix time.
fn utc(local: i64, off: &dyn Fn(i64) -> i64) -> i64 {
    let guess = local - off(local);
    local - off(guess)
}

/// When the week holding `now` began: the last time the clock read the
/// reset's day and hour.
fn week_start(now: i64, (day, hour): Reset, off: &dyn Fn(i64) -> i64) -> i64 {
    let local = now + off(now);
    // 1970-01-01 was a Thursday.
    let into = (local.div_euclid(DAY) + 3).rem_euclid(7) * DAY + local.rem_euclid(DAY);
    let back = (into - (i64::from(day) * DAY + i64::from(hour) * 3600)).rem_euclid(WEEK);
    let start = utc(local - back, off);
    // Only in the hour the clock goes back.
    if start > now {
        utc(local - back - WEEK, off)
    } else {
        start
    }
}

/// `5 pm`, or `5:30 pm` off the hour, for `secs` into a day; `12 am` is
/// midnight and `12 pm` noon.
pub fn clock_time(secs: i64) -> String {
    let (h, m) = (secs / 3600 % 24, secs / 60 % 60);
    let half = if h < 12 { "am" } else { "pm" };
    let h = if h % 12 == 0 { 12 } else { h % 12 };
    match m {
        0 => format!("{h} {half}"),
        _ => format!("{h}:{m:02} {half}"),
    }
}

/// When the clock reads `t`, as `Thu 5 pm`.
fn clock(t: i64, off: &dyn Fn(i64) -> i64) -> String {
    let local = t + off(t);
    let day = WEEKDAYS[(local.div_euclid(DAY) + 3).rem_euclid(7) as usize];
    format!("{day} {}", clock_time(local.rem_euclid(DAY)))
}

/// Local day `day`, counted from 1970-01-01, as `Thu 29`.
fn day_label(day: i64) -> String {
    let weekday = WEEKDAYS[(day + 3).rem_euclid(7) as usize];
    format!("{weekday} {}", civil(day).2)
}

/// When the 5-hour window under way at `now` began, from the `times` of
/// the requests, sorted: the first request after the last window ended.
// ponytail: an estimate from when requests came; the quotaLimits.resetsAt
// of a rejected row gives the exact end, add it if the estimate proves off.
fn window(times: &[i64], now: i64) -> Option<i64> {
    let mut start = *times.first()?;
    for &t in times {
        if t >= start + WINDOW {
            start = t;
        }
    }
    (now < start + WINDOW).then_some(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A response as Claude Code logs it, with `extra` fields in its usage.
    fn line(id: &str, ts: &str, usage: &str) -> String {
        format!(
            r#"{{"parentUuid":"p","isSidechain":false,"message":{{"model":"claude-opus-5-5","id":"msg_{id}","type":"message","role":"assistant","content":[{{"type":"text","text":"hi"}}],"usage":{{{usage}}}}},"requestId":"req_{id}","type":"assistant","timestamp":"{ts}","cwd":"/work/shop"}}"#
        )
    }

    const SMALL: &str = r#""input_tokens":3,"output_tokens":40,"cache_creation_input_tokens":0,"cache_read_input_tokens":0"#;

    fn parsed(key: u64, at: i64, model: &str, cwd: &str, agent: bool, out: u32) -> Parsed {
        Parsed {
            key,
            at,
            model: model.into(),
            cwd: cwd.into(),
            agent,
            tokens: [0, out, 0, 0, 0],
            fast: false,
            us: false,
            searches: 0,
        }
    }

    fn row(tokens: [u32; 5]) -> Row {
        Row {
            key: 0,
            at: 0,
            model: 0,
            project: 0,
            agent: false,
            tokens,
            fast: false,
            us: false,
            searches: 0,
        }
    }

    const UTC: &dyn Fn(i64) -> i64 = &|_| 0;

    fn at(ts: &str) -> i64 {
        unix(ts).expect(ts)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn reads_a_response_and_skips_other_lines() {
        let usage = r#""input_tokens":3,"cache_creation_input_tokens":500,"cache_read_input_tokens":9000,"output_tokens":42,"output_tokens_details":{"thinking_tokens":7},"server_tool_use":{"web_search_requests":2},"service_tier":"standard","inference_geo":"us","iterations":[],"speed":"fast""#;
        let ts = "2026-10-29T14:03:22.123Z";
        let got = parse(&format!("{}\r\n", line("1", ts, usage)));
        let want = Parsed {
            key: key("msg_1", "req_1"),
            at: 1_793_282_602,
            model: "claude-opus-5-5".into(),
            cwd: "/work/shop".into(),
            agent: false,
            tokens: [3, 42, 500, 0, 9000],
            fast: true,
            us: true,
            searches: 2,
        };
        assert_eq!(got, Some(want));
        let sidechain =
            line("2", ts, SMALL).replace(r#""isSidechain":false"#, r#""isSidechain":true"#);
        assert!(parse(&sidechain).is_some_and(|p| p.agent));
        for (why, bad) in [
            (
                "a user row",
                line("3", ts, SMALL).replace(r#""type":"assistant""#, r#""type":"user""#),
            ),
            (
                "a synthetic row",
                line("4", ts, SMALL).replace("claude-opus-5-5", "<synthetic>"),
            ),
            (
                "no tokens",
                line("5", ts, r#""input_tokens":0,"output_tokens":0"#),
            ),
            (
                "no id",
                line("6", ts, SMALL).replace(r#""id":"msg_6","#, ""),
            ),
            ("a bad time", line("7", "2026-10-29 14:03:22", SMALL)),
            ("malformed", line("8", ts, SMALL).replace("}}", "}")),
            (
                "no usage",
                r#"{"type":"assistant","message":{"id":"x"}}"#.into(),
            ),
            ("a file", r#"{"type":"summary","summary":"tests"}"#.into()),
        ] {
            assert_eq!(parse(&bad), None, "{why}");
        }
    }

    #[test]
    fn splits_cache_writes_by_how_long_they_last() {
        let ts = "2026-10-29T14:03:22Z";
        let both = r#""input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":700,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":200,"ephemeral_1h_input_tokens":500}"#;
        let tokens = |u| parse(&line("1", ts, u)).map(|p| p.tokens);
        assert_eq!(tokens(both), Some([1, 1, 200, 500, 0]));
        let plain = r#""input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":700"#;
        assert_eq!(
            tokens(plain),
            Some([1, 1, 700, 0, 0]),
            "five minutes without a split"
        );
    }

    #[test]
    fn model_ids_lose_dates_and_suffixes() {
        for (raw, id) in [
            ("claude-haiku-4-5-20251001", "claude-haiku-4-5"),
            ("claude-opus-5-5[1m]", "claude-opus-5-5"),
            (
                "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
                "claude-sonnet-4-5",
            ),
            ("claude-sonnet-4-5@20250929", "claude-sonnet-4-5"),
            ("claude-3-5-haiku-20241022", "claude-3-5-haiku"),
            ("claude-opus-5-5", "claude-opus-5-5"),
            ("gpt-5", "gpt-5"),
        ] {
            assert_eq!(model_id(raw).as_deref(), Some(id), "{raw}");
        }
        assert_eq!(model_id("<synthetic>"), None);
        assert_eq!(model_id(""), None);
    }

    #[test]
    fn models_read_as_people_say_them() {
        assert_eq!(model_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_name("claude-sonnet-4"), "Sonnet 4");
        assert_eq!(model_name("claude-3-5-haiku"), "Haiku 3.5");
        assert_eq!(model_name("gpt-5"), "gpt-5");
        assert_eq!(
            model_name("claude-opus-5-5-preview"),
            "claude-opus-5-5-preview"
        );
    }

    #[test]
    fn prices_follow_the_table() {
        let opus = price("claude-opus-5-5").expect("listed");
        assert!(!opus.1);
        let m = 1_000_000;
        let spend = |tokens, f: fn(&mut Row)| {
            let mut r = row(tokens);
            f(&mut r);
            cost(&r, opus.0, false)
        };
        let plain = |_: &mut Row| {};
        let kinds = |s: Spend| s.tokens.map(|k| k.1);
        assert!(close(spend([m, 0, 0, 0, 0], plain).cost, 4.0));
        assert!(close(spend([0, m, 0, 0, 0], plain).cost, 20.0));
        assert!(close(spend([0, 0, m, 0, 0], plain).cost, 5.0));
        assert!(close(spend([0, 0, 0, m, 0], plain).cost, 8.0));
        let reads = spend([0, 0, 0, 0, m], plain);
        assert!(close(reads.cost, 0.2) && close(reads.saved, 3.8));
        let all = spend([m, m, m, m, m], plain);
        let want = [4.0, 20.0, 13.0, 0.2];
        assert!(
            kinds(all.clone())
                .iter()
                .zip(want)
                .all(|(a, b)| close(*a, b)),
            "{all:?}"
        );
        assert_eq!(
            all.tokens.map(|k| k.0),
            [m.into(), m.into(), 2 * u64::from(m), m.into()]
        );
        assert!(close(spend([m, 0, 0, 0, 0], |r| r.fast = true).cost, 8.0));
        assert!(close(spend([m, 0, 0, 0, 0], |r| r.us = true).cost, 4.4));
        assert!(close(spend([0; 5], |r| r.searches = 1).cost, 0.01));
        // A long prompt to Haiku 5.5 costs five times as much.
        let haiku = price("claude-haiku-5-5").expect("listed").0;
        let r = row([100_001, 0, 0, 0, 0]);
        assert!(long("claude-haiku-5-5", &r) && !long("claude-opus-5-5", &r));
        assert!(!long("claude-haiku-5-5", &row([100_000, 0, 0, 0, 0])));
        assert!(close(cost(&r, haiku, true).cost, 5.0 * 0.1 * 0.100_001));
        // A model newer than the table costs what the newest of its family does.
        assert_eq!(price("claude-opus-6"), Some((opus.0, true)));
        assert_eq!(
            price("claude-4-haiku").map(|p| p.0),
            price("claude-haiku-5-5").map(|p| p.0)
        );
        assert_eq!(price("gpt-5"), None);
    }

    #[test]
    fn keeps_the_last_copy_of_a_response() {
        let out = |t: &Tracker| t.rows.iter().map(|r| r.tokens[1]).collect::<Vec<_>>();
        for order in [[10, 50, 120], [120, 10, 50], [50, 120, 10]] {
            let mut t = Tracker::new(None);
            for n in order {
                t.add(0, parsed(1, 100, "claude-opus-5-5", "/work/shop", false, n));
            }
            assert_eq!(out(&t), [120], "{order:?}");
        }
        // A sidechain copy never takes the place of the session's own.
        let mut t = Tracker::new(None);
        t.add(
            0,
            parsed(1, 100, "claude-opus-5-5", "/work/shop", false, 10),
        );
        t.add(
            0,
            parsed(1, 100, "claude-opus-5-5", "/work/shop", true, 500),
        );
        assert_eq!(out(&t), [10]);
        assert!(!t.rows[0].agent);
        // Nor does a copy in another project's log count twice.
        t.projects = vec![
            ("a".into(), String::new(), false),
            ("b".into(), String::new(), false),
        ];
        t.add(
            1,
            parsed(1, 100, "claude-opus-5-5", "/work/shop", false, 10),
        );
        t.add(
            1,
            parsed(2, 100, "claude-opus-5-5", "/work/shop", false, 10),
        );
        assert_eq!(out(&t), [10, 10]);
    }

    #[test]
    fn timestamps_read_as_unix_seconds() {
        assert_eq!(unix("2026-10-29T14:03:22.123Z"), Some(1_793_282_602));
        assert_eq!(unix("2026-10-29T14:03:22Z"), Some(1_793_282_602));
        assert_eq!(unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        for day in [-800_000, -1, 0, 59, 11_016, 19_782, 20_514, 2_932_896] {
            let (y, m, d) = civil(day);
            assert_eq!(days_from_civil(y, m, d), day, "{y}-{m}-{d}");
        }
        assert_eq!(civil(19_782), (2024, 2, 29));
        assert_eq!(civil(19_783), (2024, 3, 1));
        for bad in [
            "",
            "2026-10-29T14:03:22",
            "2026-10-29T14:03:22+02:00",
            "2026-13-29T14:03:22Z",
            "2026-10-29T24:03:22Z",
            "2026-10-29T14:03:22.Z",
            "2026-1-29T14:03:22Z",
            "+026-10-29T14:03:22Z",
        ] {
            assert_eq!(unix(bad), None, "{bad}");
        }
    }

    /// US Central in 2026: daylight saving from 8 March to 1 November.
    fn central(t: i64) -> i64 {
        if (1_772_956_800..1_793_516_400).contains(&t) {
            -5 * 3600
        } else {
            -6 * 3600
        }
    }

    #[test]
    fn weeks_start_at_the_reset() {
        let now = at("2026-10-29T14:03:22Z");
        let monday = at("2026-10-26T00:00:00Z");
        assert_eq!(week_start(now, (0, 0), UTC), monday);
        // Thursday 5 pm, which is still to come today.
        assert_eq!(week_start(now, (3, 17), UTC), at("2026-10-22T17:00:00Z"));
        let plus2: &dyn Fn(i64) -> i64 = &|_| 2 * 3600;
        assert_eq!(week_start(now, (0, 0), plus2), monday - 2 * 3600);
        // Exactly at the reset the week starts; a second before, it has not.
        let reset = at("2026-10-29T17:00:00Z");
        assert_eq!(week_start(reset, (3, 17), UTC), reset);
        assert_eq!(week_start(reset - 1, (3, 17), UTC), reset - WEEK);
        let this = week_start(now, (3, 17), UTC);
        assert_eq!(week_start(this - 1, (3, 17), UTC), this - WEEK, "last week");
        assert_eq!(
            week_start(this + 8 * DAY, (3, 17), UTC),
            this + WEEK,
            "the next reset"
        );
        assert_eq!(clock(reset, UTC), "Thu 5 pm");
        assert_eq!(clock(reset + 1800, plus2), "Thu 7:30 pm");
    }

    #[test]
    fn weeks_keep_the_clock_across_daylight_saving() {
        let off: &dyn Fn(i64) -> i64 = &central;
        let week = |now| {
            let this = week_start(now, (0, 0), off);
            (
                this,
                week_start(this - 1, (0, 0), off),
                week_start(this + 8 * DAY, (0, 0), off),
            )
        };
        // The week after the clocks go back, and the one before.
        assert_eq!(
            week(1_793_815_200),
            (1_793_599_200, 1_792_990_800, 1_794_204_000)
        );
        // The week after they go forward.
        let (this, last, _) = week(1_773_252_000);
        assert_eq!((this, last), (1_773_032_400, 1_772_431_200));
        assert_eq!(clock(this, off), "Mon 12 am");
        // A reset at 1 am on the Sunday 1 am comes twice, either time.
        for now in [at("2026-11-01T06:30:00Z"), at("2026-11-01T07:30:00Z")] {
            let start = week_start(now, (6, 1), off);
            assert!(start <= now && now - start < WEEK, "{now}");
            assert_eq!(clock(start, off), "Sun 1 am");
        }
    }

    #[test]
    fn times_read_on_a_twelve_hour_clock() {
        assert_eq!(clock_time(0), "12 am");
        assert_eq!(clock_time(12 * 3600), "12 pm");
        assert_eq!(clock_time(17 * 3600), "5 pm");
        assert_eq!(clock_time(19 * 3600 + 45 * 60), "7:45 pm");
        assert_eq!(clock_time(9 * 3600 + 5 * 60), "9:05 am");
    }

    #[test]
    fn days_follow_the_local_calendar() {
        let day = |ts| at(ts).div_euclid(DAY);
        assert_eq!(day_label(day("2026-10-29T00:00:00Z")), "Thu 29");
        assert_eq!(day_label(day("2026-11-01T12:00:00Z")), "Sun 1");
        // Thursday 3 am in UTC is still Wednesday in Chicago.
        let mut t = Tracker::new(None);
        t.add(
            0,
            parsed(
                1,
                at("2026-10-29T03:00:00Z"),
                "claude-opus-5-5",
                "/w",
                false,
                1_000_000,
            ),
        );
        let r = t.report(at("2026-10-29T18:00:00Z"), (0, 0), &central);
        let days: Vec<_> = r.days.iter().map(|d| d.0.as_str()).collect();
        assert_eq!(days, ["Mon 26", "Tue 27", "Wed 28", "Thu 29"]);
        assert_eq!(r.days.iter().position(|d| d.1 > 0.0), Some(2));
    }

    #[test]
    fn the_five_hour_window_starts_at_the_first_request_after_the_last_one_ended() {
        let times = [0, 100, 18_000, 18_100];
        assert_eq!(window(&times, 20_000), Some(18_000));
        assert_eq!(window(&times, 36_001), None);
        assert_eq!(window(&[0, 100], 17_999), Some(0));
        assert_eq!(window(&[], 0), None);
    }

    /// A folder for one test, gone when the test ends, even if it fails.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir =
                std::env::temp_dir().join(format!("blitz-usage-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reads_only_what_was_added() {
        let t = Temp::new("read");
        let dir = t.0.join("shop");
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("s.jsonl");
        let ts = "2026-10-29T14:03:22Z";
        let (a, b, c) = (
            line("a1", ts, SMALL),
            line("b1", ts, SMALL),
            line("c1", ts, SMALL),
        );
        let mut tr = Tracker::new(Some(t.0.clone()));
        let since = 1_000_000;
        // Half a line is one still being written.
        std::fs::write(&log, format!("{a}\n{}", &b[..20])).unwrap();
        tr.read(since);
        assert_eq!(tr.rows.len(), 1);
        assert_eq!(tr.files.get(&log), Some(&(a.len() as u64 + 1)));
        std::fs::write(&log, format!("{a}\n{b}\n")).unwrap();
        tr.read(since);
        assert_eq!(tr.rows.len(), 2, "completed");
        // Only the new bytes are read: a changed first line goes unseen.
        let other = a.replace("a1", "z1");
        std::fs::write(&log, format!("{other}\n{b}\n{c}\n")).unwrap();
        tr.read(since);
        assert_eq!(tr.rows.len(), 3);
        // Rewritten shorter, it is read again, and counts nothing twice.
        std::fs::write(&log, format!("{a}\n{b}\n")).unwrap();
        tr.read(since);
        assert_eq!(tr.rows.len(), 3);
        assert_eq!(tr.projects[0].1, "shop", "named from the row's folder");
        // A log last written before the week cannot hold its rows.
        let old = dir.join("old.jsonl");
        std::fs::write(&old, format!("{}\n", line("d1", ts, SMALL))).unwrap();
        let f = std::fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::from_secs(1000))
            .unwrap();
        drop(f);
        tr.read(since);
        assert_eq!(tr.rows.len(), 3);
        assert!(!tr.files.contains_key(&old));
        // A workflow agent's log, deep down.
        let deep =
            t.0.join("x")
                .join("s")
                .join("subagents")
                .join("workflows")
                .join("wf_1");
        std::fs::create_dir_all(&deep).unwrap();
        let agent = deep.join("agent-a.jsonl");
        std::fs::write(&agent, format!("{}\n", line("e1", ts, SMALL))).unwrap();
        std::fs::write(deep.join("agent-a.meta.json"), "{}").unwrap();
        tr.read(since);
        assert_eq!(tr.rows.len(), 4);
        // A log deleted is forgotten.
        std::fs::remove_file(&log).unwrap();
        tr.read(since);
        assert!(!tr.files.contains_key(&log) && tr.files.contains_key(&agent));
        assert!(tr.report(at(ts), (0, 0), UTC).found);
        assert!(
            !Tracker::new(Some(t.0.join("gone")))
                .report(0, (0, 0), UTC)
                .found
        );
    }

    #[test]
    fn a_week_gone_by_drops_its_rows() {
        let mut t = Tracker::new(None);
        t.add(0, parsed(1, 100, "claude-opus-5-5", "/w", false, 1));
        t.add(0, parsed(2, 1000, "claude-opus-5-5", "/w", false, 1));
        t.read(500);
        assert_eq!(t.rows.iter().map(|r| r.at).collect::<Vec<_>>(), [1000]);
        // The index follows the rows that are left.
        t.add(0, parsed(2, 1000, "claude-opus-5-5", "/w", false, 5));
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows[0].tokens[1], 5);
    }

    #[test]
    fn report_sums_weeks_days_models_projects_and_agents() {
        let mut t = Tracker::new(None);
        t.projects = vec![
            ("C--work-shop".into(), String::new(), false),
            ("C--work-site".into(), String::new(), false),
        ];
        let m = 1_000_000;
        let mut add = |key, ts, model: &str, project, cwd: &str, agent, input| {
            let mut p = parsed(key, at(ts), model, cwd, agent, 0);
            p.tokens[0] = input;
            t.add(project, p);
        };
        // An agent in a worktree is read first, then the session itself.
        add(
            1,
            "2026-10-28T09:00:00Z",
            "claude-sonnet-4-5",
            0,
            "/work/.wt/wf_1",
            true,
            m,
        );
        add(
            2,
            "2026-10-26T10:00:00Z",
            "claude-opus-5-5",
            0,
            "/work/shop",
            false,
            m,
        );
        add(
            3,
            "2026-10-27T10:00:00Z",
            "claude-opus-7",
            1,
            "/work/site",
            false,
            m / 2,
        );
        add(
            4,
            "2026-10-29T12:00:00Z",
            "gpt-5",
            1,
            "/work/site",
            false,
            m,
        );
        add(
            5,
            "2026-10-23T10:00:00Z",
            "claude-opus-5-5",
            0,
            "/work/shop",
            false,
            2 * m,
        );
        let r = t.report(at("2026-10-29T14:00:00Z"), (0, 0), UTC);
        assert!(close(r.this.cost, 9.0), "{r:?}");
        assert_eq!(r.this.requests, 3);
        assert!(close(r.last, 8.0));
        assert_eq!(r.unpriced, 1);
        assert!(close(r.agents, 3.0));
        assert_eq!(r.since, "Mon 12 am");
        assert_eq!(
            r.week,
            (at("2026-10-26T00:00:00Z"), at("2026-11-02T00:00:00Z"))
        );
        let days: Vec<_> = r.days.iter().map(|d| (d.0.as_str(), d.1)).collect();
        assert_eq!(
            days,
            [
                ("Mon 26", 4.0),
                ("Tue 27", 2.0),
                ("Wed 28", 3.0),
                ("Thu 29", 0.0)
            ]
        );
        let models: Vec<_> = r.models.iter().map(|m| (m.0.as_str(), m.1, m.2)).collect();
        assert_eq!(
            models,
            [
                ("Opus 5.5", Some(4.0), false),
                ("Sonnet 4.5", Some(3.0), false),
                ("Opus 7", Some(2.0), true),
                ("gpt-5", None, false),
            ]
        );
        // Named from the session's row, not the agent's worktree.
        assert_eq!(
            r.projects,
            [("shop".to_string(), 7.0), ("site".to_string(), 2.0)]
        );
        // The window holds the request at noon, which has no price.
        assert_eq!(r.window, Some(("5 pm".to_string(), 0.0)));
        assert!(!r.found);
    }
}
