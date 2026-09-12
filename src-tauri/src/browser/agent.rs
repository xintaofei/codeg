//! Who may read a page on an agent's behalf, and the one read that exists.
//!
//! The rule everything here implements: **nothing is granted automatically.**
//! Not by address class — a private address says nothing about whether the
//! page behind it is signed in. Not by which process is listening — a `socat`
//! in front of the real server owns the port just as convincingly. Not by an
//! origin allow-list — a same-origin substitution is invisible to any
//! re-check. A tab an agent opened itself is no different from one the user
//! opened. The only way an agent reads a page is a person sharing that tab.
//!
//! Reading is a grant, not just acting on it: an unshared page is a data
//! leak through `snapshot` exactly as much as through a click, and the pages
//! most worth protecting — the ones with a session in them — are the ones a
//! tree would describe in the most detail.
//!
//! The grant lives on the tab (`BrowserTabState::agent_grant`), which is why
//! this module is decisions and wire types rather than a store: the code that
//! learns a tab changed origin is the code that revokes, under the one lock
//! it already holds, with no second map to keep in step.

use serde::{Deserialize, Serialize};

use super::types::{BrowserTabState, TabKind};

/// How much of a tab an agent may have.
///
/// `Control` is defined here rather than with the package that will act on a
/// page, because the level a person picks is the level they picked: a UI that
/// could only offer `Read` today would have to re-ask everyone the day
/// actions land, and a stored `control` that silently behaved as `read` would
/// be worse. Nothing in this build asks for `Control` yet — `allows` is how
/// the asking will be spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum GrantLevel {
    #[default]
    None,
    Read,
    Control,
}

impl GrantLevel {
    /// Whether a tab at this level may be asked for something that needs
    /// `required`.
    pub fn allows(self, required: GrantLevel) -> bool {
        self.rank() >= required.rank()
    }

    /// Written as a match, not as a derived `Ord`, so that adding a level
    /// forces someone to say where it sits rather than inheriting a position
    /// from where it happened to be typed.
    fn rank(self) -> u8 {
        match self {
            GrantLevel::None => 0,
            GrantLevel::Read => 1,
            GrantLevel::Control => 2,
        }
    }
}

/// What is answering on a loopback address.
///
/// A grant names an origin, and for a real site the name is the thing: nobody
/// else can be `https://mail.example.com`. `http://localhost:3000` names
/// nothing durable — it is a port number, and this morning's dev server and
/// this afternoon's unrelated admin tool wear it equally well. Someone who
/// shares `localhost:3000` means *the program they are working on*, so this
/// records enough to notice when that stops being what answers.
///
/// **It records the program, not the process.** The instinct is to pin
/// `(pid, path, start time)`, which is what an identity looks like — but a
/// dev server under `nodemon`, `cargo watch`, `air` or `uvicorn --reload`
/// gets a new pid on every save, and an agent-driven edit loop is *made* of
/// saves. Pinning the instance would end the sharing several times a minute
/// in exactly the workflow this exists to serve, and a person clicking
/// "share" back every time is a person who learns to stop reading the notice.
/// A restart is still the same program serving the same project, and it is
/// allowed to be.
///
/// The working directory is here because the program alone rarely says
/// anything: two projects on port 3000 an hour apart are both `node`. What
/// distinguishes them is where they were started, which also survives the
/// restart the executable path survives.
///
/// Either field is `None` when this process cannot see it — a socket owned by
/// another user, a platform with no way to ask. `None` compares equal only to
/// `None`, so a listener that was invisible and is now visible (or the other
/// way round) reads as a change, which it is: an unprivileged agent can only
/// bind a port as the user, where we *can* see it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenerIdentity {
    /// Absolute path of the listening process's executable.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub program: Option<String>,
    /// Its working directory.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub workdir: Option<String>,
}

impl ListenerIdentity {
    /// The name to put in front of a person when this program displaced
    /// another: the executable's file name, which is what they would have
    /// typed, rather than the path they never look at.
    pub fn display_name(&self) -> Option<&str> {
        let program = self.program.as_deref()?;
        program
            .rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
    }
}

/// A grant in force on one tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentGrant {
    /// Never `GrantLevel::None`: a tab with no grant carries no `AgentGrant`
    /// at all, so "level none, but still bound to an origin" is a state that
    /// cannot be written down.
    pub level: GrantLevel,
    /// The origin the tab was showing when the person shared it, in the ASCII
    /// serialization `hooks::origin_of` produces.
    pub origin: String,
    /// Unix milliseconds. The audit surface shows when access began; it is
    /// not used for any expiry, because a grant ends when the user revokes it
    /// or the page leaves the origin, not after a duration nobody chose.
    pub granted_at: i64,
    /// What was serving [`Self::origin`] when the person shared it, for a
    /// loopback address this process could look into. `None` everywhere else,
    /// and an absent pin is never checked: a grant that could not be pinned
    /// behaves exactly as it did before this existed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub listener: Option<ListenerIdentity>,
}

impl AgentGrant {
    /// Whether this grant still covers a tab now showing `origin`.
    ///
    /// A grant is for one origin, so anything else ends it — another site,
    /// and equally an opaque origin (`about:blank`, a `data:` document, a
    /// `blob:null`), which has nothing to compare and is not the page the
    /// person was looking at when they shared it.
    pub fn covers(&self, origin: Option<&str>) -> bool {
        origin == Some(self.origin.as_str())
    }
}

/// The level a tab is at, reading the absence of a grant as `None`.
pub fn level_of(grant: Option<&AgentGrant>) -> GrantLevel {
    grant.map_or(GrantLevel::None, |g| g.level)
}

/// One tab as an agent may see it before it is allowed to read anything.
///
/// A listing exists so an agent can *name* a tab — to read it, or to ask the
/// user to share it — which is why it is not itself behind a grant. What it
/// carries is bounded by that purpose: an address, and whether this agent may
/// read the page at it.
///
/// The title is the exception that proves the rule. It is chosen by the page
/// and is the first line of its content — an unshared tab called
/// "Re: termination letter — Mail" would hand over the very thing the grant
/// exists to withhold. So it appears only once the page is readable, at which
/// point the agent could have read the whole document anyway and is merely
/// saved a round trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTabSummary {
    pub tab_id: String,
    /// `None` for a tab that has committed no document yet, or one whose
    /// document has an opaque origin. Such a tab cannot be shared at all (see
    /// [`grantable_origin`]); it is listed anyway, because a page that is
    /// merely still loading would otherwise drop out of the listing and
    /// reappear a moment later.
    pub origin: Option<String>,
    pub level: GrantLevel,
    /// Present only from [`GrantLevel::Read`] upwards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// What an agent may know about a tab, or `None` for one it should not be
/// told about at all.
///
/// The only such tab today is a document guest. It shows a local file — one
/// the agent itself usually wrote — through a scheme spelled differently on
/// every platform, it can never be shared ([`NotGrantable::DocumentGuest`]),
/// and the file is on disk where the agent reads it directly. Listing it would
/// only invite an agent to ask for something nobody can grant.
pub fn summarize_tab(state: &BrowserTabState) -> Option<AgentTabSummary> {
    if state.kind == TabKind::Document {
        return None;
    }
    let level = level_of(state.agent_grant.as_ref());
    Some(AgentTabSummary {
        tab_id: state.tab_id.clone(),
        origin: state.origin.clone(),
        level,
        title: level
            .allows(GrantLevel::Read)
            .then(|| state.title.clone()),
    })
}

/// Why a tab's grant changed.
///
/// The level itself travels on `browser://state` with the rest of the tab, so
/// this is not a second source of truth for it. It carries what the state
/// cannot: that a change was not the user's doing, and what happened instead.
/// A tab that silently stops answering an agent mid-task is a bug report; a
/// tab that says "access to example.com ended when the page went to
/// other.example" is a thing the user can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GrantChange {
    /// The user shared the tab, or changed the level.
    Granted,
    /// The user took it back.
    Revoked,
    /// The page left the origin the grant was bound to.
    Navigated,
    /// The address stayed the same and the thing behind it did not: another
    /// program is serving the loopback port this grant was pinned to. Its own
    /// transition rather than a [`Self::Navigated`] with a confusing origin,
    /// because what a person has to be told is the opposite of a navigation —
    /// the address in the toolbar is still the one they shared.
    Replaced,
}

/// `browser://agent-grant`: a transition, with its reason. Current level:
/// `browser://state`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentGrantPayload {
    pub tab_id: String,
    pub change: GrantChange,
    pub level: GrantLevel,
    /// The origin involved: the one just granted, or the one just lost.
    pub origin: Option<String>,
}

pub const AGENT_GRANT_EVENT: &str = "browser://agent-grant";

/// What an agent did to a page, for the person watching it.
///
/// One variant today because there is one thing an agent can do. Acting on a
/// page (W3.2) extends this rather than reinterpreting it, which is the point
/// of spelling out a single-variant enum: the alternative — a bare "an agent
/// touched this tab" — would have to be redefined the first time two kinds of
/// touch existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentAction {
    /// Took a snapshot of the page.
    Read,
}

/// Whether the action happened.
///
/// Refusals are reported, not swallowed. They are the more interesting half:
/// a page the user never shared, or one whose grant died when it navigated,
/// being asked for repeatedly is exactly what someone would want to see, and
/// it is invisible everywhere else — the agent is told, the user is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentOutcome {
    Done,
    /// No grant covered the page. The agent was told to ask.
    Refused,
    /// The grant was there; the page was not reachable (no answer from the
    /// world, an unreadable one). Reported so that "nothing on the strip"
    /// keeps meaning "nothing reached this tab" rather than "nothing worked".
    Failed,
}

/// `browser://agent-activity`: one agent's one attempt on one tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivityPayload {
    pub tab_id: String,
    pub action: AgentAction,
    pub outcome: AgentOutcome,
    /// Unix milliseconds.
    pub at: i64,
}

pub const AGENT_ACTIVITY_EVENT: &str = "browser://agent-activity";

/// Why a tab cannot be shared with an agent at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotGrantable {
    /// Nothing to bind to: the tab has not committed a document yet, or the
    /// one it committed has an opaque origin (`about:blank`, `data:`, a
    /// `blob:` of one of those). There is no address here that a later
    /// navigation could be compared against, so a grant made now could never
    /// be revoked for leaving.
    NoOrigin,
    /// A document guest (`codeg-doc:`). It shows a local file — usually one
    /// the agent wrote — through a scheme spelled differently on every
    /// platform (`codeg-doc://…` under WebKit, `https://codeg-doc.localhost/…`
    /// under WebView2), so there is no stable origin to bind to. There is
    /// also no need: the file is on disk, where the agent reads it directly
    /// and without a browser in between.
    DocumentGuest,
}

/// The origin a grant on this tab would bind to.
///
/// Web origins only. The grant model's whole mechanism is "this origin and no
/// other", and a scheme whose origin is not a stable `scheme://host:port` has
/// no way to participate in it.
pub fn grantable_origin(state: &BrowserTabState) -> Result<&str, NotGrantable> {
    if state.kind == TabKind::Document {
        return Err(NotGrantable::DocumentGuest);
    }
    let origin = state.origin.as_deref().ok_or(NotGrantable::NoOrigin)?;
    if origin.starts_with("http://") || origin.starts_with("https://") {
        Ok(origin)
    } else {
        Err(NotGrantable::NoOrigin)
    }
}

/// Move a tab to `level`, atomically with the origin it is showing.
///
/// Returns the transition to announce, or `None` when nothing changed — so a
/// second press of a button that is already on says nothing, and re-granting
/// keeps the `granted_at` the audit surface is showing rather than resetting
/// a clock the user did not touch.
///
/// Atomic because the origin a grant binds to has to be the one on screen at
/// the instant it is made. Reading the origin, deciding, and then writing
/// leaves a gap in which the page can navigate — and a grant written into
/// that gap would be bound to an origin the tab has already left, which is
/// exactly the state [`revoke_if_departed`] exists to make impossible.
pub fn apply_grant(
    state: &mut BrowserTabState,
    level: GrantLevel,
    now: i64,
    listener: Option<ProbedListener>,
) -> Result<Option<AgentGrantPayload>, NotGrantable> {
    if level == GrantLevel::None {
        let previous = state.agent_grant.take();
        return Ok(previous.map(|previous| AgentGrantPayload {
            tab_id: state.tab_id.clone(),
            change: GrantChange::Revoked,
            level: GrantLevel::None,
            origin: Some(previous.origin),
        }));
    }
    let origin = grantable_origin(state)?.to_string();
    // Asking the operating system who holds a port takes long enough that the
    // tab can navigate while the answer is on its way, so the probe carries
    // the address it was taken for and is dropped unless that is still the
    // address being shared. Pinning one origin's listener to another origin's
    // grant would revoke it at the next read for no reason anybody could name.
    let listener = listener
        .filter(|probed| probed.origin == origin)
        .map(|probed| probed.identity);
    if let Some(existing) = state.agent_grant.as_mut() {
        if existing.level == level && existing.origin == origin {
            // The same share, re-affirmed. Take the fresh pin — the person
            // just pointed at what is running now and called it theirs — but
            // keep `granted_at`, and announce nothing: pressing a button that
            // is already on is not a transition.
            existing.listener = listener;
            return Ok(None);
        }
    }
    state.agent_grant = Some(AgentGrant {
        level,
        origin: origin.clone(),
        granted_at: now,
        listener,
    });
    Ok(Some(AgentGrantPayload {
        tab_id: state.tab_id.clone(),
        change: GrantChange::Granted,
        level,
        origin: Some(origin),
    }))
}

/// A listener probe's answer together with the address it was taken for.
///
/// The pairing is the point: a bare [`ListenerIdentity`] arriving at
/// [`apply_grant`] could not be checked against the origin it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedListener {
    pub origin: String,
    pub identity: ListenerIdentity,
}

/// Re-check a tab's grant against what is serving its address now, and take
/// the grant away if that is no longer the program it was pinned to. Returns
/// what was lost, for the notice.
///
/// Only a grant that *has* a pin can lose one. A grant on a real site, or on
/// a loopback address this process could not look into when it was made,
/// passes through untouched — the check can only ever narrow what an agent
/// may read, never widen it.
///
/// `origin` is the address `serving` was probed for. The probe happens outside
/// the lock, so by the time its answer arrives the tab may have navigated and
/// been re-shared for somewhere else; a grant on a different address is not
/// this probe's business and is left alone.
pub fn revoke_if_replaced(
    state: &mut BrowserTabState,
    origin: &str,
    serving: &ListenerIdentity,
) -> Option<AgentGrant> {
    let replaced = state
        .agent_grant
        .as_ref()
        .filter(|grant| grant.origin == origin)
        .and_then(|grant| grant.listener.as_ref())
        .is_some_and(|pinned| pinned != serving);
    if replaced {
        state.agent_grant.take()
    } else {
        None
    }
}

/// Re-check a tab's grant against the origin it is showing now, and take the
/// grant away if the page has left. Returns what was lost, for the notice.
///
/// This is the whole of the cross-origin revocation rule, and it is a
/// function so that it can be called from *every* place that writes
/// `state.origin` — the engine's load callback and the helper's navigation
/// report — rather than being spelled out once and forgotten at the second
/// site.
///
/// The engine's callback is the one that matters. A same-document navigation
/// reaches the host late (see [`epoch`]) and would be a poor thing to hang a
/// security boundary on, but it never needs to be: the engine refuses a
/// `pushState` to another origin, so the one class of navigation the host
/// learns about slowly is the one class that cannot cross the boundary this
/// grant is bound to. Calling it there anyway costs nothing and means a
/// helper that reported an address it should not have can only ever *lose* a
/// grant, never widen one.
pub fn revoke_if_departed(state: &mut BrowserTabState) -> Option<AgentGrant> {
    let origin = state.origin.as_deref();
    if state.agent_grant.as_ref().is_some_and(|g| !g.covers(origin)) {
        state.agent_grant.take()
    } else {
        None
    }
}

/// The snapshot engine (`browser-agent/`, built by `pnpm browser:agent`).
///
/// Evaluated into a tab's isolated world the first time a *granted* read
/// needs it, never at install time. Two things fall out of that: a tab no
/// agent ever reads does not pay to parse a hundred kilobytes of it on every
/// page load, and a tab nobody has shared contains no page-reading code at
/// all — so the grant check is not the only thing standing between an agent
/// and the page.
pub const AGENT_BUNDLE: &str = include_str!("js/agent.bundle.js");

/// Name the bundle publishes in the isolated world.
pub const AGENT_GLOBAL: &str = "__codegAgent";

/// What a caller asks a snapshot for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRequest {
    /// Cap on the rendered tree, in characters. Absent means no cap — the
    /// caller is the one that knows what it can hold, and a page large enough
    /// to matter is a page the caller wanted to know was large.
    pub max_chars: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotViewport {
    pub width: f64,
    pub height: f64,
    pub dpr: f64,
}

/// A page as an agent reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSnapshot {
    /// The token a later ref must quote, opaque to everyone but the world
    /// that issued it: its own generation and the host's epoch, joined.
    pub generation: String,
    /// The address the world was at when it walked the page. Not necessarily
    /// the tab's `url`: a snapshot is taken at a moment, and the host checks
    /// this one against the grant rather than trusting what it last heard.
    pub url: String,
    pub title: String,
    pub viewport: SnapshotViewport,
    /// The aria tree, in Playwright's `ai` rendering.
    pub tree: String,
    pub refs_count: usize,
    /// The tree stops at `max_chars` rather than at the end of the page.
    pub truncated: bool,
}

/// The host's half of the token a snapshot hands out: which incarnation of
/// this tab id, and how many navigations the host has learned of within it.
///
/// `browser-agent` mixes this into the generation it reports and, from the
/// next snapshot onwards, refuses a ref that quotes an older one. It has to
/// be the host's half because the world cannot see a page's own
/// `history.pushState`: patching `History.prototype` in an isolated world
/// patches *that world's* prototype, and the page calls a different function
/// object. So the world's own floor is "the address moved", and a route that
/// goes A → B → A arrives back at an address that matches.
///
/// What the host actually knows is worth being exact about, because it is
/// less than it sounds. A new document arrives through the engine's load
/// callback, promptly. A same-document navigation has no native signal at
/// all, and reaches the host only because `src/browser-injected/helper.js`
/// polls `location.href` a few times a second — and only while the document
/// reports itself visible. So a route change that leaves and returns between
/// two polls bumps nothing.
///
/// The epoch narrows that window; it does not close it. Closing it belongs to
/// the world, which can watch things the host cannot reach at all —
/// `history.length` moves on every `pushState`, and `popstate` fires there
/// like any other event — and that work belongs with the package that acts
/// on refs, where a stale one costs a wrong click rather than a confusing
/// tree.
pub fn epoch(generation: u64, nav_epoch: u64) -> String {
    format!("{generation}.{nav_epoch}")
}

/// The sentinel [`probe_and_snapshot`] returns in place of a snapshot when
/// the engine is not in this document yet. A bare string where a snapshot
/// would be an object, so it cannot be mistaken for one.
pub const ENGINE_ABSENT: &str = "absent";

/// `JSON.stringify(__codegAgent.snapshot({…}))`.
///
/// `epoch` is the host's, from [`epoch`]; `request` is the caller's. Both go
/// in through `serde_json`, so neither can break out of the expression. An
/// epoch is host-made and a cap is a number, so nothing here is dangerous
/// today; it is built this way because the day one of them comes from
/// somewhere else is not the day to discover it was string concatenation.
fn snapshot_call(request: &SnapshotRequest, epoch: &str) -> String {
    let options = serde_json::json!({
        "maxChars": request.max_chars,
        "epoch": epoch,
    });
    format!("JSON.stringify(globalThis.{AGENT_GLOBAL}.snapshot({options}))")
}

/// Take a snapshot, or say the engine is not here yet ([`ENGINE_ABSENT`]).
///
/// The host does not remember whether it has injected into the document a tab
/// is showing *now*. It asks. A remembered flag would have to be cleared from
/// every path that can replace a document, and being wrong about it produces
/// a snapshot that fails for a reason the caller cannot act on. So: one round
/// trip in the steady state, and one more on a cold document.
pub fn probe_and_snapshot(request: &SnapshotRequest, epoch: &str) -> String {
    format!(
        "typeof globalThis.{AGENT_GLOBAL} === 'undefined' ? {absent} : {call}",
        absent = serde_json::Value::from(ENGINE_ABSENT),
        call = snapshot_call(request, epoch),
    )
}

/// Put the engine in this document and take the snapshot, in one evaluation.
///
/// Together rather than one after the other because the two would otherwise
/// straddle a gap in which the page can navigate — and because the shim
/// evaluates an *expression*, while [`AGENT_BUNDLE`] is a program. Wrapping
/// it in a function body is what makes it one; the bundle publishes itself
/// through `globalThis`, so running it inside a function installs it just the
/// same, and its own top-level names stay out of the world's globals rather
/// than merely being unlikely to collide.
///
/// Deliberately not `eval`: a page's CSP has no say over an isolated world,
/// but that is a claim about three engines' handling of a directive, and
/// there is no reason to depend on it when a function body does the job.
pub fn install_and_snapshot(request: &SnapshotRequest, epoch: &str) -> String {
    format!(
        "(function(){{\n{AGENT_BUNDLE}\n;return {call};}})()",
        call = snapshot_call(request, epoch),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::types::{ChannelKind, SurfaceKind};

    fn tab(origin: Option<&str>, kind: TabKind) -> BrowserTabState {
        BrowserTabState {
            tab_id: "t1".into(),
            owner_window: "main".into(),
            kind,
            surface: SurfaceKind::Child,
            channel: ChannelKind::Native,
            channel_error: None,
            url: origin.unwrap_or("about:blank").into(),
            requested_url: String::new(),
            title: String::new(),
            favicon: None,
            loading: false,
            can_go_back: false,
            can_go_forward: false,
            origin: origin.map(str::to_string),
            zoom: 1.0,
            error: None,
            remote_host: None,
            opener_tab_id: None,
            profile: Some("default".into()),
            agent_grant: None,
        }
    }

    #[test]
    fn levels_are_ordered_and_none_allows_nothing() {
        assert!(GrantLevel::Control.allows(GrantLevel::Read));
        assert!(GrantLevel::Control.allows(GrantLevel::Control));
        assert!(GrantLevel::Read.allows(GrantLevel::Read));
        assert!(!GrantLevel::Read.allows(GrantLevel::Control));
        assert!(!GrantLevel::None.allows(GrantLevel::Read));
        // The absence of a grant is the absence of permission, not a hole.
        assert_eq!(level_of(None), GrantLevel::None);
        assert!(!level_of(None).allows(GrantLevel::Read));
    }

    /// A grant is for the origin it was made at. Everything else — another
    /// site, a port change, a scheme change, and having no origin at all —
    /// ends it.
    #[test]
    fn a_grant_covers_its_own_origin_and_nothing_else() {
        let grant = AgentGrant {
            level: GrantLevel::Read,
            origin: "https://example.com".into(),
            granted_at: 0,
            listener: None,
        };
        assert!(grant.covers(Some("https://example.com")));
        assert!(!grant.covers(Some("https://other.example")));
        assert!(!grant.covers(Some("https://example.com:8443")));
        assert!(!grant.covers(Some("http://example.com")));
        assert!(!grant.covers(None));
    }

    /// The listing names pages; it does not quote them. A title is the page's
    /// own words, so it waits for the grant that lets the agent read the rest
    /// of them.
    #[test]
    fn a_listed_tab_gives_up_its_title_only_once_it_is_readable() {
        let mut state = tab(Some("https://example.com"), TabKind::Page);
        state.title = "Re: termination letter — Mail".into();

        let closed = summarize_tab(&state).expect("a page is listed");
        assert_eq!(closed.tab_id, "t1");
        assert_eq!(closed.origin.as_deref(), Some("https://example.com"));
        assert_eq!(closed.level, GrantLevel::None);
        assert_eq!(closed.title, None);
        // And it is absent from the wire, not present-and-null.
        let wire = serde_json::to_value(&closed).expect("serialises");
        assert!(wire.get("title").is_none());
        assert_eq!(wire["tabId"], "t1");

        apply_grant(&mut state, GrantLevel::Read, 1, None).unwrap();
        let open = summarize_tab(&state).expect("a page is listed");
        assert_eq!(open.level, GrantLevel::Read);
        assert_eq!(open.title.as_deref(), Some("Re: termination letter — Mail"));
    }

    /// A tab that has not committed a document yet is still a tab. Dropping it
    /// would make the listing flicker while a page loads; a document guest is
    /// dropped because it can never be shared at all.
    #[test]
    fn a_blank_tab_is_listed_and_a_document_guest_is_not() {
        let blank = summarize_tab(&tab(None, TabKind::Page)).expect("a blank page is still a tab");
        assert_eq!(blank.origin, None);
        assert_eq!(blank.level, GrantLevel::None);

        assert_eq!(
            summarize_tab(&tab(Some("https://codeg-doc.localhost"), TabKind::Document)),
            None
        );
    }

    #[test]
    fn only_a_web_origin_can_be_shared() {
        assert_eq!(
            grantable_origin(&tab(Some("https://example.com"), TabKind::Page)),
            Ok("https://example.com")
        );
        assert_eq!(
            grantable_origin(&tab(Some("http://localhost:3000"), TabKind::Page)),
            Ok("http://localhost:3000")
        );
        assert_eq!(
            grantable_origin(&tab(None, TabKind::Page)),
            Err(NotGrantable::NoOrigin)
        );
        // A document guest is refused even though its origin looks like one
        // on this platform, because on the next platform it does not.
        assert_eq!(
            grantable_origin(&tab(Some("https://codeg-doc.localhost"), TabKind::Document)),
            Err(NotGrantable::DocumentGuest)
        );
        assert_eq!(
            grantable_origin(&tab(Some("codeg-doc://abc"), TabKind::Page)),
            Err(NotGrantable::NoOrigin)
        );
    }

    #[test]
    fn sharing_binds_to_the_origin_on_screen_and_repeating_it_is_quiet() {
        let mut state = tab(Some("https://example.com"), TabKind::Page);

        let first = apply_grant(&mut state, GrantLevel::Read, 100, None)
            .expect("a web origin can be shared")
            .expect("a first grant is a transition");
        assert_eq!(first.change, GrantChange::Granted);
        assert_eq!(first.level, GrantLevel::Read);
        assert_eq!(first.origin.as_deref(), Some("https://example.com"));
        assert_eq!(state.agent_grant.as_ref().unwrap().granted_at, 100);

        // Same level, same origin: nothing happened, and in particular the
        // clock the audit surface shows did not restart.
        assert_eq!(apply_grant(&mut state, GrantLevel::Read, 200, None), Ok(None));
        assert_eq!(state.agent_grant.as_ref().unwrap().granted_at, 100);

        // A different level is a decision, and dates from when it was made.
        let raised = apply_grant(&mut state, GrantLevel::Control, 300, None)
            .unwrap()
            .expect("raising the level is a transition");
        assert_eq!(raised.level, GrantLevel::Control);
        assert_eq!(state.agent_grant.as_ref().unwrap().granted_at, 300);
    }

    #[test]
    fn taking_it_back_reports_what_was_lost_once() {
        let mut state = tab(Some("https://example.com"), TabKind::Page);
        apply_grant(&mut state, GrantLevel::Read, 1, None).unwrap();

        let revoked = apply_grant(&mut state, GrantLevel::None, 2, None)
            .unwrap()
            .expect("revoking a live grant is a transition");
        assert_eq!(revoked.change, GrantChange::Revoked);
        assert_eq!(revoked.level, GrantLevel::None);
        assert_eq!(revoked.origin.as_deref(), Some("https://example.com"));
        assert!(state.agent_grant.is_none());

        // Revoking nothing is not an event.
        assert_eq!(apply_grant(&mut state, GrantLevel::None, 3, None), Ok(None));
    }

    /// The refusal has to happen here, not at the UI: a tab showing nothing
    /// with an origin has no way to ever lose a grant again.
    #[test]
    fn a_tab_with_no_web_origin_cannot_be_shared_at_all() {
        let mut blank = tab(None, TabKind::Page);
        assert_eq!(
            apply_grant(&mut blank, GrantLevel::Read, 1, None),
            Err(NotGrantable::NoOrigin)
        );
        assert!(blank.agent_grant.is_none());

        let mut guest = tab(Some("https://codeg-doc.localhost"), TabKind::Document);
        assert_eq!(
            apply_grant(&mut guest, GrantLevel::Control, 1, None),
            Err(NotGrantable::DocumentGuest)
        );
        assert!(guest.agent_grant.is_none());
    }

    #[test]
    fn a_departed_page_loses_the_grant_and_a_reload_does_not() {
        let mut state = tab(Some("https://example.com"), TabKind::Page);
        state.agent_grant = Some(AgentGrant {
            level: GrantLevel::Read,
            origin: "https://example.com".into(),
            granted_at: 1,
            listener: None,
        });

        // Same origin: a reload, a route change, another page on the site.
        // The dev loop is the reason this has to hold — one share, then work.
        assert_eq!(revoke_if_departed(&mut state), None);
        assert!(state.agent_grant.is_some());

        // Somewhere else, and the grant goes with it. Idempotent afterwards:
        // there is nothing left to lose, so no second notice.
        state.origin = Some("https://other.example".into());
        let lost = revoke_if_departed(&mut state).expect("the grant is taken away");
        assert_eq!(lost.origin, "https://example.com");
        assert!(state.agent_grant.is_none());
        assert_eq!(revoke_if_departed(&mut state), None);
    }

    fn serving(program: &str, workdir: &str) -> ListenerIdentity {
        ListenerIdentity {
            program: Some(program.into()),
            workdir: Some(workdir.into()),
        }
    }

    fn shared_localhost(pin: Option<ListenerIdentity>) -> BrowserTabState {
        let mut state = tab(Some("http://localhost:3000"), TabKind::Page);
        state.agent_grant = Some(AgentGrant {
            level: GrantLevel::Read,
            origin: "http://localhost:3000".into(),
            granted_at: 1,
            listener: pin,
        });
        state
    }

    /// The case the whole design turns on. A dev server under `nodemon` or
    /// `cargo watch` is a new process on every save, and an agent-driven edit
    /// loop is made of saves — so the pin is the *program*, and a restart of
    /// it passes.
    #[test]
    fn a_restarted_dev_server_keeps_the_grant() {
        let node = serving("/usr/local/bin/node", "/home/dev/project");
        let mut state = shared_localhost(Some(node.clone()));
        // A different process, indistinguishable as a program: same
        // executable, same directory. Nothing about it is a new decision for
        // the user to make.
        assert_eq!(
            revoke_if_replaced(&mut state, "http://localhost:3000", &node),
            None
        );
        assert!(state.agent_grant.is_some());
    }

    #[test]
    fn another_program_on_the_port_ends_the_grant() {
        let mut state = shared_localhost(Some(serving("/usr/local/bin/node", "/home/dev/project")));
        let lost = revoke_if_replaced(
            &mut state,
            "http://localhost:3000",
            &serving("/usr/bin/python3", "/home/dev/project"),
        )
        .expect("the grant is taken away");
        assert_eq!(lost.origin, "http://localhost:3000");
        assert!(state.agent_grant.is_none());
    }

    /// The accident this is really for: the same runtime, an hour later,
    /// serving a different project. `node` and `node` say nothing; the
    /// directory says everything.
    #[test]
    fn the_same_runtime_serving_a_different_project_ends_the_grant() {
        let mut state = shared_localhost(Some(serving("/usr/local/bin/node", "/home/dev/shop")));
        assert!(revoke_if_replaced(
            &mut state,
            "http://localhost:3000",
            &serving("/usr/local/bin/node", "/home/dev/admin-tool"),
        )
        .is_some());
        assert!(state.agent_grant.is_none());
    }

    /// A grant that was never pinned — a real site, or a loopback address
    /// this machine could not look into — is not judged by a probe. The check
    /// narrows what an agent may read or does nothing; it never widens.
    #[test]
    fn an_unpinned_grant_is_never_taken_away_by_the_check() {
        let mut state = shared_localhost(None);
        assert_eq!(
            revoke_if_replaced(&mut state, "http://localhost:3000", &serving("/x", "/y")),
            None
        );
        assert!(state.agent_grant.is_some());
    }

    /// The probe runs outside the registry lock. By the time it answers, the
    /// tab can have navigated and been shared again for somewhere else — and
    /// that grant is not the one this answer is about.
    #[test]
    fn a_probe_of_one_address_does_not_judge_a_grant_on_another() {
        let mut state = shared_localhost(Some(serving("/usr/local/bin/node", "/p")));
        assert_eq!(
            revoke_if_replaced(&mut state, "http://localhost:4000", &serving("/other", "/q")),
            None
        );
        assert!(state.agent_grant.is_some());
    }

    #[test]
    fn a_probe_is_pinned_only_to_the_address_it_was_taken_for() {
        let mut state = tab(Some("http://localhost:3000"), TabKind::Page);
        // The page moved while the probe was in flight.
        apply_grant(
            &mut state,
            GrantLevel::Read,
            1,
            Some(ProbedListener {
                origin: "http://localhost:9999".into(),
                identity: serving("/usr/local/bin/node", "/p"),
            }),
        )
        .expect("granted");
        let grant = state.agent_grant.as_ref().expect("a grant");
        assert_eq!(grant.origin, "http://localhost:3000");
        // Unpinned rather than wrongly pinned: an identity for another port
        // would revoke this grant at the next read for no nameable reason.
        assert_eq!(grant.listener, None);
    }

    /// Pressing "share" on a tab that is already shared is not a transition —
    /// no toast, and the audit surface keeps showing when access began. It is
    /// still a fresh statement about what the person means, so the pin moves.
    #[test]
    fn re_sharing_refreshes_the_pin_without_resetting_the_clock() {
        let mut state = tab(Some("http://localhost:3000"), TabKind::Page);
        apply_grant(&mut state, GrantLevel::Read, 100, None).expect("granted");
        let refreshed = apply_grant(
            &mut state,
            GrantLevel::Read,
            200,
            Some(ProbedListener {
                origin: "http://localhost:3000".into(),
                identity: serving("/usr/bin/python3", "/home/dev"),
            }),
        )
        .expect("no error");
        assert_eq!(refreshed, None, "a button already on announces nothing");
        let grant = state.agent_grant.as_ref().expect("a grant");
        assert_eq!(grant.granted_at, 100);
        assert_eq!(grant.listener, Some(serving("/usr/bin/python3", "/home/dev")));
    }

    #[test]
    fn a_program_is_named_by_its_file_name_on_either_platforms_separator() {
        assert_eq!(
            serving("/usr/local/bin/node", "/p").display_name(),
            Some("node")
        );
        assert_eq!(
            ListenerIdentity {
                program: Some(r"C:\Program Files\nodejs\node.exe".into()),
                workdir: None,
            }
            .display_name(),
            Some("node.exe")
        );
        assert_eq!(ListenerIdentity::default().display_name(), None);
    }

    /// A page that ends up with no origin at all — an `about:blank` the
    /// engine substituted for a load it refused, a `data:` document — is not
    /// the page that was shared.
    #[test]
    fn losing_the_origin_loses_the_grant() {
        let mut state = tab(Some("https://example.com"), TabKind::Page);
        state.agent_grant = Some(AgentGrant {
            level: GrantLevel::Control,
            origin: "https://example.com".into(),
            granted_at: 1,
            listener: None,
        });
        state.origin = None;
        assert!(revoke_if_departed(&mut state).is_some());
        assert!(state.agent_grant.is_none());
    }

    #[test]
    fn the_epoch_moves_with_the_incarnation_and_the_navigation() {
        assert_eq!(epoch(3, 0), "3.0");
        assert_ne!(epoch(3, 1), epoch(3, 0));
        // A reopened tab id is a different tab, and says so even if the new
        // one has navigated exactly as often as the old one had.
        assert_ne!(epoch(4, 1), epoch(3, 1));
    }

    /// The strip branches on these three words. Renaming a variant without
    /// renaming its message would leave the user reading a blank line about
    /// something an agent just did to their page.
    #[test]
    fn an_activity_line_says_which_of_the_three_things_happened() {
        let line = |outcome| {
            serde_json::to_value(AgentActivityPayload {
                tab_id: "t1".into(),
                action: AgentAction::Read,
                outcome,
                at: 1_700_000_000_000,
            })
            .expect("serialises")
        };
        assert_eq!(line(AgentOutcome::Done)["action"], "read");
        assert_eq!(line(AgentOutcome::Done)["outcome"], "done");
        assert_eq!(line(AgentOutcome::Refused)["outcome"], "refused");
        assert_eq!(line(AgentOutcome::Failed)["outcome"], "failed");
        assert_eq!(line(AgentOutcome::Done)["tabId"], "t1");
        assert_eq!(line(AgentOutcome::Done)["at"], 1_700_000_000_000i64);
    }

    #[test]
    fn the_expression_probes_before_it_calls() {
        let js = probe_and_snapshot(&SnapshotRequest { max_chars: Some(2000) }, "7.2");
        assert!(js.starts_with("typeof globalThis.__codegAgent === 'undefined'"));
        assert!(js.contains(r#""absent""#));
        assert!(js.contains(r#""epoch":"7.2""#));
        assert!(js.contains(r#""maxChars":2000"#));
        assert!(js.contains("JSON.stringify(globalThis.__codegAgent.snapshot("));
    }

    /// No cap is no cap: the option is absent rather than zero, which the
    /// bundle would also treat as "no cap" but which would claim the caller
    /// asked for something.
    #[test]
    fn an_absent_cap_stays_absent() {
        let js = probe_and_snapshot(&SnapshotRequest::default(), "1.0");
        assert!(js.contains(r#""maxChars":null"#));
    }

    /// The bundle is a program and the shim evaluates an expression, so the
    /// installing form has to be a function body that ends in the call. If
    /// this ever stops holding, world eval fails with a syntax error on a
    /// hundred kilobytes of generated source, which is a bad thing to debug
    /// on a platform one does not have.
    #[test]
    fn the_installing_form_is_one_expression_ending_in_the_call() {
        let js = install_and_snapshot(&SnapshotRequest::default(), "1.0");
        assert!(js.starts_with("(function(){\n"));
        assert!(js.ends_with(";})()"), "must be an immediately invoked expression");
        assert!(js.contains(";return JSON.stringify(globalThis.__codegAgent.snapshot("));
        // The bundle goes in whole, and it is a program: statements at the
        // top, no trailing expression of its own to be confused with ours.
        assert!(js.contains(AGENT_BUNDLE));
        assert!(AGENT_BUNDLE.trim_end().ends_with("})();"));
    }

    /// The engine really is what the host is about to call into: if the
    /// bundle stopped publishing this name, every snapshot would come back
    /// `absent` forever and the retry would install it again each time.
    #[test]
    fn the_bundle_publishes_the_global_the_host_calls() {
        assert!(AGENT_BUNDLE.contains(&format!("globalThis.{AGENT_GLOBAL} = ")));
        assert!(AGENT_BUNDLE.contains("snapshot"));
        assert!(AGENT_BUNDLE.contains("elementForRef"));
    }
}
